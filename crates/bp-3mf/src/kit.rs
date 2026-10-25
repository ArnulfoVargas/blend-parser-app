//! The colour kit as a ZIP: a 3MF with every piece laid out on plates of a
//! single colour, one STL per piece, and an assembly guide (an HTML page with
//! pictures of every step, and the same steps as plain text).

use std::fmt::Write as _;
use std::io::{Seek, Write};

use bp_core::kit::Kit;
use bp_core::render::{self, Camera, Frame, Item, Style};
use rayon::prelude::*;
use zip::write::SimpleFileOptions;

use crate::{escape, write_plates, write_stl, PlateObject, PlateProject, Printer};

pub struct KitInfo<'a> {
    /// Model name, used for file names and titles.
    pub name: &'a str,
    /// `#RRGGBB` of every filament slot.
    pub palette: &'a [String],
    pub printer: Printer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitSummary {
    pub pieces: usize,
    pub plates: usize,
    /// Pieces larger than the plate.
    pub oversize: usize,
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

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

    fn html(&self, alt: &str, label: &dyn Fn(usize) -> String) -> String {
        let mut s = format!(
            "<figure class=\"shot\"><img width=\"{}\" height=\"{}\" alt=\"{}\" src=\"data:image/png;base64,{}\">",
            self.width,
            self.height,
            escape(alt),
            base64(&self.png)
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
                a[k] = (a[k] - 6.0).max(0.0);
                b[k] = (b[k] + 6.0).min([kit.bed.0, kit.bed.1][k]);
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
    Ok(Pictures { overview, thumbs, plates, steps, details })
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
.step{display:grid;grid-template-columns:minmax(0,3fr) minmax(220px,2fr);gap:20px;align-items:center;padding:20px 0;border-top:1px solid var(--rule)}
.step .n{font:600 .8rem "DM Mono",ui-monospace,monospace;color:var(--accent);text-transform:uppercase;letter-spacing:.06em}
.step h3{font-size:1.25rem;margin:4px 0 8px}
@media (max-width:720px){.step{grid-template-columns:1fr}}
@media print{body{background:#fff}main{padding:0}.step{break-inside:avoid}h2{break-after:avoid}}
"#;

fn guide_html(
    kit: &Kit,
    info: &KitInfo,
    pics: &Pictures,
    labels: &[String],
    files: &[String],
    three_mf: &str,
) -> String {
    let slot_hex = |s: u8| info.palette.get(s as usize).cloned().unwrap_or_else(|| "#808080".into());
    let colour_of = |i: usize| {
        let hex = slot_hex(kit.pieces[i].slot);
        (color_name(rgb(&hex)), hex)
    };
    let label = |i: usize| labels[i].clone();
    let swatch = |hex: &str| format!("<span class=\"sw sw--s\" style=\"background:{hex}\"></span>");
    let (lo, hi) = bounds(kit.pieces.iter().flat_map(|p| p.positions.iter()));
    let mut slots: Vec<u8> = kit.pieces.iter().map(|p| p.slot).collect();
    slots.sort_unstable();
    slots.dedup();

    let mut h = String::with_capacity(1 << 20);
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"es\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Guía de armado · {name}</title><style>{CSS}</style></head><body><main>\
<h1>Guía de armado · {name}</h1>\
<p class=\"lead\">El modelo dividido en piezas sólidas, una o varias por color. Imprime cada placa con su filamento y pega las piezas en el orden de los pasos.</p>\
<div class=\"meta\"><span>{n} piezas</span><span>{c} colores</span><span>{p} placas</span><span>{x:.0} × {y:.0} × {z:.0} mm armado</span><span>Voxel {v:.2} mm</span></div>\
<div class=\"pair\">{o1}{o2}</div>",
        name = escape(info.name),
        n = kit.pieces.len(),
        c = slots.len(),
        p = kit.plates.len(),
        x = hi[0] - lo[0],
        y = hi[1] - lo[1],
        z = hi[2] - lo[2],
        v = kit.voxel_mm,
        o1 = pics.overview[0].html("Modelo armado, vista frontal", &label),
        o2 = pics.overview[1].html("Modelo armado, vista trasera", &label),
    );

    let _ = write!(
        h,
        "<h2>Antes de empezar</h2><ol class=\"prep\">\
<li>Abre <span class=\"mono\">{three_mf}</span> en Bambu Studio. Cada placa lleva un solo color: imprímela con el filamento que indica su nombre.</li>\
<li>Las piezas ya están giradas sobre su cara más plana. Si Bambu avisa de voladizos, activa soportes en árbol solo sobre la placa.</li>\
<li>Al despegar cada placa, separa las piezas por número usando la sección <b>Piezas</b>: cada una aparece con su forma y su color. Los STL de la carpeta <span class=\"mono\">stl/</span> llevan el mismo número.</li>\
<li>Prueba cada unión en seco antes de pegar. Lija un poco las caras de contacto para que el pegamento agarre.</li>\
<li>Para PLA usa cianoacrilato en gel; para uniones grandes, epoxi de 5 minutos. Sujeta unos segundos y sigue con el siguiente paso.</li>\
</ol>"
    );

    h.push_str("<h2>Filamentos</h2><div class=\"legend\">");
    for &s in &slots {
        let hex = slot_hex(s);
        let count = kit.pieces.iter().filter(|p| p.slot == s).count();
        let plates: Vec<String> =
            kit.plates.iter().enumerate().filter(|(_, p)| p.slot == s).map(|(k, _)| (k + 1).to_string()).collect();
        let _ = write!(
            h,
            "<div class=\"chip\"><span class=\"sw\" style=\"background:{hex}\"></span><div><b>{name}</b> <span class=\"mono\">{hex}</span>\
<small>Filamento {f} · {count} piezas · placa{pl} {list}</small></div></div>",
            name = color_name(rgb(&hex)),
            f = s + 1,
            pl = if plates.len() > 1 { "s" } else { "" },
            list = plates.join(", ")
        );
    }
    h.push_str("</div>");

    h.push_str("<h2>Placas</h2><div class=\"plates\">");
    for (k, plate) in kit.plates.iter().enumerate() {
        let hex = slot_hex(plate.slot);
        let _ = write!(
            h,
            "<div class=\"plate\">{shot}<h3>Placa {n} · {sw}{name}</h3><small>{count} piezas: {list}</small></div>",
            shot = pics.plates[k].html(&format!("Placa {}", k + 1), &label),
            n = k + 1,
            sw = swatch(&hex),
            name = color_name(rgb(&hex)),
            count = plate.pieces.len(),
            list = plate.pieces.iter().map(|&i| labels[i].as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    h.push_str("</div>");

    h.push_str("<h2>Piezas</h2><div class=\"grid\">");
    for (i, p) in kit.pieces.iter().enumerate() {
        let (name, hex) = colour_of(i);
        let s = kit.placed[i].size;
        let _ = write!(
            h,
            "<div class=\"card\">{shot}<div><b>{label}</b> · {sw}{name}<small>{x:.0} × {y:.0} × {z:.0} mm · {vol:.1} cm³</small>\
<small>Placa {plate} · <span class=\"mono\">{file}</span></small>{warn}</div></div>",
            shot = pics.thumbs[i].html(&labels[i], &label),
            label = labels[i],
            sw = swatch(&hex),
            x = s[0],
            y = s[1],
            z = s[2],
            vol = p.volume_mm3 / 1000.0,
            plate = kit.placed[i].plate + 1,
            file = escape(&files[i]),
            warn = if kit.placed[i].fits { "" } else { "<small class=\"warn\">Más grande que la placa: escálala o córtala en Bambu Studio.</small>" }
        );
    }
    h.push_str("</div>");

    let _ = write!(h, "<h2>Armado</h2><p class=\"lead\">{} pasos. En cada imagen la pieza nueva aparece en su color con un contorno grueso; lo ya armado, en tonos suaves.</p>", kit.steps.len());
    for k in 0..kit.steps.len() {
        let (title, body) = step_text(kit, k, labels, &|i| colour_of(i).0);
        let _ = write!(
            h,
            "<section class=\"step\">{shot}<div><div class=\"n\">Paso {n} de {total}</div><h3>{title}</h3><p>{body}</p>{detail}</div></section>",
            shot = pics.steps[k].html(&format!("Paso {}", k + 1), &label),
            detail = pics.details[k]
                .as_ref()
                .map(|d| format!("<div class=\"detail\">{}<small>Detalle</small></div>", d.html(&format!("Detalle del paso {}", k + 1), &label)))
                .unwrap_or_default(),
            n = k + 1,
            total = kit.steps.len(),
        );
    }
    h.push_str("<p class=\"lead\" style=\"margin-top:40px\">Listo: el modelo está completo. Deja curar el pegamento antes de manipularlo.</p></main></body></html>\n");
    h
}

/// Title and explanation of step `k`, shared by the HTML and the text guide.
fn step_text(kit: &Kit, k: usize, labels: &[String], colour: &dyn Fn(usize) -> &'static str) -> (String, String) {
    let step = &kit.steps[k];
    let p = step.piece;
    let title = format!("{} · {}", labels[p], colour(p));
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

fn guide_text(kit: &Kit, info: &KitInfo, labels: &[String], files: &[String], three_mf: &str) -> String {
    let colour = |i: usize| {
        let hex = info.palette.get(kit.pieces[i].slot as usize).cloned().unwrap_or_default();
        color_name(rgb(&hex))
    };
    let mut t = format!(
        "Guía de armado · {}\n\n{} piezas en {} placas. Abre {three_mf} en Bambu Studio e imprime cada placa con su filamento.\n\
La guía con imágenes está en guia-de-armado.html.\n\nPLACAS\n",
        info.name,
        kit.pieces.len(),
        kit.plates.len()
    );
    for (k, plate) in kit.plates.iter().enumerate() {
        let hex = info.palette.get(plate.slot as usize).cloned().unwrap_or_default();
        let _ = writeln!(
            t,
            "  Placa {:>2}  filamento {} {} {:<14} {}",
            k + 1,
            plate.slot + 1,
            hex,
            color_name(rgb(&hex)),
            plate.pieces.iter().map(|&i| labels[i].as_str()).collect::<Vec<_>>().join(" ")
        );
    }
    t.push_str("\nPIEZAS\n");
    for (i, p) in kit.pieces.iter().enumerate() {
        let s = kit.placed[i].size;
        let _ = writeln!(
            t,
            "  {}  {:<14} {:>5.0} × {:>4.0} × {:>4.0} mm  {:>7.1} cm³  placa {:>2}  {}{}",
            labels[i],
            colour(i),
            s[0],
            s[1],
            s[2],
            p.volume_mm3 / 1000.0,
            kit.placed[i].plate + 1,
            files[i],
            if kit.placed[i].fits { "" } else { "  (más grande que la placa)" }
        );
    }
    t.push_str("\nARMADO\n");
    for k in 0..kit.steps.len() {
        let (title, body) = step_text(kit, k, labels, &colour);
        let _ = writeln!(t, "  {:>3}. {title}\n       {body}", k + 1);
    }
    t
}

pub fn write_kit_zip<W: Write + Seek>(out: W, kit: &Kit, info: &KitInfo) -> zip::result::ZipResult<KitSummary> {
    let digits = kit.pieces.len().to_string().len().max(2);
    let labels: Vec<String> = (0..kit.pieces.len()).map(|i| format!("P{:0digits$}", i + 1)).collect();
    let hex_of = |i: usize| info.palette.get(kit.pieces[i].slot as usize).cloned().unwrap_or_else(|| "#808080".into());
    let colours: Vec<[u8; 3]> = (0..kit.pieces.len()).map(|i| rgb(&hex_of(i))).collect();
    let files: Vec<String> = (0..kit.pieces.len())
        .map(|i| {
            let hex = hex_of(i);
            format!("stl/{}_{}_{}.stl", labels[i], slug(color_name(rgb(&hex))), hex.trim_start_matches('#'))
        })
        .collect();
    let base = slug(info.name);
    let base = if base.is_empty() { "modelo".to_string() } else { base };
    let three_mf = format!("{base}_piezas.3mf");

    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let plate_names: Vec<String> = kit
        .plates
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let hex = info.palette.get(p.slot as usize).cloned().unwrap_or_default();
            format!("{} · {} {}", k + 1, color_name(rgb(&hex)), hex)
        })
        .collect();
    let objects: Vec<PlateObject> = (0..kit.pieces.len())
        .map(|i| PlateObject {
            name: &labels[i],
            positions: &kit.placed[i].positions,
            tris: &kit.pieces[i].tris,
            filament: kit.pieces[i].slot as usize + 1,
            plate: kit.placed[i].plate,
            at: kit.placed[i].at,
        })
        .collect();
    let project = PlateProject {
        name: info.name,
        objects,
        plates: plate_names,
        filaments: info.palette.to_vec(),
        printer: info.printer,
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    write_plates(&mut buf, &project)?;
    z.start_file(&three_mf, SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored))?;
    z.write_all(&buf.into_inner())?;

    for (i, file) in files.iter().enumerate() {
        let mut stl = Vec::new();
        write_stl(&mut stl, &labels[i], &kit.placed[i].positions, &kit.pieces[i].tris)?;
        z.start_file(file, opts)?;
        z.write_all(&stl)?;
    }

    let pics = pictures(kit, &colours)?;
    z.start_file("guia-de-armado.html", opts)?;
    z.write_all(guide_html(kit, info, &pics, &labels, &files, &three_mf).as_bytes())?;
    z.start_file("instrucciones.txt", opts)?;
    z.write_all(guide_text(kit, info, &labels, &files, &three_mf).as_bytes())?;
    z.finish()?;
    Ok(KitSummary {
        pieces: kit.pieces.len(),
        plates: kit.plates.len(),
        oversize: kit.placed.iter().filter(|p| !p.fits).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_core::split::{Piece, Split};
    use std::io::{Cursor, Read};

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
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }

    #[test]
    fn zip_holds_plates_stls_and_guide() {
        let (pa, ta) = cuboid([0.0; 3], [20.0, 20.0, 10.0]);
        let (pb, tb) = cuboid([0.0, 0.0, 10.0], [20.0, 20.0, 14.0]);
        let pieces = vec![
            Piece { slot: 0, positions: pa, tris: ta, volume_mm3: 4000.0, contacts: vec![(1, 400.0)] },
            Piece { slot: 1, positions: pb, tris: tb, volume_mm3: 1600.0, contacts: vec![(0, 400.0)] },
        ];
        let kit = bp_core::kit::kit(Split { pieces, voxel_mm: 0.5 }, (256.0, 256.0));
        let palette = vec!["#202020".to_string(), "#F5F4F0".to_string()];
        let info = KitInfo { name: "Araña", palette: &palette, printer: Printer::A1 };
        let mut buf = Cursor::new(Vec::new());
        let summary = write_kit_zip(&mut buf, &kit, &info).unwrap();
        assert_eq!(summary, KitSummary { pieces: 2, plates: 2, oversize: 0 });
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        assert!(names.contains(&"arana_piezas.3mf".to_string()), "{names:?}");
        assert!(names.contains(&"stl/P01_negro_202020.stl".to_string()), "{names:?}");
        assert!(names.contains(&"stl/P02_blanco_F5F4F0.stl".to_string()));
        let mut html = String::new();
        zip.by_name("guia-de-armado.html").unwrap().read_to_string(&mut html).unwrap();
        assert!(html.contains("Paso 2 de 2") && html.contains("Pega P02 a P01"));
        assert!(html.matches("data:image/png;base64,").count() >= 2 + 2 + 2 + 2);
        let mut txt = String::new();
        zip.by_name("instrucciones.txt").unwrap().read_to_string(&mut txt).unwrap();
        assert!(txt.contains("Placa  2"));
    }
}
