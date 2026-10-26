//! The colour kit as a ZIP:
//!
//! ```text
//! LEEME.txt               what is in the kit, what to check, how to start
//! resumen.html            the same, with colours, filaments and links
//! guia-de-armado.html     pictures of every piece, plate and step
//! guia-de-armado.txt      the same steps as plain text
//! 3mf/<modelo>_placas.3mf every piece on a plate of its colour
//! stl/<n>-<color>/P07.stl one STL per piece, by filament
//! img/…                   the guide's pictures (it works offline)
//! ```
//!
//! A piece is called by its label (`P07`) everywhere: its STL file and header,
//! its 3MF object, and the guide.

use std::fmt::Write as _;
use std::io::{Seek, Write};

use bp_core::kit::{Bed, Issue, Kit};
use bp_core::render::{self, Camera, Frame, Item, Style};
use rayon::prelude::*;
use zip::write::SimpleFileOptions;

use crate::plate::Reserve;
use crate::project_file::KitSettings;
use crate::{escape, write_plates, write_stl, Flush, PlateObject, PlateProject, Printer};

/// The printer's plate as the kit packs it: its printable area and excluded
/// zones, the margin to the edges, `gap_mm` between pieces, and the prime
/// tower (`reserve`, with its gap) for plates where colours mix.
pub fn kit_bed(printer: &Printer, reserve: &Reserve, gap_mm: f32, mix: bool) -> Bed {
    let a = printer.printable_area();
    let tower = reserve.tower.then(|| {
        let (x, y) = printer.tower_origin();
        let g = reserve.tower_gap_mm.max(0.0);
        [x - g, y - g, x + reserve.tower_mm[0] + g, y + reserve.tower_mm[1] + g]
    });
    Bed {
        area: [a.x0, a.y0, a.x1, a.y1],
        excluded: printer.excluded_areas().iter().map(|e| [e.x0, e.y0, e.x1, e.y1]).collect(),
        tower,
        margin_mm: reserve.margin_mm,
        gap_mm,
        mix,
    }
}

pub struct KitInfo<'a> {
    /// Model name, used for file names and titles.
    pub name: &'a str,
    /// `#RRGGBB` of every filament slot.
    pub palette: &'a [String],
    /// The filament on each slot, where the user picked one.
    pub slots: &'a [Option<crate::filament::SlotFilament>],
    /// How the user's filament on each slot is called ("Bambu Lab PLA Basic Naranja").
    pub filament_names: &'a [Option<String>],
    pub printer: &'a Printer,
    /// How the kit was cut, for the summary.
    pub params: KitSettings,
    /// Filament density, to weigh each colour.
    pub density_g_cm3: f32,
    /// Flushes between filaments, for plates where colours mix.
    pub flush: Flush,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitSummary {
    pub pieces: usize,
    pub plates: usize,
    /// Pieces larger than the plate.
    pub oversize: usize,
    /// Pieces with anything to look at before printing (see [`bp_core::kit::check`]).
    pub review: usize,
    /// Crumbs that went to a neighbour, and regions too thin to make a piece.
    pub merged: usize,
    pub dropped: usize,
}

/// Spanish name for a filament colour, close enough to tell spools apart.
pub fn color_name(rgb: [u8; 3]) -> &'static str {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    if l < 0.14 {
        return "Negro";
    }
    if s < 0.12 || d < 0.06 {
        return match l {
            l if l > 0.9 => "Blanco",
            l if l > 0.65 => "Gris claro",
            l if l > 0.35 => "Gris",
            _ => "Gris oscuro",
        };
    }
    let warm = !(50.0..345.0).contains(&h);
    if warm && l < 0.42 && s < 0.75 {
        return if l < 0.26 { "Marrón oscuro" } else { "Marrón" };
    }
    match h {
        h if !(15.0..345.0).contains(&h) => {
            if l < 0.35 {
                "Granate"
            } else {
                "Rojo"
            }
        }
        h if h < 45.0 => match l {
            l if l > 0.8 => "Crema",
            l if s < 0.85 && l > 0.5 => "Caramelo",
            _ => "Naranja",
        },
        h if h < 70.0 => {
            if l > 0.85 {
                "Crema"
            } else {
                "Amarillo"
            }
        }
        h if h < 165.0 => "Verde",
        h if h < 200.0 => "Turquesa",
        h if h < 260.0 => "Azul",
        h if h < 300.0 => "Morado",
        _ => "Rosa",
    }
}

fn rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(128);
    [byte(0), byte(2), byte(4)]
}

/// ASCII, lower case, dashes: safe in any file name.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn png(frame: &Frame) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, frame.width as u32, frame.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(std::io::Error::other)?;
        w.write_image_data(&frame.rgb).map_err(std::io::Error::other)?;
    }
    Ok(buf)
}

/// A picture plus the labels to pin on it, at fractions of its size.
struct Shot {
    png: Vec<u8>,
    width: usize,
    height: usize,
    tags: Vec<(usize, [f32; 2], bool)>,
    /// A ring around the new piece (left, top, right, bottom); its label then
    /// sits on the ring instead of over the piece.
    ring: Option<[f32; 4]>,
}

impl Shot {
    fn new(frame: &Frame, tags: Vec<(usize, [f32; 2], bool)>) -> std::io::Result<Self> {
        Ok(Self { png: png(frame)?, width: frame.width, height: frame.height, tags, ring: None })
    }

    /// Rings piece `id` and moves its label onto the ring.
    fn focus(mut self, frame: &Frame, id: usize) -> Self {
        if let Some(b) = frame.bbox(id as u32) {
            let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            let (rw, rh) = ((b[2] - b[0]) / 2.0 + 0.03, (b[3] - b[1]) / 2.0 + 0.04);
            let (rw, rh) = (rw.max(0.05), rh.max(0.07));
            let ring = [cx - rw, cy - rh, cx + rw, cy + rh];
            self.ring = Some(ring);
            self.tags.retain(|t| t.0 != id);
            // On the ring's upper right, at 45°.
            let at = [cx + rw * 0.707, cy - rh * 0.707];
            self.tags.push((id, at, true));
        }
        self
    }

    /// The picture, saved at `src` next to the page, with its labels and ring.
    fn html(&self, src: &str, alt: &str, label: &dyn Fn(usize) -> String) -> String {
        let mut s = format!(
            "<figure class=\"shot\"><img width=\"{}\" height=\"{}\" alt=\"{}\" src=\"{}\" loading=\"lazy\">",
            self.width,
            self.height,
            escape(alt),
            escape(src)
        );
        if let Some(r) = self.ring {
            let _ = write!(
                s,
                "<span class=\"ring\" style=\"left:{:.1}%;top:{:.1}%;width:{:.1}%;height:{:.1}%\"></span>",
                r[0] * 100.0,
                r[1] * 100.0,
                (r[2] - r[0]) * 100.0,
                (r[3] - r[1]) * 100.0
            );
        }
        for &(piece, at, strong) in &self.tags {
            let _ = write!(
                s,
                "<span class=\"tag{}\" style=\"left:{:.1}%;top:{:.1}%\">{}</span>",
                if strong { " tag--focus" } else { "" },
                at[0] * 100.0,
                at[1] * 100.0,
                label(piece)
            );
        }
        s.push_str("</figure>");
        s
    }
}

/// Drops labels that would cover one already placed (biggest pieces come first).
fn declutter(tags: impl Iterator<Item = (usize, [f32; 2], bool)>) -> Vec<(usize, [f32; 2], bool)> {
    let mut kept: Vec<(usize, [f32; 2], bool)> = vec![];
    for t in tags {
        if kept.iter().all(|k| (k.1[0] - t.1[0]).abs() > 0.085 || (k.1[1] - t.1[1]).abs() > 0.06) {
            kept.push(t);
        }
    }
    kept
}

const VIEWS: [(f32, f32); 6] =
    [(35.0, 28.0), (125.0, 28.0), (215.0, 28.0), (305.0, 28.0), (35.0, -25.0), (215.0, -25.0)];

struct Pictures {
    overview: Vec<Shot>,
    thumbs: Vec<Shot>,
    plates: Vec<Shot>,
    /// Each plate's picture for Bambu Studio's plate list: large and small PNG.
    covers: Vec<(Vec<u8>, Vec<u8>)>,
    steps: Vec<Shot>,
    /// A close-up for steps whose piece is small in the main picture.
    details: Vec<Option<Shot>>,
}

fn bounds<'a>(points: impl Iterator<Item = &'a [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in points {
        lo = std::array::from_fn(|a| lo[a].min(p[a]));
        hi = std::array::from_fn(|a| hi[a].max(p[a]));
    }
    (lo, hi)
}

fn pictures(kit: &Kit, colours: &[[u8; 3]]) -> std::io::Result<Pictures> {
    let (lo, hi) = bounds(kit.pieces.iter().flat_map(|p| p.positions.iter()));
    let item = |i: usize, style: Style| Item {
        positions: &kit.pieces[i].positions,
        tris: &kit.pieces[i].tris,
        rgb: colours[i],
        id: i as u32,
        style,
    };

    let overview = [(35.0, 28.0), (215.0, 28.0)]
        .par_iter()
        .map(|&(az, el)| {
            let items: Vec<Item> = (0..kit.pieces.len()).map(|i| item(i, Style::Normal)).collect();
            Shot::new(&render::render(&Camera::new(az, el, lo, hi, 560, 420), &items, 2), vec![])
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let thumbs = (0..kit.pieces.len())
        .into_par_iter()
        .map(|i| {
            let (a, b) = bounds(kit.pieces[i].positions.iter());
            Shot::new(&render::render(&Camera::new(35.0, 28.0, a, b, 200, 150), &[item(i, Style::Normal)], 2), vec![])
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let plate_items = |k: usize| -> Vec<Vec<[f32; 3]>> {
        kit.plates[k]
            .pieces
            .iter()
            .map(|&i| {
                let pl = &kit.placed[i];
                pl.positions.iter().map(|v| [v[0] + pl.at[0], v[1] + pl.at[1], v[2]]).collect()
            })
            .collect()
    };
    let covers = (0..kit.plates.len())
        .into_par_iter()
        .map(|k| {
            let moved = plate_items(k);
            let items: Vec<Item> = kit.plates[k]
                .pieces
                .iter()
                .zip(&moved)
                .map(|(&i, m)| Item {
                    positions: m,
                    tris: &kit.pieces[i].tris,
                    rgb: colours[i],
                    id: i as u32,
                    style: Style::Normal,
                })
                .collect();
            let (a, b) = bounds(moved.iter().flatten());
            let shot = |size: usize| png(&render::render(&Camera::new(35.0, 28.0, a, b, size, size), &items, 2));
            Ok((shot(512)?, shot(128)?))
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let plates = (0..kit.plates.len())
        .into_par_iter()
        .map(|k| {
            let ids = &kit.plates[k].pieces;
            let moved: Vec<Vec<[f32; 3]>> = ids
                .iter()
                .map(|&i| {
                    let pl = &kit.placed[i];
                    pl.positions.iter().map(|v| [v[0] + pl.at[0], v[1] + pl.at[1], v[2]]).collect()
                })
                .collect();
            let items: Vec<Item> = ids
                .iter()
                .zip(&moved)
                .map(|(&i, m)| Item {
                    positions: m,
                    tris: &kit.pieces[i].tris,
                    rgb: colours[i],
                    id: i as u32,
                    style: Style::Normal,
                })
                .collect();
            // Framed on what is on the plate, not the whole bed, so small pieces stay legible.
            let (mut a, mut b) = bounds(moved.iter().flatten());
            for k in 0..2 {
                a[k] = (a[k] - 6.0).max(kit.bed.area[k]);
                b[k] = (b[k] + 6.0).min(kit.bed.area[k + 2]);
            }
            let cam = Camera::new(0.0, 89.0, [a[0], a[1], 0.0], [b[0], b[1], b[2].max(1.0)], 520, 360);
            let frame = render::render(&cam, &items, 2);
            Shot::new(&frame, declutter(ids.iter().filter_map(|&i| frame.anchor(i as u32).map(|a| (i, a, false)))))
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let mut built = vec![vec![]; kit.steps.len()];
    let mut so_far: Vec<usize> = vec![];
    for (k, step) in kit.steps.iter().enumerate() {
        built[k] = so_far.clone();
        so_far.push(step.piece);
    }
    let steps = kit
        .steps
        .par_iter()
        .zip(&built)
        .map(|(step, before)| {
            let mut items: Vec<Item> = before.iter().map(|&i| item(i, Style::Faded)).collect();
            items.push(item(step.piece, Style::Focus));
            // The view that shows most of the new piece.
            let (az, el) = VIEWS
                .iter()
                .copied()
                .max_by_key(|&(az, el)| {
                    render::render(&Camera::new(az, el, lo, hi, 160, 120), &items, 1).count(step.piece as u32)
                })
                .unwrap_or(VIEWS[0]);
            let frame = render::render(&Camera::new(az, el, lo, hi, 640, 460), &items, 2);
            let tags = declutter(step.joins.iter().filter_map(|&j| frame.anchor(j as u32).map(|a| (j, a, false))));
            let shot = Shot::new(&frame, tags)?.focus(&frame, step.piece);
            let small = frame.bbox(step.piece as u32).is_none_or(|b| b[2] - b[0] < 0.3 && b[3] - b[1] < 0.3);
            let detail = if small {
                // Frame the piece with room around it for what it sits against.
                let (a, b) = bounds(kit.pieces[step.piece].positions.iter());
                let c: [f32; 3] = std::array::from_fn(|k| (a[k] + b[k]) / 2.0);
                let r = (0..3).map(|k| b[k] - a[k]).fold(0.0f32, f32::max) * 0.9 + 4.0;
                let cam = Camera::new(az, el, c.map(|x| x - r), c.map(|x| x + r), 320, 240);
                let frame = render::render(&cam, &items, 2);
                Some(Shot::new(&frame, vec![])?.focus(&frame, step.piece))
            } else {
                None
            };
            Ok((shot, detail))
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let (steps, details) = steps.into_iter().unzip();
    Ok(Pictures { overview, thumbs, plates, covers, steps, details })
}

const CSS: &str = r#"
:root{--paper:#f8f5ec;--paper-2:#eef4ee;--ink:#17231e;--ink-2:#2c3a33;--muted:#5f6f67;--rule:#d5dccd;--accent:#1f7a55;--accent-soft:#cfe9da;--card:#fffdf7}
*{box-sizing:border-box}
body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.5 "Instrument Sans",ui-sans-serif,system-ui,sans-serif}
main{max-width:1080px;margin:0 auto;padding:40px 16px 96px}
h1,h2,h3{font-family:"Schibsted Grotesk",ui-sans-serif,system-ui,sans-serif;letter-spacing:-.02em;line-height:1.15;margin:0}
h1{font-size:2.1rem}h2{font-size:1.45rem;margin:56px 0 16px}h3{font-size:1.05rem}
p{margin:0 0 12px}.lead{color:var(--ink-2);font-size:1.05rem;max-width:68ch}
.mono{font-family:"DM Mono",ui-monospace,monospace;font-size:.9em}
.meta{display:flex;flex-wrap:wrap;gap:8px;margin:16px 0 24px}
.meta span{background:var(--paper-2);border:1px solid var(--rule);border-radius:999px;padding:4px 12px;font-size:.9rem}
.pair{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:16px}
.shot{position:relative;margin:0;background:#f4f1e8;border:1px solid var(--rule);border-radius:14px;overflow:hidden}
.shot img{display:block;width:100%;height:auto}
.tag{position:absolute;transform:translate(-50%,-50%);background:var(--card);color:var(--ink);border:1.5px solid var(--ink);border-radius:999px;padding:1px 7px;font:600 12px/1.4 "DM Mono",ui-monospace,monospace;white-space:nowrap;box-shadow:0 1px 3px #0003}
.tag--focus{background:var(--ink);color:var(--card);font-size:14px;padding:2px 9px}
.ring{position:absolute;border:2.5px solid var(--ink);border-radius:50%;box-shadow:0 0 0 3px #fffdf7aa}
.detail{margin-top:12px;max-width:320px}.detail small{color:var(--muted)}
ol.prep{padding-left:22px;max-width:72ch}ol.prep li{margin-bottom:8px}
.legend{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}
.chip{display:flex;gap:12px;align-items:center;background:var(--card);border:1px solid var(--rule);border-radius:14px;padding:12px}
.sw{width:36px;height:36px;border-radius:50%;border:1px solid #0002;flex:none}
.sw--s{width:14px;height:14px;display:inline-block;vertical-align:-2px;margin-right:4px}
.chip small,.card small{color:var(--muted);display:block}
.plates{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:16px}
.plate h3{margin:10px 0 2px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(170px,1fr));gap:12px}
.card{background:var(--card);border:1px solid var(--rule);border-radius:14px;overflow:hidden}
.card .shot{border:0;border-radius:0;border-bottom:1px solid var(--rule)}
.card div{padding:8px 10px 10px;font-size:.9rem}
.card b{font-family:"DM Mono",ui-monospace,monospace}
.warn{color:#9a3412}
.alert{background:#fdf0e6;border:1px solid #f0c9a8;border-radius:14px;padding:12px 16px;margin:20px 0}
.card--warn{border-color:#e6a774;box-shadow:inset 0 3px 0 #e6a774}
table{border-collapse:collapse;width:100%;margin:8px 0 12px;font-size:.95rem}
th,td{text-align:left;padding:8px 10px;border-bottom:1px solid var(--rule);vertical-align:top}
thead th{font-size:.8rem;text-transform:uppercase;letter-spacing:.05em;color:var(--muted)}
ul.files{padding-left:20px;max-width:72ch}ul.files li{margin-bottom:6px}
a{color:var(--accent)}
.step{display:grid;grid-template-columns:minmax(0,3fr) minmax(220px,2fr);gap:20px;align-items:center;padding:20px 0;border-top:1px solid var(--rule)}
.step .n{font:600 .8rem "DM Mono",ui-monospace,monospace;color:var(--accent);text-transform:uppercase;letter-spacing:.06em}
.step h3{font-size:1.25rem;margin:4px 0 8px}
@media (max-width:720px){.step{grid-template-columns:1fr}}
@media print{body{background:#fff}main{padding:0}.step{break-inside:avoid}h2{break-after:avoid}}
"#;

/// Where each picture of the guide is saved, next to the page.
struct Paths {
    overview: Vec<String>,
    thumbs: Vec<String>,
    plates: Vec<String>,
    steps: Vec<String>,
    details: Vec<Option<String>>,
}

impl Paths {
    fn new(kit: &Kit, pics: &Pictures, labels: &[String]) -> Self {
        Self {
            overview: (1..=pics.overview.len()).map(|k| format!("img/modelo-{k}.png")).collect(),
            thumbs: labels.iter().map(|l| format!("img/pieza-{l}.png")).collect(),
            plates: (1..=kit.plates.len()).map(|k| format!("img/placa-{k:02}.png")).collect(),
            steps: (1..=kit.steps.len()).map(|k| format!("img/paso-{k:03}.png")).collect(),
            details: pics
                .details
                .iter()
                .enumerate()
                .map(|(k, d)| d.as_ref().map(|_| format!("img/paso-{:03}-detalle.png", k + 1)))
                .collect(),
        }
    }

    /// Every picture with its path, to put in the ZIP.
    fn files<'a>(&'a self, pics: &'a Pictures) -> impl Iterator<Item = (&'a str, &'a [u8])> {
        let pairs = |paths: &'a [String], shots: &'a [Shot]| {
            paths.iter().zip(shots).map(|(p, s)| (p.as_str(), s.png.as_slice()))
        };
        pairs(&self.overview, &pics.overview)
            .chain(pairs(&self.thumbs, &pics.thumbs))
            .chain(pairs(&self.plates, &pics.plates))
            .chain(pairs(&self.steps, &pics.steps))
            .chain(
                self.details
                    .iter()
                    .zip(&pics.details)
                    .filter_map(|(p, s)| Some((p.as_deref()?, s.as_ref()?.png.as_slice()))),
            )
    }
}

/// `n` and the word for it, singular or plural ("1 pieza", "3 piezas").
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a piece needs looked at, as the user reads it.
fn issue_text(issue: &Issue) -> String {
    match issue {
        Issue::Open => "su superficie no cierra: el laminador puede rellenarla mal".into(),
        Issue::Degenerate(n) => format!("{n} triángulos sin área"),
        Issue::Oversize => "es más grande que la placa: escálala o córtala en Bambu Studio".into(),
        Issue::Thin(mm) => format!("su lado más fino mide {mm:.1} mm, menos de dos líneas de la boquilla"),
        Issue::Tiny(mm) => format!("mide {mm:.1} mm como mucho: difícil de imprimir y de manipular"),
    }
}

/// Everything the guide, the summary and the file names share.
struct Names<'a> {
    kit: &'a Kit,
    info: &'a KitInfo<'a>,
    labels: Vec<String>,
    files: Vec<String>,
    three_mf: String,
    issues: Vec<Vec<Issue>>,
}

impl<'a> Names<'a> {
    fn new(kit: &'a Kit, info: &'a KitInfo<'a>) -> Self {
        let digits = kit.pieces.len().to_string().len().max(2);
        let labels: Vec<String> = (0..kit.pieces.len()).map(|i| format!("P{:0digits$}", i + 1)).collect();
        let base = slug(info.name);
        let base = if base.is_empty() { "modelo".to_string() } else { base };
        let mut names =
            Self { kit, info, labels, files: vec![], three_mf: format!("3mf/{base}_placas.3mf"), issues: vec![] };
        names.files = (0..kit.pieces.len())
            .map(|i| {
                let s = kit.pieces[i].slot;
                format!("stl/{}-{}/{}.stl", s + 1, slug(color_name(rgb(&names.hex(s)))), names.labels[i])
            })
            .collect();
        names.issues = bp_core::kit::check(kit, info.printer.nozzle_mm);
        names
    }

    fn hex(&self, slot: u8) -> String {
        self.info.palette.get(slot as usize).cloned().unwrap_or_else(|| "#808080".into())
    }

    /// The colour's everyday name ("Naranja").
    fn colour(&self, slot: u8) -> &'static str {
        color_name(rgb(&self.hex(slot)))
    }

    /// The spool to load: the user's filament when they picked one, else the colour's name.
    fn filament(&self, slot: u8) -> String {
        self.info.filament_names.get(slot as usize).cloned().flatten().unwrap_or_else(|| self.colour(slot).to_string())
    }

    fn piece_colour(&self, i: usize) -> &'static str {
        self.colour(self.kit.pieces[i].slot)
    }

    fn slots(&self) -> Vec<u8> {
        let mut slots: Vec<u8> = self.kit.pieces.iter().map(|p| p.slot).collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    fn plate_name(&self, k: usize) -> String {
        let slots = &self.kit.plates[k].slots;
        match slots.as_slice() {
            [s] => format!("Placa {} · {} {}", k + 1, self.filament(*s), self.hex(*s)),
            _ => format!(
                "Placa {} · {}",
                k + 1,
                slots.iter().map(|&s| format!("{} {}", self.filament(s), self.hex(s))).collect::<Vec<_>>().join(" + ")
            ),
        }
    }

    fn grams(&self, mm3: f32) -> f32 {
        mm3 * self.info.density_g_cm3 / 1000.0
    }

    /// The pieces to look at, with why.
    fn review(&self) -> Vec<(usize, &[Issue])> {
        self.issues.iter().enumerate().filter(|(_, v)| !v.is_empty()).map(|(i, v)| (i, v.as_slice())).collect()
    }

    /// What the export changed or left out on its own, for the user to know.
    fn notes(&self) -> Vec<String> {
        let mut notes = vec![];
        let p = &self.info.params;
        if self.kit.voxel_mm > p.voxel_mm * 1.01 {
            notes.push(format!(
                "El modelo es grande para el detalle pedido: las uniones se calcularon con {:.2} mm en lugar de {:.2} mm.",
                self.kit.voxel_mm, p.voxel_mm
            ));
        }
        if self.kit.merged > 0 {
            notes.push(format!(
                "{} de menos de {:.0} mm³ {} a la pieza vecina con la que más contacto {}.",
                count(self.kit.merged, "miga", "migas"),
                p.min_piece_mm3,
                if self.kit.merged == 1 { "se unió" } else { "se unieron" },
                if self.kit.merged == 1 { "tenía" } else { "tenían" }
            ));
        }
        if self.kit.dropped > 0 {
            notes.push(format!(
                "{} para formar una pieza {} fuera; revisa el modelo armado por si falta algo.",
                count(self.kit.dropped, "zona demasiado fina", "zonas demasiado finas"),
                if self.kit.dropped == 1 { "se dejó" } else { "se dejaron" }
            ));
        }
        notes
    }
}

fn guide_html(n: &Names, pics: &Pictures, paths: &Paths) -> String {
    let (kit, info, labels) = (n.kit, n.info, &n.labels);
    let label = |i: usize| labels[i].clone();
    let swatch = |hex: &str| format!("<span class=\"sw sw--s\" style=\"background:{hex}\"></span>");
    let (lo, hi) = bounds(kit.pieces.iter().flat_map(|p| p.positions.iter()));
    let slots = n.slots();

    let mut h = String::with_capacity(1 << 18);
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"es\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Guía de armado · {name}</title><style>{CSS}</style></head><body><main>\
<h1>Guía de armado · {name}</h1>\
<p class=\"lead\">El modelo dividido en piezas sólidas, una o varias por color. Imprime cada placa con su filamento y pega las piezas en el orden de los pasos. El <a href=\"resumen.html\">resumen</a> dice qué trae el kit y qué revisar.</p>\
<div class=\"meta\"><span>{np}</span><span>{c}</span><span>{p}</span><span>{x:.0} × {y:.0} × {z:.0} mm armado</span><span>Uniones de {v:.2} mm</span></div>\
<div class=\"pair\">{o1}{o2}</div>",
        name = escape(info.name),
        np = count(kit.pieces.len(), "pieza", "piezas"),
        c = count(slots.len(), "color", "colores"),
        p = count(kit.plates.len(), "placa", "placas"),
        x = hi[0] - lo[0],
        y = hi[1] - lo[1],
        z = hi[2] - lo[2],
        v = kit.voxel_mm,
        o1 = pics.overview[0].html(&paths.overview[0], "Modelo armado, vista frontal", &label),
        o2 = pics.overview[1].html(&paths.overview[1], "Modelo armado, vista trasera", &label),
    );

    let review = n.review();
    if !review.is_empty() {
        let _ = write!(
            h,
            "<div class=\"alert\"><b>{} que revisar antes de imprimir:</b> ",
            count(review.len(), "pieza", "piezas")
        );
        let links: Vec<String> = review.iter().map(|(i, _)| format!("<a href=\"#{0}\">{0}</a>", labels[*i])).collect();
        let _ = write!(h, "{}. Su ficha dice por qué.</div>", links.join(", "));
    }

    let _ = write!(
        h,
        "<h2>Antes de empezar</h2><ol class=\"prep\">\
<li>Abre <span class=\"mono\">{three_mf}</span> en Bambu Studio. Cada placa lleva un solo color: imprímela con el filamento que indica su nombre.</li>\
<li>Las piezas ya están giradas sobre su cara más plana. Si Bambu avisa de voladizos, activa soportes en árbol solo sobre la placa.</li>\
<li>Al despegar cada placa, separa las piezas por número usando la sección <b>Piezas</b>: cada una aparece con su forma y su color. Su STL, en <span class=\"mono\">stl/</span>, lleva el mismo número.</li>\
<li>Prueba cada unión en seco antes de pegar. Lija un poco las caras de contacto para que el pegamento agarre.</li>\
<li>Para PLA usa cianoacrilato en gel; para uniones grandes, epoxi de 5 minutos. Sujeta unos segundos y sigue con el siguiente paso.</li>\
</ol>",
        three_mf = escape(&n.three_mf)
    );

    h.push_str("<h2>Filamentos</h2><div class=\"legend\">");
    for &s in &slots {
        let hex = n.hex(s);
        let count_of = kit.pieces.iter().filter(|p| p.slot == s).count();
        let plates: Vec<String> = kit
            .plates
            .iter()
            .enumerate()
            .filter(|(_, p)| p.slots.contains(&s))
            .map(|(k, _)| (k + 1).to_string())
            .collect();
        let _ = write!(
            h,
            "<div class=\"chip\"><span class=\"sw\" style=\"background:{hex}\"></span><div><b>{name}</b> <span class=\"mono\">{hex}</span>\
<small>Ranura {f} · {pieces} · placa{pl} {list}</small></div></div>",
            name = escape(&n.filament(s)),
            f = s + 1,
            pieces = count(count_of, "pieza", "piezas"),
            pl = if plates.len() > 1 { "s" } else { "" },
            list = plates.join(", ")
        );
    }
    h.push_str("</div>");

    h.push_str("<h2>Placas</h2><div class=\"plates\">");
    for (k, plate) in kit.plates.iter().enumerate() {
        let colours: Vec<String> =
            plate.slots.iter().map(|&s| format!("{}{}", swatch(&n.hex(s)), escape(&n.filament(s)))).collect();
        let _ = write!(
            h,
            "<div class=\"plate\" id=\"placa-{n1}\">{shot}<h3>Placa {n1} · {name}</h3><small>{pieces} · ocupa el {occ:.0} %{tower}: {list}</small></div>",
            shot = pics.plates[k].html(&paths.plates[k], &format!("Placa {}", k + 1), &label),
            n1 = k + 1,
            name = colours.join(" + "),
            pieces = count(plate.pieces.len(), "pieza", "piezas"),
            occ = plate.occupancy * 100.0,
            tower = if plate.mixed() { " · con torre de purga" } else { "" },
            list = plate.pieces.iter().map(|&i| format!("<a href=\"#{0}\">{0}</a>", labels[i])).collect::<Vec<_>>().join(", ")
        );
    }
    h.push_str("</div>");

    h.push_str("<h2>Piezas</h2><div class=\"grid\">");
    for (i, p) in kit.pieces.iter().enumerate() {
        let hex = n.hex(p.slot);
        let s = kit.placed[i].size;
        let warn: String =
            n.issues[i].iter().map(|x| format!("<small class=\"warn\">{}</small>", escape(&issue_text(x)))).collect();
        let _ = write!(
            h,
            "<div class=\"card{flag}\" id=\"{label}\">{shot}<div><b>{label}</b> · {sw}{name}<small>{x:.0} × {y:.0} × {z:.0} mm · {vol:.1} cm³</small>\
<small><a href=\"#placa-{plate}\">Placa {plate}</a> · <a class=\"mono\" href=\"{file}\">{file}</a></small>{warn}</div></div>",
            flag = if n.issues[i].is_empty() { "" } else { " card--warn" },
            shot = pics.thumbs[i].html(&paths.thumbs[i], &labels[i], &label),
            label = labels[i],
            sw = swatch(&hex),
            name = n.piece_colour(i),
            x = s[0],
            y = s[1],
            z = s[2],
            vol = p.volume_mm3 / 1000.0,
            plate = kit.placed[i].plate + 1,
            file = escape(&n.files[i]),
        );
    }
    h.push_str("</div>");

    let _ = write!(h, "<h2>Armado</h2><p class=\"lead\">{} pasos. En cada imagen la pieza nueva aparece en su color con un contorno grueso; lo ya armado, en tonos suaves.</p>", kit.steps.len());
    for k in 0..kit.steps.len() {
        let (title, body) = step_text(n, k);
        let _ = write!(
            h,
            "<section class=\"step\">{shot}<div><div class=\"n\">Paso {n1} de {total}</div><h3>{title}</h3><p>{body}</p>{detail}</div></section>",
            shot = pics.steps[k].html(&paths.steps[k], &format!("Paso {}", k + 1), &label),
            detail = match (&pics.details[k], &paths.details[k]) {
                (Some(d), Some(src)) =>
                    format!("<div class=\"detail\">{}<small>Detalle</small></div>", d.html(src, &format!("Detalle del paso {}", k + 1), &label)),
                _ => String::new(),
            },
            n1 = k + 1,
            total = kit.steps.len(),
        );
    }
    h.push_str("<p class=\"lead\" style=\"margin-top:40px\">Listo: el modelo está completo. Deja curar el pegamento antes de manipularlo.</p></main></body></html>\n");
    h
}

/// Title and explanation of step `k`, shared by the HTML and the text guide.
fn step_text(n: &Names, k: usize) -> (String, String) {
    let (kit, labels) = (n.kit, &n.labels);
    let step = &kit.steps[k];
    let p = step.piece;
    let title = format!("{} · {}", labels[p], n.piece_colour(p));
    let area = |j: usize| kit.pieces[p].contacts.iter().find(|c| c.0 == j).map_or(0.0, |c| c.1);
    let body = if k == 0 {
        format!("Empieza con {}: es la pieza más grande y sirve de base. Las demás se pegan sobre ella.", labels[p])
    } else if step.joins.is_empty() {
        format!(
            "{} no toca ninguna pieza anterior: empieza un grupo aparte. Ármalo con los pasos siguientes y únelo al final por donde encaje.",
            labels[p]
        )
    } else {
        let list: Vec<String> = step.joins.iter().map(|&j| format!("{} ({:.0} mm²)", labels[j], area(j))).collect();
        let tail = if step.joins.len() > 1 { " Encájala en el hueco entre ellas antes de pegar." } else { "" };
        format!("Pega {} a {}.{tail}", labels[p], list.join(", "))
    };
    (title, body)
}

fn guide_text(n: &Names) -> String {
    let (kit, info, labels) = (n.kit, n.info, &n.labels);
    let mut t = format!(
        "Guía de armado · {}\n\n{} piezas en {} placas. Abre {} en Bambu Studio e imprime cada placa con su filamento.\n\
La guía con imágenes está en guia-de-armado.html y el resumen del kit en resumen.html.\n\nPLACAS\n",
        info.name,
        kit.pieces.len(),
        kit.plates.len(),
        n.three_mf
    );
    for (k, plate) in kit.plates.iter().enumerate() {
        let _ = writeln!(
            t,
            "  {:<40} {}",
            n.plate_name(k),
            plate.pieces.iter().map(|&i| labels[i].as_str()).collect::<Vec<_>>().join(" ")
        );
    }
    t.push_str("\nPIEZAS\n");
    for (i, p) in kit.pieces.iter().enumerate() {
        let s = kit.placed[i].size;
        let _ = writeln!(
            t,
            "  {}  {:<14} {:>5.0} × {:>4.0} × {:>4.0} mm  {:>7.1} cm³  placa {:>2}  {}",
            labels[i],
            n.piece_colour(i),
            s[0],
            s[1],
            s[2],
            p.volume_mm3 / 1000.0,
            kit.placed[i].plate + 1,
            n.files[i],
        );
        for issue in &n.issues[i] {
            let _ = writeln!(t, "        ! {}", issue_text(issue));
        }
    }
    t.push_str("\nARMADO\n");
    for k in 0..kit.steps.len() {
        let (title, body) = step_text(n, k);
        let _ = writeln!(t, "  {:>3}. {title}\n       {body}", k + 1);
    }
    t
}

/// Per colour: pieces, plates, volume and weight.
fn colour_rows(n: &Names) -> Vec<(u8, usize, usize, f32)> {
    n.slots()
        .into_iter()
        .map(|s| {
            let pieces: Vec<&bp_core::Piece> = n.kit.pieces.iter().filter(|p| p.slot == s).collect();
            let plates = n.kit.plates.iter().filter(|p| p.slots.contains(&s)).count();
            (s, pieces.len(), plates, pieces.iter().map(|p| p.volume_mm3).sum())
        })
        .collect()
}

fn params_text(n: &Names) -> Vec<(String, String)> {
    let p = &n.info.params;
    let printer = &n.info.printer;
    vec![
        ("Impresora".into(), format!("{} · boquilla de {} mm", printer.profile.name, printer.nozzle_mm)),
        ("Detalle de las uniones".into(), format!("{:.2} mm (pedido {:.2} mm)", n.kit.voxel_mm, p.voxel_mm)),
        (
            "Profundidad de cada color".into(),
            if p.depth_mm > 0.0 {
                let core = p.core_slot.map_or_else(|| "el más usado".to_string(), |s| format!("ranura {}", s + 1));
                format!("{:.1} mm sobre un núcleo de {core}", p.depth_mm)
            } else {
                "hasta el centro: piezas macizas".into()
            },
        ),
        ("Piezas mínimas".into(), format!("{:.0} mm³; las menores se unen a su vecina", p.min_piece_mm3)),
        (
            "Placas".into(),
            format!(
                "{}; {:.0} mm al borde, {:.0} mm entre piezas{}",
                if n.kit.bed.mix { "colores mezclados, con torre de purga" } else { "un color por placa, sin purga" },
                n.kit.bed.margin_mm,
                n.kit.bed.gap_mm,
                if n.kit.bed.excluded.is_empty() { "" } else { ", lejos de las zonas que la impresora excluye" }
            ),
        ),
        ("Densidad del filamento".into(), format!("{:.2} g/cm³", n.info.density_g_cm3)),
    ]
}

fn summary_html(n: &Names) -> String {
    let (kit, info, labels) = (n.kit, n.info, &n.labels);
    let mut h = String::with_capacity(1 << 15);
    let total: f32 = kit.pieces.iter().map(|p| p.volume_mm3).sum();
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"es\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Resumen del kit · {name}</title><style>{CSS}</style></head><body><main>\
<h1>Resumen del kit · {name}</h1>\
<div class=\"meta\"><span>{np}</span><span>{nc}</span><span>{pl}</span><span>{g:.0} g de filamento</span></div>\
<p class=\"lead\">Abre <a class=\"mono\" href=\"{three_mf}\">{three_mf}</a> en Bambu Studio e imprime cada placa con su filamento; luego sigue la <a href=\"guia-de-armado.html\">guía de armado</a>.</p>",
        name = escape(info.name),
        np = count(kit.pieces.len(), "pieza", "piezas"),
        nc = count(n.slots().len(), "color", "colores"),
        pl = count(kit.plates.len(), "placa", "placas"),
        g = n.grams(total),
        three_mf = escape(&n.three_mf),
    );

    let review = n.review();
    let notes = n.notes();
    h.push_str("<h2>Qué revisar</h2>");
    if review.is_empty() && notes.is_empty() {
        h.push_str("<p>Nada: todas las piezas están cerradas, caben en la placa y no son demasiado finas para la boquilla.</p>");
    }
    if !review.is_empty() {
        h.push_str("<table><thead><tr><th>Pieza</th><th>Color</th><th>Motivo</th></tr></thead><tbody>");
        for (i, issues) in &review {
            let why: Vec<String> = issues.iter().map(|x| escape(&issue_text(x))).collect();
            let _ = write!(
                h,
                "<tr><td><a class=\"mono\" href=\"guia-de-armado.html#{l}\">{l}</a></td><td>{c}</td><td>{w}</td></tr>",
                l = labels[*i],
                c = n.piece_colour(*i),
                w = why.join("; ")
            );
        }
        h.push_str("</tbody></table>");
    }
    for note in &notes {
        let _ = write!(h, "<p class=\"warn\">{}</p>", escape(note));
    }

    h.push_str("<h2>Colores</h2><table><thead><tr><th>Ranura</th><th>Filamento</th><th>Piezas</th><th>Placas</th><th>Volumen</th><th>Peso</th></tr></thead><tbody>");
    for (s, pieces, plates, mm3) in colour_rows(n) {
        let hex = n.hex(s);
        let _ = write!(
            h,
            "<tr><td>{f}</td><td><span class=\"sw sw--s\" style=\"background:{hex}\"></span>{name} <span class=\"mono\">{hex}</span></td><td>{pieces}</td><td>{plates}</td><td>{cm3:.1} cm³</td><td>{g:.0} g</td></tr>",
            f = s + 1,
            name = escape(&n.filament(s)),
            cm3 = mm3 / 1000.0,
            g = n.grams(mm3),
        );
    }
    h.push_str("</tbody></table><p><small>Volumen y peso de las piezas, sin soportes, relleno ni purga.</small></p>");

    h.push_str("<h2>Placas</h2><table><thead><tr><th>Placa</th><th>Ocupa</th><th>Piezas</th></tr></thead><tbody>");
    for (k, plate) in kit.plates.iter().enumerate() {
        let list: Vec<&str> = plate.pieces.iter().map(|&i| labels[i].as_str()).collect();
        let _ = write!(
            h,
            "<tr><td><a href=\"guia-de-armado.html#placa-{}\">{}</a></td><td>{:.0} %</td><td class=\"mono\">{}</td></tr>",
            k + 1,
            escape(&n.plate_name(k)),
            plate.occupancy * 100.0,
            list.join(" ")
        );
    }
    h.push_str("</tbody></table>");

    h.push_str("<h2>Cómo se cortó</h2><table><tbody>");
    for (k, v) in params_text(n) {
        let _ = write!(h, "<tr><th>{}</th><td>{}</td></tr>", escape(&k), escape(&v));
    }
    h.push_str("</tbody></table>");

    let _ = write!(
        h,
        "<h2>Contenido</h2><ul class=\"files\">\
<li><a href=\"guia-de-armado.html\">guia-de-armado.html</a>: imágenes de cada pieza, placa y paso.</li>\
<li><a href=\"guia-de-armado.txt\">guia-de-armado.txt</a>: los mismos pasos en texto.</li>\
<li><a class=\"mono\" href=\"{t}\">{t}</a>: todas las piezas en sus placas, con su filamento.</li>\
<li><span class=\"mono\">stl/</span>: un STL por pieza, en una carpeta por color, con el nombre de la pieza.</li>\
<li><span class=\"mono\">img/</span>: las imágenes de la guía.</li></ul></main></body></html>",
        t = escape(&n.three_mf)
    );
    h.push('\n');
    h
}

fn readme_text(n: &Names) -> String {
    let (kit, info, labels) = (n.kit, n.info, &n.labels);
    let total: f32 = kit.pieces.iter().map(|p| p.volume_mm3).sum();
    let mut t = format!(
        "Kit por colores · {}\n\n{}, {}, {}, {:.0} g de filamento.\n\n\
1. Abre {} en Bambu Studio e imprime cada placa con su filamento.\n\
2. Separa las piezas por número y arma el modelo con guia-de-armado.html (o guia-de-armado.txt).\n\
   resumen.html tiene todo esto con enlaces.\n\n",
        info.name,
        count(kit.pieces.len(), "pieza", "piezas"),
        count(n.slots().len(), "color", "colores"),
        count(kit.plates.len(), "placa", "placas"),
        n.grams(total),
        n.three_mf
    );
    let review = n.review();
    let notes = n.notes();
    t.push_str("QUÉ REVISAR\n");
    if review.is_empty() && notes.is_empty() {
        t.push_str("  Nada: todas las piezas están cerradas, caben en la placa y no son demasiado finas.\n");
    }
    for (i, issues) in &review {
        let why: Vec<String> = issues.iter().map(issue_text).collect();
        let _ = writeln!(t, "  {} ({}): {}", labels[*i], n.piece_colour(*i), why.join("; "));
    }
    for note in &notes {
        let _ = writeln!(t, "  {note}");
    }
    t.push_str("\nCOLORES\n");
    for (s, pieces, plates, mm3) in colour_rows(n) {
        let _ = writeln!(
            t,
            "  Ranura {:>2}  {} {:<30} {:>10}  {:>8}  {:>7.1} cm³  {:>5.0} g",
            s + 1,
            n.hex(s),
            n.filament(s),
            count(pieces, "pieza", "piezas"),
            count(plates, "placa", "placas"),
            mm3 / 1000.0,
            n.grams(mm3)
        );
    }
    t.push_str("\nCÓMO SE CORTÓ\n");
    for (k, v) in params_text(n) {
        let _ = writeln!(t, "  {k}: {v}");
    }
    t.push_str(
        "\nCONTENIDO\n  resumen.html          este resumen, con enlaces\n  guia-de-armado.html   imágenes de cada pieza, placa y paso\n  guia-de-armado.txt    los mismos pasos en texto\n",
    );
    let _ = writeln!(t, "  {:<21} todas las piezas en sus placas", n.three_mf);
    t.push_str("  stl/                  un STL por pieza, en una carpeta por color\n  img/                  las imágenes de la guía\n");
    t
}

pub fn write_kit_zip<W: Write + Seek>(out: W, kit: &Kit, info: &KitInfo) -> zip::result::ZipResult<KitSummary> {
    let n = Names::new(kit, info);
    let colours: Vec<[u8; 3]> = kit.pieces.iter().map(|p| rgb(&n.hex(p.slot))).collect();
    let pics = pictures(kit, &colours)?;
    let paths = Paths::new(kit, &pics, &n.labels);

    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    z.start_file("LEEME.txt", opts)?;
    z.write_all(readme_text(&n).as_bytes())?;
    z.start_file("resumen.html", opts)?;
    z.write_all(summary_html(&n).as_bytes())?;
    z.start_file("guia-de-armado.html", opts)?;
    z.write_all(guide_html(&n, &pics, &paths).as_bytes())?;
    z.start_file("guia-de-armado.txt", opts)?;
    z.write_all(guide_text(&n).as_bytes())?;

    let objects: Vec<PlateObject> = (0..kit.pieces.len())
        .map(|i| PlateObject {
            name: &n.labels[i],
            positions: &kit.placed[i].positions,
            tris: &kit.pieces[i].tris,
            filament: kit.pieces[i].slot as usize + 1,
            plate: kit.placed[i].plate,
            at: kit.placed[i].at,
        })
        .collect();
    let project = PlateProject {
        name: info.name,
        flush: info.flush.clone(),
        objects,
        plates: (0..kit.plates.len()).map(|k| n.plate_name(k)).collect(),
        filaments: info.palette.to_vec(),
        slots: info.slots.to_vec(),
        printer: info.printer.clone(),
        thumbnails: pics.covers.clone(),
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    write_plates(&mut buf, &project)?;
    z.start_file(&n.three_mf, stored)?;
    z.write_all(&buf.into_inner())?;

    for (i, file) in n.files.iter().enumerate() {
        let mut stl = Vec::new();
        write_stl(&mut stl, &n.labels[i], &kit.placed[i].positions, &kit.pieces[i].tris)?;
        z.start_file(file, opts)?;
        z.write_all(&stl)?;
    }
    // PNG is compressed already.
    for (path, png) in paths.files(&pics) {
        z.start_file(path, stored)?;
        z.write_all(png)?;
    }
    z.finish()?;
    Ok(KitSummary {
        pieces: kit.pieces.len(),
        plates: kit.plates.len(),
        oversize: kit.placed.iter().filter(|p| !p.fits).count(),
        review: n.review().len(),
        merged: kit.merged,
        dropped: kit.dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_core::split::{Piece, Split};
    use std::io::{Cursor, Read};

    #[test]
    fn the_printers_plate_keeps_its_excluded_zones_and_tower_free() {
        let x1c = crate::catalog().into_iter().find(|p| p.id == "Bambu Lab X1 Carbon").unwrap();
        let printer = Printer::new(x1c, 0.4);
        let reserve = Reserve { tower_mm: [40.0, 40.0], ..Default::default() };
        let pieces = || -> Vec<Piece> {
            (0..40)
                .map(|i| {
                    let (positions, tris) = cuboid([0.0; 3], [28.0 + (i % 5) as f32 * 6.0, 22.0, 6.0]);
                    Piece { slot: (i % 2) as u8, positions, tris, volume_mm3: 1.0, contacts: vec![] }
                })
                .collect()
        };
        for mix in [false, true] {
            let bed = kit_bed(&printer, &reserve, 4.0, mix);
            assert!(!bed.excluded.is_empty() && bed.tower.is_some());
            let kit = bp_core::kit::kit(Split { pieces: pieces(), voxel_mm: 0.5, merged: 0, dropped: 0 }, bed.clone());
            let tower = bed.tower.filter(|_| mix);
            for (i, p) in kit.placed.iter().enumerate() {
                let r = crate::plate::Rect::centred(p.at[0], p.at[1], p.size[0], p.size[1]);
                assert!(printer.printable_area().grow(-reserve.margin_mm).contains(&r), "P{i} in the margin");
                for o in bed.excluded.iter().chain(&tower) {
                    assert!(!r.overlaps(&crate::plate::Rect::new(o[0], o[1], o[2], o[3])), "P{i} on {o:?}, mix {mix}");
                }
            }
            assert_eq!(kit.plates.iter().any(|p| p.mixed()), mix);
        }
    }

    fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let p = (0..8).map(|b| [[lo[0], hi[0]][b & 1], [lo[1], hi[1]][(b >> 1) & 1], [lo[2], hi[2]][b >> 2]]).collect();
        let t = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        (p, t)
    }

    #[test]
    fn colour_names() {
        assert_eq!(color_name([0x20, 0x20, 0x20]), "Negro");
        assert_eq!(color_name([0xF5, 0xF4, 0xF0]), "Blanco");
        assert_eq!(color_name([0xE9, 0xA4, 0x62]), "Caramelo");
        assert_eq!(color_name([0x4A, 0x34, 0x2E]), "Marrón oscuro");
        assert_eq!(color_name([0xD0, 0x20, 0x20]), "Rojo");
        assert_eq!(color_name([0x20, 0x50, 0xD0]), "Azul");
        assert_eq!(slug("Marrón oscuro"), "marron-oscuro");
    }

    /// Every `href` and `src` of a page that points inside the ZIP.
    fn links(html: &str) -> Vec<String> {
        let mut out = vec![];
        for key in ["href=\"", "src=\""] {
            let mut rest = html;
            while let Some(i) = rest.find(key) {
                rest = &rest[i + key.len()..];
                let end = rest.find('"').unwrap();
                let link = rest[..end].split('#').next().unwrap();
                if !link.is_empty() && !link.contains(':') {
                    out.push(link.to_string());
                }
                rest = &rest[end..];
            }
        }
        out
    }

    fn read(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
        let mut out = vec![];
        zip.by_name(name).unwrap_or_else(|_| panic!("{name} is missing")).read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn zip_holds_plates_stls_and_guide() {
        let (pa, ta) = cuboid([0.0; 3], [20.0, 20.0, 10.0]);
        let (pb, tb) = cuboid([0.0, 0.0, 10.0], [20.0, 20.0, 14.0]);
        // A sliver: thinner than two lines of a 0.4 mm nozzle.
        let (pc, tc) = cuboid([20.0, 0.0, 0.0], [20.5, 20.0, 14.0]);
        let pieces = vec![
            Piece { slot: 0, positions: pa, tris: ta, volume_mm3: 4000.0, contacts: vec![(1, 400.0), (2, 280.0)] },
            Piece { slot: 1, positions: pb, tris: tb, volume_mm3: 1600.0, contacts: vec![(0, 400.0)] },
            Piece { slot: 1, positions: pc, tris: tc, volume_mm3: 140.0, contacts: vec![(0, 280.0)] },
        ];
        let kit = bp_core::kit::kit(Split { pieces, voxel_mm: 0.5, merged: 3, dropped: 1 }, Bed::plain(256.0, 256.0));
        let palette = vec!["#202020".to_string(), "#F5F4F0".to_string()];
        let names = vec![None, Some("Bambu Lab PLA Basic Blanco".to_string())];
        let info = KitInfo {
            name: "Araña",
            palette: &palette,
            slots: &[],
            filament_names: &names,
            printer: &Printer::default(),
            params: KitSettings { voxel_mm: 0.5, depth_mm: 0.0, core_slot: None, min_piece_mm3: 30.0, mix: false },
            density_g_cm3: 1.24,
            flush: Default::default(),
        };
        let mut buf = Cursor::new(Vec::new());
        let summary = write_kit_zip(&mut buf, &kit, &info).unwrap();
        assert_eq!(summary, KitSummary { pieces: 3, plates: 2, oversize: 0, review: 1, merged: 3, dropped: 1 });
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        for f in ["LEEME.txt", "resumen.html", "guia-de-armado.html", "guia-de-armado.txt", "3mf/arana_placas.3mf"] {
            assert!(names.contains(&f.to_string()), "{f} missing: {names:?}");
        }
        assert!(names.contains(&"stl/1-negro/P01.stl".to_string()), "{names:?}");
        assert!(names.contains(&"stl/2-blanco/P02.stl".to_string()), "{names:?}");

        // Every page finds everything it points at, offline.
        for page in ["guia-de-armado.html", "resumen.html"] {
            let html = String::from_utf8(read(&mut zip, page)).unwrap();
            for link in links(&html) {
                assert!(names.contains(&link), "{page} points at {link}, which is not in the ZIP");
            }
        }

        // A piece has the same name in its STL, the 3MF and the guide.
        let mut three_mf = zip::ZipArchive::new(Cursor::new(read(&mut zip, "3mf/arana_placas.3mf"))).unwrap();
        let mut cfg = String::new();
        three_mf.by_name("Metadata/model_settings.config").unwrap().read_to_string(&mut cfg).unwrap();
        let guide = String::from_utf8(read(&mut zip, "guia-de-armado.html")).unwrap();
        for (i, label) in ["P01", "P02", "P03"].iter().enumerate() {
            let stl_name = names.iter().find(|n| n.ends_with(&format!("/{label}.stl"))).unwrap().clone();
            let stl = read(&mut zip, &stl_name);
            assert!(stl.starts_with(label.as_bytes()), "STL header of {label}");
            assert!(cfg.contains(&format!("<metadata key=\"name\" value=\"{label}\"/>")), "3MF object {label}");
            assert!(guide.contains(&format!("id=\"{label}\"")), "guide card {label}");
            assert!(guide.contains(&stl_name), "guide links {stl_name} ({i})");
        }
        assert!(cfg.contains("Placa 2 · Bambu Lab PLA Basic Blanco #F5F4F0"), "{cfg}");

        // The sliver is reported, with why, and so is what the split merged and dropped.
        let summary_html = String::from_utf8(read(&mut zip, "resumen.html")).unwrap();
        assert!(summary_html.contains("guia-de-armado.html#P03") && summary_html.contains("menos de dos líneas"));
        assert!(
            summary_html.contains("3 migas")
                && summary_html.contains("1 zona demasiado fina para formar una pieza se dejó")
        );
        let readme = String::from_utf8(read(&mut zip, "LEEME.txt")).unwrap();
        assert!(readme.contains("P03 (Blanco)") && readme.contains("3 piezas, 2 colores, 2 placas"), "{readme}");
        // 5,74 cm³ of PLA weigh about 7 g.
        assert!(readme.contains(" 7 g de filamento"), "{readme}");
        assert!(guide.contains("Paso 2 de 3") && guide.contains("Pega P02 a P01"));
        let txt = String::from_utf8(read(&mut zip, "guia-de-armado.txt")).unwrap();
        assert!(txt.contains("Placa 2") && txt.contains("! su lado más fino mide 0.5 mm"), "{txt}");
    }
}
