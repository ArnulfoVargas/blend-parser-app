//! The colour kit as a ZIP, in Spanish or English (names in brackets):
//!
//! ```text
//! LEEME.txt (README.txt)               what is in the kit, what to check, how to start
//! resumen.html (summary.html)          the same, with colours, filaments and links
//! guia-de-armado.html (assembly-guide) pictures of every piece, plate and step
//! guia-de-armado.txt                   the same steps as plain text
//! 3mf/<modelo>_placas.3mf              every piece on its plate
//! stl/<n>-<color>/P07.stl              one STL per piece, by filament
//! img/…                                the guide's pictures (it works offline)
//! ```
//!
//! A piece is called by its label (`P07`) everywhere: its STL file and header,
//! its 3MF object, and the guide. Large kits are built in sub-assemblies (A,
//! B…) joined at the end; each step shows the new piece pulled out and the
//! glue striped where it goes, and the guide remembers ticked steps.

use std::fmt::Write as _;
use std::io::{Seek, Write};

use bp_core::kit::{Bed, Issue, Kit};
use bp_core::render::{self, Camera, Frame, Item, Style};
use rayon::prelude::*;
use zip::write::SimpleFileOptions;

use crate::plate::Reserve;
use crate::project_file::{KitSettings, Quality, StlFiles};
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
    /// The language of the guide, the summary and the file names.
    pub lang: Lang,
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
    /// Pins keying pieces together, and joins with no room for one.
    pub pins: usize,
    pub unpinned: usize,
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
    tags: Vec<(String, [f32; 2], bool)>,
    /// A ring around what is being added (left, top, right, bottom); its label
    /// then sits on the ring instead of over it.
    ring: Option<[f32; 4]>,
}

impl Shot {
    fn new(frame: &Frame, tags: Vec<(String, [f32; 2], bool)>) -> std::io::Result<Self> {
        Ok(Self { png: png(frame)?, width: frame.width, height: frame.height, tags, ring: None })
    }

    /// Rings pieces `ids` and labels the ring `label`.
    fn focus(mut self, frame: &Frame, ids: &[usize], label: &str) -> Self {
        let boxes: Vec<[f32; 4]> = ids.iter().filter_map(|&i| frame.bbox(i as u32)).collect();
        if boxes.is_empty() {
            return self;
        }
        let b = boxes.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, b| {
            [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
        });
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let (rw, rh) = ((b[2] - b[0]) / 2.0 + 0.03, (b[3] - b[1]) / 2.0 + 0.04);
        let (rw, rh) = (rw.max(0.05), rh.max(0.07));
        self.ring = Some([cx - rw, cy - rh, cx + rw, cy + rh]);
        self.tags.retain(|t| t.0 != label);
        // On the ring's upper right, at 45°.
        self.tags.push((label.to_string(), [cx + rw * 0.707, cy - rh * 0.707], true));
        self
    }

    /// The picture, saved at `src` next to the page, with its labels and ring.
    fn html(&self, src: &str, alt: &str) -> String {
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
        for (label, at, strong) in &self.tags {
            let _ = write!(
                s,
                "<span class=\"tag{}\" style=\"left:{:.1}%;top:{:.1}%\">{}</span>",
                if *strong { " tag--focus" } else { "" },
                at[0] * 100.0,
                at[1] * 100.0,
                escape(label)
            );
        }
        s.push_str("</figure>");
        s
    }
}

/// Drops labels that would cover one already placed (biggest pieces come first).
fn declutter(tags: impl Iterator<Item = (String, [f32; 2], bool)>) -> Vec<(String, [f32; 2], bool)> {
    let mut kept: Vec<(String, [f32; 2], bool)> = vec![];
    for t in tags {
        if kept.iter().all(|k| (k.1[0] - t.1[0]).abs() > 0.085 || (k.1[1] - t.1[1]).abs() > 0.06) {
            kept.push(t);
        }
    }
    kept
}

const VIEWS: [(f32, f32); 6] =
    [(35.0, 28.0), (125.0, 28.0), (215.0, 28.0), (305.0, 28.0), (35.0, -25.0), (215.0, -25.0)];

/// Id the view search gives glue, to count how much of it shows.
const GLUE_ID: u32 = u32::MAX - 1;

/// How large and smooth the guide's pictures are.
#[derive(Clone, Copy)]
struct Look {
    scale: f32,
    ssaa: usize,
}

impl Look {
    fn new(q: Quality) -> Self {
        let (scale, ssaa) = q.scale();
        Self { scale, ssaa }
    }

    fn px(&self, medium: usize) -> usize {
        ((medium as f32 * self.scale).round() as usize).max(16)
    }

    /// `items` seen from `view` (azimuth, elevation), framing `lo..hi`, `size` at medium quality.
    fn render(&self, view: (f32, f32), (lo, hi): ([f32; 3], [f32; 3]), size: (usize, usize), items: &[Item]) -> Frame {
        render::render(&Camera::new(view.0, view.1, lo, hi, self.px(size.0), self.px(size.1)), items, self.ssaa)
    }
}

/// Counts a picture done; stops the export when the user cancelled.
type Tick<'a> = &'a (dyn Fn() -> bool + Sync);

fn tick(t: Tick) -> std::io::Result<()> {
    if t() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"))
    }
}

struct Pictures {
    overview: Vec<Shot>,
    thumbs: Vec<Shot>,
    plates: Vec<Shot>,
    /// Each plate's picture for Bambu Studio's plate list: large and small PNG.
    covers: Vec<(Vec<u8>, Vec<u8>)>,
    steps: Vec<Shot>,
    /// A close-up for steps whose piece is small in the main picture.
    details: Vec<Option<Shot>>,
    /// Each sub-assembly after the first, glued to the ones before it.
    joins: Vec<Shot>,
}

fn bounds<'a>(points: impl Iterator<Item = &'a [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in points {
        lo = std::array::from_fn(|a| lo[a].min(p[a]));
        hi = std::array::from_fn(|a| hi[a].max(p[a]));
    }
    (lo, hi)
}

/// Gluing, as a picture: pieces in place (faded) with the glue striped on
/// them, and the pieces being added pulled away from it, ready to press on.
struct Gluing<'a> {
    kit: &'a Kit,
    colours: &'a [[u8; 3]],
    fixed: Vec<usize>,
    moving: Vec<usize>,
    /// On a piece in place, the triangles that get glue.
    glue: Vec<(usize, Vec<[u32; 3]>)>,
    /// The moving pieces where the picture shows them.
    moved: Vec<Vec<[f32; 3]>>,
    /// What the picture frames.
    lo: [f32; 3],
    hi: [f32; 3],
}

impl<'a> Gluing<'a> {
    /// `fixed` in place, `moving` glued to `pairs` (its piece, a fixed piece);
    /// framed on `frame` and wherever the moving pieces end up.
    fn new(
        kit: &'a Kit,
        colours: &'a [[u8; 3]],
        fixed: Vec<usize>,
        moving: Vec<usize>,
        pairs: &[(usize, usize)],
        frame: &[usize],
    ) -> Self {
        let tol = kit.voxel_mm * 0.5;
        let mut glue: Vec<(usize, Vec<[u32; 3]>)> = vec![];
        for &(m, f) in pairs {
            let faces = bp_core::kit::contact_faces(&kit.pieces[f], &kit.pieces[m], tol);
            let tris: Vec<[u32; 3]> = faces.iter().map(|&t| kit.pieces[f].tris[t]).collect();
            match glue.iter_mut().find(|g| g.0 == f) {
                Some(g) => g.1.extend(tris),
                None if !tris.is_empty() => glue.push((f, tris)),
                None => {}
            }
        }
        // Away from the glue, by a little more than half the moving part.
        let (mlo, mhi) = bounds(moving.iter().flat_map(|&i| kit.pieces[i].positions.iter()));
        let centre: [f32; 3] = std::array::from_fn(|k| (mlo[k] + mhi[k]) / 2.0);
        let (mut sum, mut area) = ([0.0f32; 3], 0.0f32);
        for (f, tris) in &glue {
            for t in tris {
                let c = t.map(|v| kit.pieces[*f].positions[v as usize]);
                let a = 0.5
                    * bp_core::geom::len(bp_core::geom::cross(
                        bp_core::geom::sub(c[1], c[0]),
                        bp_core::geom::sub(c[2], c[0]),
                    ));
                for k in 0..3 {
                    sum[k] += a * (c[0][k] + c[1][k] + c[2][k]) / 3.0;
                }
                area += a;
            }
        }
        let shift = if area > 0.0 && !fixed.is_empty() {
            let away = bp_core::geom::sub(centre, sum.map(|x| x / area));
            let dir = if bp_core::geom::len(away) > 1e-3 { bp_core::geom::normalize(away) } else { [0.0, 0.0, 1.0] };
            let extent = (0..3).map(|k| mhi[k] - mlo[k]).fold(0.0f32, f32::max);
            bp_core::geom::scale(dir, extent * 0.55 + 3.0)
        } else {
            [0.0; 3]
        };
        let moved: Vec<Vec<[f32; 3]>> = moving
            .iter()
            .map(|&i| kit.pieces[i].positions.iter().map(|&v| bp_core::geom::add(v, shift)).collect())
            .collect();
        let (flo, fhi) =
            bounds(frame.iter().flat_map(|&i| kit.pieces[i].positions.iter()).chain(moved.iter().flatten()));
        Self { kit, colours, fixed, moving, glue, moved, lo: flo, hi: fhi }
    }

    fn items(&self, scoring: bool) -> Vec<Item<'_>> {
        let p = &self.kit.pieces;
        let mut items: Vec<Item> = self
            .fixed
            .iter()
            .map(|&i| Item {
                positions: &p[i].positions,
                tris: &p[i].tris,
                rgb: self.colours[i],
                id: i as u32,
                style: Style::Faded,
            })
            .collect();
        items.extend(self.glue.iter().map(|(f, tris)| Item {
            positions: &p[*f].positions,
            tris,
            rgb: render::GLUE,
            id: if scoring { GLUE_ID } else { *f as u32 },
            style: Style::Glue,
        }));
        items.extend(self.moving.iter().zip(&self.moved).map(|(&i, m)| Item {
            positions: m,
            tris: &p[i].tris,
            rgb: self.colours[i],
            id: i as u32,
            style: Style::Focus,
        }));
        items
    }

    /// The view that shows most of the moving pieces and the glue.
    fn view(&self) -> (f32, f32) {
        let items = self.items(true);
        VIEWS
            .iter()
            .copied()
            .max_by_key(|&(az, el)| {
                let f = render::render(&Camera::new(az, el, self.lo, self.hi, 160, 120), &items, 1);
                self.moving.iter().map(|&i| f.count(i as u32)).sum::<usize>() + 2 * f.count(GLUE_ID)
            })
            .unwrap_or(VIEWS[0])
    }

    /// The picture, the fixed pieces glued to labelled, the moving ones ringed
    /// as `label`; and a close-up when they come out small.
    fn shots(&self, look: Look, labels: &[String], label: &str) -> std::io::Result<(Shot, Option<Shot>)> {
        let (az, el) = self.view();
        let items = self.items(false);
        let frame = look.render((az, el), (self.lo, self.hi), (640, 460), &items);
        let tags = declutter(
            self.glue.iter().filter_map(|(f, _)| frame.anchor(*f as u32).map(|a| (labels[*f].clone(), a, false))),
        );
        let shot = Shot::new(&frame, tags)?.focus(&frame, &self.moving, label);
        let boxes: Vec<[f32; 4]> = self.moving.iter().filter_map(|&i| frame.bbox(i as u32)).collect();
        let small = boxes.is_empty() || boxes.iter().all(|b| b[2] - b[0] < 0.3 && b[3] - b[1] < 0.3);
        let detail =
            if small {
                // The moving pieces and the glue, with room around them.
                let (a, b) =
                    bounds(self.moved.iter().flatten().chain(self.glue.iter().flat_map(|(f, t)| {
                        t.iter().flatten().map(move |&v| &self.kit.pieces[*f].positions[v as usize])
                    })));
                let c: [f32; 3] = std::array::from_fn(|k| (a[k] + b[k]) / 2.0);
                let r = (0..3).map(|k| b[k] - a[k]).fold(0.0f32, f32::max) * 0.6 + 4.0;
                let frame = look.render((az, el), (c.map(|x| x - r), c.map(|x| x + r)), (320, 240), &items);
                Some(Shot::new(&frame, vec![])?.focus(&frame, &self.moving, label))
            } else {
                None
            };
        Ok((shot, detail))
    }
}

/// Plate `k`'s pieces, at `moved`: where they print.
fn plate_items<'a>(kit: &'a Kit, colours: &[[u8; 3]], k: usize, moved: &'a [Vec<[f32; 3]>]) -> Vec<Item<'a>> {
    kit.plates[k]
        .pieces
        .iter()
        .zip(moved)
        .map(|(&i, m)| Item {
            positions: m,
            tris: &kit.pieces[i].tris,
            rgb: colours[i],
            id: i as u32,
            style: Style::Normal,
        })
        .collect()
}

/// How many pictures [`pictures`] draws, for the progress.
fn picture_count(kit: &Kit, covers: bool, guide: bool) -> usize {
    let covers = if covers { kit.plates.len() } else { 0 };
    covers + if guide { 2 + kit.pieces.len() + kit.plates.len() + kit.steps.len() + kit.groups.len() - 1 } else { 0 }
}

/// With `covers`, the plates' pictures for the 3MF; with `guide`, every picture of the guide.
#[allow(clippy::too_many_arguments)]
fn pictures(
    kit: &Kit,
    colours: &[[u8; 3]],
    labels: &[String],
    groups: &[String],
    look: Look,
    covers: bool,
    guide: bool,
    done: Tick,
) -> std::io::Result<Pictures> {
    let (lo, hi) = bounds(kit.pieces.iter().flat_map(|p| p.positions.iter()));
    let item = |i: usize, style: Style| Item {
        positions: &kit.pieces[i].positions,
        tris: &kit.pieces[i].tris,
        rgb: colours[i],
        id: i as u32,
        style,
    };

    let views: &[(f32, f32)] = if guide { &[(35.0, 28.0), (215.0, 28.0)] } else { &[] };
    let overview = views
        .par_iter()
        .map(|&(az, el)| {
            let items: Vec<Item> = (0..kit.pieces.len()).map(|i| item(i, Style::Normal)).collect();
            let shot = Shot::new(&look.render((az, el), (lo, hi), (560, 420), &items), vec![]);
            tick(done)?;
            shot
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let thumbs = (0..if guide { kit.pieces.len() } else { 0 })
        .into_par_iter()
        .map(|i| {
            let (a, b) = bounds(kit.pieces[i].positions.iter());
            let shot = Shot::new(&look.render((35.0, 28.0), (a, b), (200, 150), &[item(i, Style::Normal)]), vec![]);
            tick(done)?;
            shot
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    // Each plate's pieces where they print.
    let on_plate = |k: usize| -> Vec<Vec<[f32; 3]>> {
        kit.plates[k]
            .pieces
            .iter()
            .map(|&i| {
                let pl = &kit.placed[i];
                pl.positions.iter().map(|v| [v[0] + pl.at[0], v[1] + pl.at[1], v[2]]).collect()
            })
            .collect()
    };
    let covers = (0..if covers { kit.plates.len() } else { 0 })
        .into_par_iter()
        .map(|k| {
            let moved = on_plate(k);
            let items = plate_items(kit, colours, k, &moved);
            let (a, b) = bounds(moved.iter().flatten());
            let shot = |size: usize| png(&render::render(&Camera::new(35.0, 28.0, a, b, size, size), &items, 2));
            let both = (shot(512)?, shot(128)?);
            tick(done)?;
            Ok(both)
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let plates = (0..if guide { kit.plates.len() } else { 0 })
        .into_par_iter()
        .map(|k| {
            let moved = on_plate(k);
            let items = plate_items(kit, colours, k, &moved);
            // Framed on what is on the plate, not the whole bed, so small pieces stay legible.
            let (mut a, mut b) = bounds(moved.iter().flatten());
            for k in 0..2 {
                a[k] = (a[k] - 6.0).max(kit.bed.area[k]);
                b[k] = (b[k] + 6.0).min(kit.bed.area[k + 2]);
            }
            let frame = look.render((0.0, 89.0), ([a[0], a[1], 0.0], [b[0], b[1], b[2].max(1.0)]), (520, 360), &items);
            let ids = &kit.plates[k].pieces;
            let shot = Shot::new(
                &frame,
                declutter(ids.iter().filter_map(|&i| frame.anchor(i as u32).map(|a| (labels[i].clone(), a, false)))),
            );
            tick(done)?;
            shot
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    // A step shows its sub-assembly so far, framed on it and the new piece.
    let steps = (0..if guide { kit.steps.len() } else { 0 })
        .into_par_iter()
        .map(|k| {
            let step = &kit.steps[k];
            let range = kit.groups[step.group].steps.clone();
            let fixed: Vec<usize> = kit.steps[range.start..k].iter().map(|s| s.piece).collect();
            let pairs: Vec<(usize, usize)> = step.joins.iter().map(|&j| (step.piece, j)).collect();
            let frame = [fixed.as_slice(), &[step.piece]].concat();
            let shots = Gluing::new(kit, colours, fixed, vec![step.piece], &pairs, &frame).shots(
                look,
                labels,
                &labels[step.piece],
            );
            tick(done)?;
            shots
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let (steps, details) = steps.into_iter().unzip();

    // A sub-assembly comes to the groups before it, glued where they touch.
    let joins = (1..if guide { kit.groups.len() } else { 1 })
        .into_par_iter()
        .map(|g| {
            let before: Vec<usize> = kit.steps[..kit.groups[g].steps.start].iter().map(|s| s.piece).collect();
            let mine: Vec<usize> = kit.steps[kit.groups[g].steps.clone()].iter().map(|s| s.piece).collect();
            let all: Vec<usize> = (0..kit.pieces.len()).collect();
            let shot =
                Gluing::new(kit, colours, before, mine, &kit.groups[g].joins, &all).shots(look, labels, &groups[g])?.0;
            tick(done)?;
            Ok(shot)
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(Pictures { overview, thumbs, plates, covers, steps, details, joins })
}

const CSS: &str = r#"
:root{--paper:#f8f5ec;--paper-2:#eef4ee;--ink:#17231e;--ink-2:#2c3a33;--muted:#5f6f67;--rule:#d5dccd;--accent:#1f7a55;--accent-soft:#cfe9da;--card:#fffdf7;--glue:#d81b72}
*{box-sizing:border-box}
body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.5 "Instrument Sans",ui-sans-serif,system-ui,sans-serif}
main{max-width:1080px;margin:0 auto;padding:40px 16px 96px}
h1,h2,h3{font-family:"Schibsted Grotesk",ui-sans-serif,system-ui,sans-serif;letter-spacing:-.02em;line-height:1.15;margin:0}
h1{font-size:2.1rem}h2{font-size:1.45rem;margin:56px 0 16px}h3{font-size:1.05rem}
p{margin:0 0 12px}.lead{color:var(--ink-2);font-size:1.05rem;max-width:68ch}
.mono{font-family:"DM Mono",ui-monospace,monospace;font-size:.9em}
.meta{display:flex;flex-wrap:wrap;gap:8px;margin:16px 0 24px}
.meta span{background:var(--paper-2);border:1px solid var(--rule);border-radius:999px;padding:4px 12px;font-size:.9rem}
.toc{display:flex;flex-wrap:wrap;gap:6px 16px;margin:0 0 8px;font-size:.95rem}
.pair{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:16px}
.shot{position:relative;margin:0;background:#f4f1e8;border:1px solid var(--rule);border-radius:14px;overflow:hidden}
.shot img{display:block;width:100%;height:auto}
.tag{position:absolute;transform:translate(-50%,-50%);background:var(--card);color:var(--ink);border:1.5px solid var(--ink);border-radius:999px;padding:1px 7px;font:600 12px/1.4 "DM Mono",ui-monospace,monospace;white-space:nowrap;box-shadow:0 1px 3px #0003}
.tag--focus{background:var(--ink);color:var(--card);font-size:14px;padding:2px 9px}
.ring{position:absolute;border:2.5px solid var(--ink);border-radius:50%;box-shadow:0 0 0 3px #fffdf7aa}
.glue{display:inline-block;width:22px;height:14px;vertical-align:-2px;border-radius:3px;background:repeating-linear-gradient(45deg,var(--glue) 0 4px,#f6c4db 4px 8px)}
.detail{margin-top:12px;max-width:320px}.detail small{color:var(--muted)}
ol.prep{padding-left:22px;max-width:72ch}ol.prep li{margin-bottom:8px}
.legend{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}
.chip{display:flex;gap:12px;align-items:center;background:var(--card);border:1px solid var(--rule);border-radius:14px;padding:12px}
.sw{width:36px;height:36px;border-radius:50%;border:1px solid #0002;flex:none}
.sw--s{width:14px;height:14px;display:inline-block;vertical-align:-2px;margin-right:4px}
.chip small,.card small{color:var(--muted);display:block}
.plates{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:16px}
.plate h3{margin:10px 0 2px}
.filters,.tools{display:flex;flex-wrap:wrap;gap:8px;align-items:center;margin:0 0 16px}
button{font:inherit;font-size:.9rem;background:var(--card);color:var(--ink);border:1px solid var(--rule);border-radius:999px;padding:6px 14px;cursor:pointer;min-height:36px}
button:hover{border-color:var(--ink-2)}button[aria-pressed=true]{background:var(--ink);color:var(--card);border-color:var(--ink)}
button:focus-visible,input:focus-visible,a:focus-visible{outline:3px solid var(--accent);outline-offset:2px}
.progress{font-weight:600}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(170px,1fr));gap:12px}
.card{background:var(--card);border:1px solid var(--rule);border-radius:14px;overflow:hidden}
.card[hidden]{display:none}
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
.group{margin-top:40px}.group h3{font-size:1.2rem}.group small{color:var(--muted)}
.step{display:grid;grid-template-columns:minmax(0,3fr) minmax(220px,2fr);gap:20px;align-items:center;padding:20px 0;border-top:1px solid var(--rule)}
.step .n{font:600 .8rem "DM Mono",ui-monospace,monospace;color:var(--accent);text-transform:uppercase;letter-spacing:.06em}
.step h3{font-size:1.25rem;margin:4px 0 8px}
.done{display:inline-flex;gap:8px;align-items:center;cursor:pointer;min-height:36px}.done input{width:20px;height:20px;accent-color:var(--accent)}
.step.is-done .shot{opacity:.45}.step.is-done h3{text-decoration:line-through;text-decoration-thickness:2px}
@media (max-width:720px){.step{grid-template-columns:1fr}}
@media print{body{background:#fff}main{padding:0;max-width:none}.tools,.filters,.toc,.done{display:none}.step{break-inside:avoid}h2{break-after:avoid}
body.print-steps main>:not(#assembly){display:none}
body.print-steps #assembly{display:grid;grid-template-columns:1fr 1fr;gap:10px}
body.print-steps #assembly>:not(.step){grid-column:1/-1}
body.print-steps .step{display:block;border:1px solid var(--rule);border-radius:10px;padding:10px}
body.print-steps .step h3{font-size:1rem}body.print-steps .detail{display:none}
.step.is-done .shot{opacity:1}.step.is-done h3{text-decoration:none}}
"#;

/// Remembers ticked steps in the browser, filters the piece index and prints the steps as cards.
const SCRIPT: &str = r#"
(()=>{const key='bp-kit:'+document.body.dataset.kit;let done={};try{done=JSON.parse(localStorage.getItem(key)||'{}')||{}}catch(e){}
const boxes=[...document.querySelectorAll('input[data-step]')],out=document.getElementById('progress');
const sync=()=>{const n=boxes.filter(b=>b.checked).length;if(out)out.textContent=out.dataset.tpl.replace('{n}',n).replace('{total}',boxes.length)};
const save=()=>{try{localStorage.setItem(key,JSON.stringify(done))}catch(e){}};
for(const b of boxes){b.checked=!!done[b.dataset.step];b.closest('.step').classList.toggle('is-done',b.checked);
b.addEventListener('change',()=>{done[b.dataset.step]=b.checked;save();b.closest('.step').classList.toggle('is-done',b.checked);sync()})}
sync();
const reset=document.getElementById('reset');if(reset)reset.addEventListener('click',()=>{done={};save();for(const b of boxes){b.checked=false;b.closest('.step').classList.remove('is-done')}sync()});
const print=document.getElementById('print-steps');if(print)print.addEventListener('click',()=>{document.body.classList.add('print-steps');window.print()});
addEventListener('afterprint',()=>document.body.classList.remove('print-steps'));
const filters=[...document.querySelectorAll('[data-filter]')],cards=[...document.querySelectorAll('.card[data-slot]')];
for(const f of filters)f.addEventListener('click',()=>{for(const g of filters)g.setAttribute('aria-pressed',String(g===f));const v=f.dataset.filter;
for(const c of cards)c.hidden=!(v==='all'||(v==='review'?c.dataset.review==='1':c.dataset.slot===v))})})();
"#;

/// The language of the kit: its guide, summary and file names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Es,
    En,
}

impl Lang {
    /// "en", "en-GB"… are English; anything else Spanish.
    pub fn from_code(code: &str) -> Self {
        if code.to_ascii_lowercase().starts_with("en") {
            Self::En
        } else {
            Self::Es
        }
    }

    fn t(self, es: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::Es => es,
            Self::En => en,
        }
    }

    fn code(self) -> &'static str {
        self.t("es", "en")
    }

    /// `n` and the word for it, singular or plural.
    fn n(self, n: usize, es: (&str, &str), en: (&str, &str)) -> String {
        let (one, many) = if self == Self::Es { es } else { en };
        format!("{n} {}", if n == 1 { one } else { many })
    }

    fn pieces(self, n: usize) -> String {
        self.n(n, ("pieza", "piezas"), ("piece", "pieces"))
    }

    fn plates(self, n: usize) -> String {
        self.n(n, ("placa", "placas"), ("plate", "plates"))
    }

    fn colours(self, n: usize) -> String {
        self.n(n, ("color", "colores"), ("colour", "colours"))
    }

    fn steps(self, n: usize) -> String {
        self.n(n, ("paso", "pasos"), ("step", "steps"))
    }

    fn colour_name(self, rgb: [u8; 3]) -> &'static str {
        let es = color_name(rgb);
        if self == Self::Es {
            return es;
        }
        match es {
            "Negro" => "Black",
            "Blanco" => "White",
            "Gris claro" => "Light grey",
            "Gris" => "Grey",
            "Gris oscuro" => "Dark grey",
            "Marrón oscuro" => "Dark brown",
            "Marrón" => "Brown",
            "Granate" => "Maroon",
            "Rojo" => "Red",
            "Crema" => "Cream",
            "Caramelo" => "Caramel",
            "Naranja" => "Orange",
            "Amarillo" => "Yellow",
            "Verde" => "Green",
            "Turquesa" => "Turquoise",
            "Azul" => "Blue",
            "Morado" => "Purple",
            _ => "Pink",
        }
    }
}

/// The kit's own files, named in its language.
struct Files {
    readme: &'static str,
    summary: &'static str,
    guide: &'static str,
    guide_txt: &'static str,
}

impl Files {
    fn new(lang: Lang) -> Self {
        Self {
            readme: lang.t("LEEME.txt", "README.txt"),
            summary: lang.t("resumen.html", "summary.html"),
            guide: lang.t("guia-de-armado.html", "assembly-guide.html"),
            guide_txt: lang.t("guia-de-armado.txt", "assembly-guide.txt"),
        }
    }
}

/// Where each picture of the guide is saved, next to the page.
struct Paths {
    overview: Vec<String>,
    thumbs: Vec<String>,
    plates: Vec<String>,
    steps: Vec<String>,
    details: Vec<Option<String>>,
    joins: Vec<String>,
}

impl Paths {
    fn new(n: &Names, pics: &Pictures) -> Self {
        let l = n.lang;
        let details = l.t("detalle", "detail");
        Self {
            overview: (1..=pics.overview.len()).map(|k| format!("img/{}-{k}.png", l.t("modelo", "model"))).collect(),
            thumbs: n.labels.iter().map(|p| format!("img/{}-{p}.png", l.t("pieza", "piece"))).collect(),
            plates: (1..=n.kit.plates.len()).map(|k| format!("img/{}-{k:02}.png", l.t("placa", "plate"))).collect(),
            steps: (1..=n.kit.steps.len()).map(|k| format!("img/{}-{k:03}.png", l.t("paso", "step"))).collect(),
            details: pics
                .details
                .iter()
                .enumerate()
                .map(|(k, d)| d.as_ref().map(|_| format!("img/{}-{:03}-{details}.png", l.t("paso", "step"), k + 1)))
                .collect(),
            joins: (1..n.kit.groups.len())
                .map(|g| format!("img/{}-{}.png", l.t("subconjunto", "group"), n.groups[g]))
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
            .chain(pairs(&self.joins, &pics.joins))
    }
}

/// What a piece needs looked at, as the user reads it.
fn issue_text(lang: Lang, issue: &Issue) -> String {
    match (lang, issue) {
        (Lang::Es, Issue::Open) => "su superficie no cierra: el laminador puede rellenarla mal".into(),
        (Lang::En, Issue::Open) => "its surface is not closed: the slicer may fill it wrongly".into(),
        (Lang::Es, Issue::Degenerate(n)) => format!("{n} triángulos sin área"),
        (Lang::En, Issue::Degenerate(n)) => format!("{n} triangles without area"),
        (Lang::Es, Issue::Oversize) => "es más grande que la placa: escálala o córtala en Bambu Studio".into(),
        (Lang::En, Issue::Oversize) => "it is larger than the plate: scale or cut it in Bambu Studio".into(),
        (Lang::Es, Issue::Thin(mm)) => format!("su lado más fino mide {mm:.1} mm, menos de dos líneas de la boquilla"),
        (Lang::En, Issue::Thin(mm)) => format!("its thinnest side is {mm:.1} mm, under two nozzle lines"),
        (Lang::Es, Issue::Tiny(mm)) => format!("mide {mm:.1} mm como mucho: difícil de imprimir y de manipular"),
        (Lang::En, Issue::Tiny(mm)) => format!("it is {mm:.1} mm at most: hard to print and to handle"),
    }
}

/// Sub-assembly names: A, B… Z, then G27, G28…
fn group_name(g: usize) -> String {
    if g < 26 {
        char::from(b'A' + g as u8).to_string()
    } else {
        format!("G{}", g + 1)
    }
}

/// Everything the guide, the summary and the file names share.
struct Names<'a> {
    kit: &'a Kit,
    info: &'a KitInfo<'a>,
    lang: Lang,
    f: Files,
    labels: Vec<String>,
    groups: Vec<String>,
    /// Each piece's STL in the ZIP, when there is one.
    files: Vec<Option<String>>,
    three_mf: String,
    issues: Vec<Vec<Issue>>,
}

impl<'a> Names<'a> {
    fn new(kit: &'a Kit, info: &'a KitInfo<'a>) -> Self {
        let lang = info.lang;
        let digits = kit.pieces.len().to_string().len().max(2);
        let labels: Vec<String> = (0..kit.pieces.len()).map(|i| format!("P{:0digits$}", i + 1)).collect();
        let base = slug(info.name);
        let base = if base.is_empty() { lang.t("modelo", "model").to_string() } else { base };
        let mut names = Self {
            kit,
            info,
            lang,
            f: Files::new(lang),
            labels,
            groups: (0..kit.groups.len()).map(group_name).collect(),
            files: vec![],
            three_mf: format!("3mf/{base}_{}.3mf", lang.t("placas", "plates")),
            issues: vec![],
        };
        names.files = (0..kit.pieces.len())
            .map(|i| {
                let s = kit.pieces[i].slot;
                let dir = format!("stl/{}-{}", s + 1, slug(names.colour(s)));
                match info.params.contents.stl {
                    StlFiles::None => None,
                    StlFiles::Piece => Some(format!("{dir}/{}.stl", names.labels[i])),
                    StlFiles::Colour => Some(format!("{dir}.stl")),
                }
            })
            .collect();
        names.issues = bp_core::kit::check(kit, info.printer.nozzle_mm);
        names
    }

    fn t(&self, es: &'static str, en: &'static str) -> &'static str {
        self.lang.t(es, en)
    }

    fn has(&self) -> crate::project_file::KitContents {
        self.info.params.contents
    }

    /// How to print the kit with what the ZIP holds, as HTML when `html`.
    fn how_to_print(&self, html: bool) -> String {
        let c = self.has();
        let file = if html {
            format!("<a class=\"mono\" href=\"{0}\">{0}</a>", escape(&self.three_mf))
        } else {
            self.three_mf.clone()
        };
        match (self.lang, c.three_mf, c.stl != StlFiles::None) {
            (Lang::Es, true, _) => {
                format!("Abre {file} en Bambu Studio e imprime cada placa con el filamento que indica su nombre")
            }
            (Lang::En, true, _) => {
                format!("Open {file} in Bambu Studio and print each plate with the filament its name gives")
            }
            (Lang::Es, false, true) => "Lamina los STL de stl/, cada carpeta con su filamento".into(),
            (Lang::En, false, true) => "Slice the STLs in stl/, each folder with its filament".into(),
            (Lang::Es, false, false) => "Imprime cada pieza con su filamento".into(),
            (Lang::En, false, false) => "Print each piece with its filament".into(),
        }
    }

    /// A link into the guide, or just the text when the ZIP has no guide.
    fn guide_link(&self, anchor: &str, text: &str, class: &str) -> String {
        if self.has().guide {
            format!("<a{class} href=\"{}#{anchor}\">{text}</a>", self.f.guide)
        } else {
            text.to_string()
        }
    }

    fn hex(&self, slot: u8) -> String {
        self.info.palette.get(slot as usize).cloned().unwrap_or_else(|| "#808080".into())
    }

    /// The colour's everyday name ("Naranja", "Orange").
    fn colour(&self, slot: u8) -> &'static str {
        self.lang.colour_name(rgb(&self.hex(slot)))
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

    fn plate_word(&self) -> &'static str {
        self.t("Placa", "Plate")
    }

    fn plate_anchor(&self, k: usize) -> String {
        format!("{}-{}", self.t("placa", "plate"), k + 1)
    }

    fn plate_name(&self, k: usize) -> String {
        let slots = &self.kit.plates[k].slots;
        let colours: Vec<String> = slots.iter().map(|&s| format!("{} {}", self.filament(s), self.hex(s))).collect();
        format!("{} {} · {}", self.plate_word(), k + 1, colours.join(" + "))
    }

    fn grams(&self, mm3: f32) -> f32 {
        mm3 * self.info.density_g_cm3 / 1000.0
    }

    /// The pieces of sub-assembly `g`, in the order they go on.
    fn group_pieces(&self, g: usize) -> Vec<usize> {
        self.kit.steps[self.kit.groups[g].steps.clone()].iter().map(|s| s.piece).collect()
    }

    /// "P01–P12" when its labels run on, else the list.
    fn piece_list(&self, ids: &[usize]) -> String {
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        let runs = sorted.windows(2).all(|w| w[1] == w[0] + 1);
        match sorted.as_slice() {
            [] => String::new(),
            [one] => self.labels[*one].clone(),
            [first, .., last] if runs => format!("{}–{}", self.labels[*first], self.labels[*last]),
            _ => sorted.iter().map(|&i| self.labels[i].as_str()).collect::<Vec<_>>().join(", "),
        }
    }

    /// The pieces to look at, with why.
    fn review(&self) -> Vec<(usize, &[Issue])> {
        self.issues.iter().enumerate().filter(|(_, v)| !v.is_empty()).map(|(i, v)| (i, v.as_slice())).collect()
    }

    /// What the export changed or left out on its own, for the user to know.
    fn notes(&self) -> Vec<String> {
        let (l, k, p) = (self.lang, self.kit, &self.info.params);
        let mut notes = vec![];
        if k.voxel_mm > p.voxel_mm * 1.01 {
            notes.push(match l {
                Lang::Es => format!("El modelo es grande para el detalle pedido: las uniones se calcularon con {:.2} mm en lugar de {:.2} mm.", k.voxel_mm, p.voxel_mm),
                Lang::En => format!("The model is large for the detail asked: joints were worked out at {:.2} mm instead of {:.2} mm.", k.voxel_mm, p.voxel_mm),
            });
        }
        if k.merged > 0 {
            let one = k.merged == 1;
            notes.push(match l {
                Lang::Es => format!(
                    "{} de menos de {:.0} mm³ {} a la pieza vecina con la que más contacto {}.",
                    l.n(k.merged, ("miga", "migas"), ("", "")),
                    p.min_piece_mm3,
                    if one { "se unió" } else { "se unieron" },
                    if one { "tenía" } else { "tenían" }
                ),
                Lang::En => format!(
                    "{} under {:.0} mm³ {} to the neighbouring piece {} touched most.",
                    l.n(k.merged, ("", ""), ("crumb", "crumbs")),
                    p.min_piece_mm3,
                    if one { "was joined" } else { "were joined" },
                    if one { "it" } else { "each" }
                ),
            });
        }
        if k.dropped > 0 {
            let one = k.dropped == 1;
            notes.push(match l {
                Lang::Es => format!(
                    "{} para formar una pieza {} fuera; revisa el modelo armado por si falta algo.",
                    l.n(k.dropped, ("zona demasiado fina", "zonas demasiado finas"), ("", "")),
                    if one { "se dejó" } else { "se dejaron" }
                ),
                Lang::En => format!(
                    "{} too thin to make a piece {} left out; check the assembled model for anything missing.",
                    l.n(k.dropped, ("", ""), ("region", "regions")),
                    if one { "was" } else { "were" }
                ),
            });
        }
        if let Some(n) = k.exact_skin.filter(|&n| n < k.pieces.len()) {
            let rough = k.pieces.len() - n;
            notes.push(match l {
                Lang::Es => format!(
                    "{} la superficie del modelo aproximada a {:.2} mm en lugar de la exacta: el modelo no está cerrado. «Reparar malla» (rellenar agujeros) suele arreglarlo.",
                    if rough == 1 { "1 pieza lleva".to_string() } else { format!("{rough} piezas llevan") },
                    k.voxel_mm
                ),
                Lang::En => format!(
                    "{} the model's surface to within {:.2} mm instead of exactly: the model is not closed. Mesh repair (fill holes) usually fixes that.",
                    if rough == 1 { "1 piece has".to_string() } else { format!("{rough} pieces have") },
                    k.voxel_mm
                ),
            });
        }
        if !k.pins.is_empty() || k.unpinned > 0 {
            let (pins, missed) = (k.pins.len(), k.unpinned);
            notes.push(match l {
                Lang::Es => {
                    let mut t = format!(
                        "{} con {} alojamiento {} las piezas: {} cada unión, pero conviene pegarlas igual.",
                        l.n(pins, ("espiga", "espigas"), ("", "")),
                        if pins == 1 { "su" } else { "sus" },
                        if pins == 1 { "encaja" } else { "encajan" },
                        if pins == 1 { "guía" } else { "guían" }
                    );
                    if missed > 0 {
                        let _ = write!(
                            t,
                            " {} no {} sitio para una y {} solo pegada{}.",
                            l.n(missed, ("unión", "uniones"), ("", "")),
                            if missed == 1 { "tenía" } else { "tenían" },
                            if missed == 1 { "va" } else { "van" },
                            if missed == 1 { "" } else { "s" }
                        );
                    }
                    t
                }
                Lang::En => {
                    let mut t = format!(
                        "{} in {} {} the pieces together, lining up the joint{}; glue them all the same.",
                        l.n(pins, ("", ""), ("pin", "pins")),
                        if pins == 1 { "its socket" } else { "their sockets" },
                        if pins == 1 { "keys" } else { "key" },
                        if pins == 1 { "" } else { "s" }
                    );
                    if missed > 0 {
                        let _ = write!(
                            t,
                            " {} had no room for one and {} only glued.",
                            l.n(missed, ("", ""), ("joint", "joints")),
                            if missed == 1 { "is" } else { "are" }
                        );
                    }
                    t
                }
            });
        }
        notes
    }

    /// A key that tells this kit's ticked steps apart from another's in the browser.
    fn kit_key(&self) -> String {
        let volume: f32 = self.kit.pieces.iter().map(|p| p.volume_mm3).sum();
        format!("{}|{}|{}|{:.0}", slug(self.info.name), self.kit.pieces.len(), self.kit.steps.len(), volume)
    }
}

fn swatch(hex: &str) -> String {
    format!("<span class=\"sw sw--s\" style=\"background:{hex}\"></span>")
}

fn guide_html(n: &Names, pics: &Pictures, paths: &Paths) -> String {
    let (kit, info, labels, l) = (n.kit, n.info, &n.labels, n.lang);
    let (lo, hi) = bounds(kit.pieces.iter().flat_map(|p| p.positions.iter()));
    let slots = n.slots();
    let grouped = kit.groups.len() > 1;

    let mut h = String::with_capacity(1 << 18);
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"{code}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} · {name}</title><style>{CSS}</style></head><body data-kit=\"{key}\"><main>\
<h1>{title} · {name}</h1>\
<p class=\"lead\">{lead}</p>\
<div class=\"meta\"><span>{np}</span><span>{c}</span><span>{p}</span><span>{x:.0} × {y:.0} × {z:.0} mm {built}</span><span>{joints} {v:.2} mm</span></div>\
<nav class=\"toc\" aria-label=\"{contents}\"><a href=\"#prep\">{prep}</a><a href=\"#filaments\">{filaments}</a><a href=\"#plates\">{plates}</a><a href=\"#pieces\">{pieces}</a><a href=\"#assembly\">{assembly}</a></nav>\
<div class=\"pair\">{o1}{o2}</div>",
        code = l.code(),
        title = n.t("Guía de armado", "Assembly guide"),
        name = escape(info.name),
        key = escape(&n.kit_key()),
        lead = match l {
            Lang::Es => format!("El modelo dividido en piezas sólidas, una o varias por color. Imprime cada placa con su filamento y pega las piezas en el orden de los pasos. El <a href=\"{}\">resumen</a> dice qué trae el kit y qué revisar.", n.f.summary),
            Lang::En => format!("The model split into solid pieces, one or more per colour. Print each plate with its filament and glue the pieces in the order of the steps. The <a href=\"{}\">summary</a> says what the kit holds and what to check.", n.f.summary),
        },
        np = l.pieces(kit.pieces.len()),
        c = l.colours(slots.len()),
        p = l.plates(kit.plates.len()),
        x = hi[0] - lo[0],
        y = hi[1] - lo[1],
        z = hi[2] - lo[2],
        built = n.t("armado", "assembled"),
        joints = n.t("Uniones de", "Joints of"),
        v = kit.voxel_mm,
        contents = n.t("Contenido", "Contents"),
        prep = n.t("Antes de empezar", "Before you start"),
        filaments = n.t("Filamentos", "Filaments"),
        plates = n.t("Placas", "Plates"),
        pieces = n.t("Piezas", "Pieces"),
        assembly = n.t("Armado", "Assembly"),
        o1 = pics.overview[0].html(&paths.overview[0], n.t("Modelo armado, vista frontal", "Assembled model, front view")),
        o2 = pics.overview[1].html(&paths.overview[1], n.t("Modelo armado, vista trasera", "Assembled model, back view")),
    );

    let review = n.review();
    if !review.is_empty() {
        let links: Vec<String> = review.iter().map(|(i, _)| format!("<a href=\"#{0}\">{0}</a>", labels[*i])).collect();
        let _ = write!(
            h,
            "<div class=\"alert\"><b>{} {}:</b> {}. {}</div>",
            l.pieces(review.len()),
            n.t("que revisar antes de imprimir", "to check before printing"),
            links.join(", "),
            n.t("Su ficha dice por qué.", "Their cards say why.")
        );
    }

    let _ = write!(h, "<h2 id=\"prep\">{}</h2><ol class=\"prep\">", n.t("Antes de empezar", "Before you start"));
    let stl = match (l, n.has().stl) {
        (_, StlFiles::None) => "",
        (Lang::Es, StlFiles::Piece) => " Su STL, en <span class=\"mono\">stl/</span>, lleva el mismo número.",
        (Lang::En, StlFiles::Piece) => " Its STL, in <span class=\"mono\">stl/</span>, has the same number.",
        (Lang::Es, StlFiles::Colour) => {
            " En <span class=\"mono\">stl/</span> hay un STL por color con sus piezas ya armadas."
        }
        (Lang::En, StlFiles::Colour) => {
            " <span class=\"mono\">stl/</span> holds one STL per colour with its pieces assembled."
        }
    };
    let prep: [String; 5] = match l {
        Lang::Es => [
            format!("{}.", n.how_to_print(true)),
            "Las piezas ya están giradas sobre su cara más plana. Si Bambu avisa de voladizos, activa soportes en árbol solo sobre la placa.".into(),
            format!("Al despegar cada placa, separa las piezas por número usando la sección <a href=\"#pieces\">Piezas</a>: cada una aparece con su forma y su color, y puedes filtrarlas por color.{stl}"),
            "Prueba cada unión en seco antes de pegar. Lija un poco las caras de contacto para que el pegamento agarre.".into(),
            "Pon pegamento solo en las zonas rayadas <span class=\"glue\" aria-hidden=\"true\"></span> de cada imagen: son las caras que tocan la pieza nueva. Para PLA usa cianoacrilato en gel; para uniones grandes, epoxi de 5 minutos.".into(),
        ],
        Lang::En => [
            format!("{}.", n.how_to_print(true)),
            "Pieces are already turned onto their flattest face. If Bambu warns about overhangs, turn on tree supports on the plate only.".into(),
            format!("As each plate comes off, sort the pieces by number with the <a href=\"#pieces\">Pieces</a> section: each one shows its shape and colour, and you can filter them by colour.{stl}"),
            "Dry-fit every joint before gluing. Sand the contact faces lightly so the glue grips.".into(),
            "Put glue only on the striped areas <span class=\"glue\" aria-hidden=\"true\"></span> in each picture: the faces the new piece touches. For PLA use gel superglue; for large joints, 5-minute epoxy.".into(),
        ],
    };
    for li in prep {
        let _ = write!(h, "<li>{li}</li>");
    }
    h.push_str("</ol>");

    let _ = write!(h, "<h2 id=\"filaments\">{}</h2><div class=\"legend\">", n.t("Filamentos", "Filaments"));
    for &s in &slots {
        let hex = n.hex(s);
        let count = kit.pieces.iter().filter(|p| p.slot == s).count();
        let plates: Vec<String> = kit
            .plates
            .iter()
            .enumerate()
            .filter(|(_, p)| p.slots.contains(&s))
            .map(|(k, _)| format!("<a href=\"#{}\">{}</a>", n.plate_anchor(k), k + 1))
            .collect();
        let _ = write!(
            h,
            "<div class=\"chip\"><span class=\"sw\" style=\"background:{hex}\"></span><div><b>{name}</b> <span class=\"mono\">{hex}</span>\
<small>{slot} {f} · {pieces} · {pl} {list}</small></div></div>",
            name = escape(&n.filament(s)),
            slot = n.t("Ranura", "Slot"),
            f = s + 1,
            pieces = l.pieces(count),
            pl = if plates.len() > 1 { n.t("placas", "plates") } else { n.t("placa", "plate") },
            list = plates.join(", ")
        );
    }
    h.push_str("</div>");

    let _ = write!(h, "<h2 id=\"plates\">{}</h2><div class=\"plates\">", n.t("Placas", "Plates"));
    for (k, plate) in kit.plates.iter().enumerate() {
        let colours: Vec<String> =
            plate.slots.iter().map(|&s| format!("{}{}", swatch(&n.hex(s)), escape(&n.filament(s)))).collect();
        let _ = write!(
            h,
            "<div class=\"plate\" id=\"{anchor}\">{shot}<h3>{word} {n1} · {name}</h3><small>{pieces} · {occ_word} {occ:.0} %{tower}: {list}</small></div>",
            anchor = n.plate_anchor(k),
            shot = pics.plates[k].html(&paths.plates[k], &format!("{} {}", n.plate_word(), k + 1)),
            word = n.plate_word(),
            n1 = k + 1,
            name = colours.join(" + "),
            pieces = l.pieces(plate.pieces.len()),
            occ_word = n.t("ocupa el", "fills"),
            occ = plate.occupancy * 100.0,
            tower = if plate.mixed() { n.t(" · con torre de purga", " · with a prime tower") } else { "" },
            list = plate.pieces.iter().map(|&i| format!("<a href=\"#{0}\">{0}</a>", labels[i])).collect::<Vec<_>>().join(", ")
        );
    }
    h.push_str("</div>");

    let _ = write!(
        h,
        "<h2 id=\"pieces\">{}</h2><div class=\"filters\" role=\"group\" aria-label=\"{}\"><button type=\"button\" data-filter=\"all\" aria-pressed=\"true\">{}</button>",
        n.t("Piezas", "Pieces"),
        n.t("Filtrar piezas", "Filter pieces"),
        n.t("Todas", "All")
    );
    for &s in &slots {
        let _ = write!(
            h,
            "<button type=\"button\" data-filter=\"{s}\" aria-pressed=\"false\">{}{}</button>",
            swatch(&n.hex(s)),
            escape(&n.filament(s))
        );
    }
    if !review.is_empty() {
        let _ = write!(
            h,
            "<button type=\"button\" data-filter=\"review\" aria-pressed=\"false\">{}</button>",
            n.t("Por revisar", "To check")
        );
    }
    h.push_str("</div><div class=\"grid\">");
    let step_of: Vec<usize> = {
        let mut s = vec![0; kit.pieces.len()];
        for (k, step) in kit.steps.iter().enumerate() {
            s[step.piece] = k;
        }
        s
    };
    for (i, p) in kit.pieces.iter().enumerate() {
        let s = kit.placed[i].size;
        let warn: String = n.issues[i]
            .iter()
            .map(|x| format!("<small class=\"warn\">{}</small>", escape(&issue_text(l, x))))
            .collect();
        let _ = write!(
            h,
            "<div class=\"card{flag}\" id=\"{label}\" data-slot=\"{slot}\" data-review=\"{rev}\">{shot}<div><b>{label}</b> · {sw}{name}<small>{x:.0} × {y:.0} × {z:.0} mm · {vol:.1} cm³</small>\
<small><a href=\"#{plate_anchor}\">{plate_word} {plate}</a> · <a href=\"#{step_anchor}\">{step_word} {step}</a>{group}</small>{file}{warn}</div></div>",
            flag = if n.issues[i].is_empty() { "" } else { " card--warn" },
            slot = p.slot,
            rev = u8::from(!n.issues[i].is_empty()),
            shot = pics.thumbs[i].html(&paths.thumbs[i], &labels[i]),
            label = labels[i],
            sw = swatch(&n.hex(p.slot)),
            name = n.piece_colour(i),
            x = s[0],
            y = s[1],
            z = s[2],
            vol = p.volume_mm3 / 1000.0,
            plate_anchor = n.plate_anchor(kit.placed[i].plate),
            plate_word = n.plate_word(),
            plate = kit.placed[i].plate + 1,
            step_anchor = step_anchor(n, step_of[i]),
            step_word = n.t("paso", "step"),
            step = step_of[i] + 1,
            group = if grouped { format!(" · {} {}", n.t("subconjunto", "group"), n.groups[kit.steps[step_of[i]].group]) } else { String::new() },
            file = n.files[i].as_deref().map_or(String::new(), |f| format!("<small><a class=\"mono\" href=\"{0}\">{0}</a></small>", escape(f))),
        );
    }
    h.push_str("</div>");

    let total = kit.steps.len() + kit.groups.len().saturating_sub(1);
    let _ = write!(
        h,
        "<section id=\"assembly\"><h2>{title}</h2><p class=\"lead\">{lead}</p>\
<div class=\"tools\"><span class=\"progress\" id=\"progress\" role=\"status\" data-tpl=\"{tpl}\"></span>\
<button type=\"button\" id=\"print-steps\">{print}</button><button type=\"button\" id=\"reset\">{reset}</button></div>",
        title = n.t("Armado", "Assembly"),
        lead = match (l, grouped) {
            (Lang::Es, false) => format!("{}. En cada imagen la pieza nueva aparece separada, en su color y rodeada; lo ya armado, en tonos suaves, con el pegamento en las zonas rayadas <span class=\"glue\" aria-hidden=\"true\"></span>. Marca cada paso al terminarlo: la guía lo recuerda en este navegador.", l.steps(total)),
            (Lang::En, false) => format!("{}. In each picture the new piece is pulled out, in its colour and ringed; what is built is pale, with glue on the striped areas <span class=\"glue\" aria-hidden=\"true\"></span>. Tick each step when done: the guide remembers it in this browser.", l.steps(total)),
            (Lang::Es, true) => format!("{} en {} subconjuntos ({}): arma cada uno por separado y luego únelos en la <a href=\"#final\">unión final</a>. En cada imagen la pieza nueva aparece separada, en su color y rodeada; el pegamento va en las zonas rayadas <span class=\"glue\" aria-hidden=\"true\"></span>. Marca cada paso al terminarlo: la guía lo recuerda en este navegador.", l.steps(total), kit.groups.len(), n.groups.iter().map(|g| format!("<a href=\"#sub-{g}\">{g}</a>")).collect::<Vec<_>>().join(", ")),
            (Lang::En, true) => format!("{} in {} sub-assemblies ({}): build each one apart, then join them in the <a href=\"#final\">final assembly</a>. In each picture the new piece is pulled out, in its colour and ringed; glue goes on the striped areas <span class=\"glue\" aria-hidden=\"true\"></span>. Tick each step when done: the guide remembers it in this browser.", l.steps(total), kit.groups.len(), n.groups.iter().map(|g| format!("<a href=\"#sub-{g}\">{g}</a>")).collect::<Vec<_>>().join(", ")),
        },
        tpl = escape(n.t("{n} de {total} pasos hechos", "{n} of {total} steps done")),
        print = n.t("Imprimir los pasos", "Print the steps"),
        reset = n.t("Desmarcar todo", "Untick all"),
    );
    let check = |id: &str| {
        format!("<label class=\"done\"><input type=\"checkbox\" data-step=\"{id}\"> {}</label>", n.t("Hecho", "Done"))
    };
    for (g, group) in kit.groups.iter().enumerate() {
        if grouped {
            let ids = n.group_pieces(g);
            let _ = write!(
                h,
                "<div class=\"group\" id=\"sub-{name}\"><h3>{word} {name}</h3><small>{pieces}: {list}</small></div>",
                word = n.t("Subconjunto", "Sub-assembly"),
                name = n.groups[g],
                pieces = l.pieces(ids.len()),
                list = n.piece_list(&ids)
            );
        }
        for k in group.steps.clone() {
            let (title, body) = step_text(n, k);
            let _ = write!(
                h,
                "<section class=\"step\" id=\"{anchor}\">{shot}<div><div class=\"n\">{word} {n1} / {total}</div><h3>{title}</h3><p>{body}</p>{done}{detail}</div></section>",
                anchor = step_anchor(n, k),
                shot = pics.steps[k].html(&paths.steps[k], &format!("{} {}", n.t("Paso", "Step"), k + 1)),
                word = n.t("Paso", "Step"),
                n1 = k + 1,
                done = check(&format!("s{}", k + 1)),
                detail = match (&pics.details[k], &paths.details[k]) {
                    (Some(d), Some(src)) => format!(
                        "<div class=\"detail\">{}<small>{}</small></div>",
                        d.html(src, &format!("{} {}", n.t("Detalle del paso", "Close-up of step"), k + 1)),
                        n.t("Detalle", "Close-up")
                    ),
                    _ => String::new(),
                },
            );
        }
    }
    if grouped {
        let _ = write!(
            h,
            "<div class=\"group\" id=\"final\"><h3>{}</h3><small>{}</small></div>",
            n.t("Unión final", "Final assembly"),
            n.t(
                "Pega cada subconjunto, ya seco, a lo armado antes.",
                "Glue each sub-assembly, once dry, to what is built before it."
            )
        );
        for g in 1..kit.groups.len() {
            let k = kit.steps.len() + g - 1;
            let (title, body) = join_text(n, g);
            let _ = write!(
                h,
                "<section class=\"step\" id=\"{anchor}\">{shot}<div><div class=\"n\">{word} {n1} / {total}</div><h3>{title}</h3><p>{body}</p>{done}</div></section>",
                anchor = step_anchor(n, k),
                shot = pics.joins[g - 1].html(&paths.joins[g - 1], &title),
                word = n.t("Paso", "Step"),
                n1 = k + 1,
                done = check(&format!("j{}", n.groups[g])),
            );
        }
    }
    let _ = write!(
        h,
        "</section><p class=\"lead\" style=\"margin-top:40px\">{}</p></main><script>{SCRIPT}</script></body></html>",
        n.t(
            "Listo: el modelo está completo. Deja curar el pegamento antes de manipularlo.",
            "Done: the model is complete. Let the glue cure before handling it."
        )
    );
    h.push('\n');
    h
}

fn step_anchor(n: &Names, k: usize) -> String {
    format!("{}-{}", n.t("paso", "step"), k + 1)
}

/// Title and explanation of step `k`, shared by the HTML and the text guide.
fn step_text(n: &Names, k: usize) -> (String, String) {
    let (kit, labels, l) = (n.kit, &n.labels, n.lang);
    let step = &kit.steps[k];
    let p = step.piece;
    let title = format!("{} · {}", labels[p], n.piece_colour(p));
    let area = |j: usize| kit.pieces[p].contacts.iter().find(|c| c.0 == j).map_or(0.0, |c| c.1);
    let first = kit.groups[step.group].steps.start == k;
    let body = match (l, first, step.group, step.joins.is_empty()) {
        (Lang::Es, true, 0, _) => {
            format!("Empieza con {}: es la pieza más grande y sirve de base. Las demás se pegan sobre ella.", labels[p])
        }
        (Lang::En, true, 0, _) => {
            format!("Start with {}: it is the largest piece and the base the others go on.", labels[p])
        }
        (Lang::Es, true, g, _) => format!(
            "Empieza el subconjunto {} con {}, aparte de lo armado: lo unirás en la unión final.",
            n.groups[g], labels[p]
        ),
        (Lang::En, true, g, _) => format!(
            "Start sub-assembly {} with {}, apart from what is built: it joins in the final assembly.",
            n.groups[g], labels[p]
        ),
        (Lang::Es, false, _, true) => format!(
            "{} no toca ninguna pieza anterior: déjala a un lado y pégala cuando llegue la pieza que la sostiene.",
            labels[p]
        ),
        (Lang::En, false, _, true) => format!(
            "{} touches no piece before it: set it aside and glue it on once the piece that holds it is in place.",
            labels[p]
        ),
        (_, false, _, false) => {
            let list: Vec<String> = step.joins.iter().map(|&j| format!("{} ({:.0} mm²)", labels[j], area(j))).collect();
            let keyed: Vec<&str> = step
                .joins
                .iter()
                .filter(|&&j| kit.pins_between(p, j).next().is_some())
                .map(|&j| labels[j].as_str())
                .collect();
            let crossed = kit.crossed_pins(p, &step.joins);
            let mut pins = String::new();
            if !keyed.is_empty() {
                pins = match l {
                    Lang::Es => {
                        format!(" Las espigas la guían: encájala en {} por ellas antes de apretar.", keyed.join(", "))
                    }
                    Lang::En => format!(" Pins guide it: fit it onto {} by them before pressing.", keyed.join(", ")),
                };
            }
            if !crossed.is_empty() {
                let crossed: Vec<&str> = crossed.iter().map(|&j| labels[j].as_str()).collect();
                pins += &match l {
                    Lang::Es => format!(
                        " Ojo: las espigas con {} entran desde otra dirección; si no encaja, córtalas a ras.",
                        crossed.join(", ")
                    ),
                    Lang::En => format!(
                        " Mind: the pins with {} go in from another way; if it will not seat, cut them flush.",
                        crossed.join(", ")
                    ),
                };
            }
            match l {
                Lang::Es => {
                    format!(
                    "Pega {} a {}: pon pegamento en la zona rayada y presiónala en su sitio, como muestra la imagen.{}",
                    labels[p],
                    list.join(", "),
                    if step.joins.len() > 1 { " Encájala en el hueco entre ellas antes de que seque." } else { "" }
                ) + &pins
                }
                Lang::En => {
                    format!(
                        "Glue {} to {}: put glue on the striped area and press it into place, as the picture shows.{}",
                        labels[p],
                        list.join(", "),
                        if step.joins.len() > 1 {
                            " Seat it in the gap between them before the glue sets."
                        } else {
                            ""
                        }
                    ) + &pins
                }
            }
        }
    };
    (title, body)
}

/// Title and explanation of joining sub-assembly `g` to the ones before it.
fn join_text(n: &Names, g: usize) -> (String, String) {
    let (kit, labels, l) = (n.kit, &n.labels, n.lang);
    let ids = n.group_pieces(g);
    let title = match l {
        Lang::Es => format!("Une el subconjunto {}", n.groups[g]),
        Lang::En => format!("Join sub-assembly {}", n.groups[g]),
    };
    let area = |a: usize, b: usize| kit.pieces[a].contacts.iter().find(|c| c.0 == b).map_or(0.0, |c| c.1);
    let pairs: Vec<String> = kit.groups[g]
        .joins
        .iter()
        .map(|&(a, b)| format!("{} → {} ({:.0} mm²)", labels[a], labels[b], area(a, b)))
        .collect();
    let body = match (l, pairs.is_empty()) {
        (Lang::Es, false) => format!(
            "Pega el subconjunto {} ({}) por {}: pon pegamento en las zonas rayadas y presiónalo en su sitio.",
            n.groups[g],
            n.piece_list(&ids),
            pairs.join(", ")
        ),
        (Lang::En, false) => format!(
            "Glue sub-assembly {} ({}) on at {}: put glue on the striped areas and press it into place.",
            n.groups[g],
            n.piece_list(&ids),
            pairs.join(", ")
        ),
        (Lang::Es, true) => format!(
            "El subconjunto {} ({}) no toca lo armado: colócalo donde encaje según el modelo armado.",
            n.groups[g],
            n.piece_list(&ids)
        ),
        (Lang::En, true) => format!(
            "Sub-assembly {} ({}) touches nothing built: place it where it fits, as in the assembled model.",
            n.groups[g],
            n.piece_list(&ids)
        ),
    };
    (title, body)
}

fn guide_text(n: &Names) -> String {
    let (kit, info, labels, l) = (n.kit, n.info, &n.labels, n.lang);
    let mut t = match l {
        Lang::Es => format!(
            "Guía de armado · {}\n\n{} en {}. {}.\n{}El resumen del kit está en {}.\n\nPLACAS\n",
            info.name,
            l.pieces(kit.pieces.len()),
            l.plates(kit.plates.len()),
            n.how_to_print(false),
            if n.has().guide { format!("La guía con imágenes está en {}. ", n.f.guide) } else { String::new() },
            n.f.summary
        ),
        Lang::En => format!(
            "Assembly guide · {}\n\n{} on {}. {}.\n{}The kit summary is {}.\n\nPLATES\n",
            info.name,
            l.pieces(kit.pieces.len()),
            l.plates(kit.plates.len()),
            n.how_to_print(false),
            if n.has().guide { format!("The guide with pictures is {}. ", n.f.guide) } else { String::new() },
            n.f.summary
        ),
    };
    for (k, plate) in kit.plates.iter().enumerate() {
        let _ = writeln!(
            t,
            "  {:<40} {}",
            n.plate_name(k),
            plate.pieces.iter().map(|&i| labels[i].as_str()).collect::<Vec<_>>().join(" ")
        );
    }
    let _ = writeln!(t, "\n{}", n.t("PIEZAS", "PIECES"));
    for (i, p) in kit.pieces.iter().enumerate() {
        let s = kit.placed[i].size;
        let _ = writeln!(
            t,
            "  {}  {:<14} {:>5.0} × {:>4.0} × {:>4.0} mm  {:>7.1} cm³  {} {:>2}  {}",
            labels[i],
            n.piece_colour(i),
            s[0],
            s[1],
            s[2],
            p.volume_mm3 / 1000.0,
            n.t("placa", "plate"),
            kit.placed[i].plate + 1,
            n.files[i].as_deref().unwrap_or(""),
        );
        for issue in &n.issues[i] {
            let _ = writeln!(t, "        ! {}", issue_text(l, issue));
        }
    }
    let _ = writeln!(t, "\n{}", n.t("ARMADO", "ASSEMBLY"));
    let grouped = kit.groups.len() > 1;
    for (g, group) in kit.groups.iter().enumerate() {
        if grouped {
            let _ = writeln!(
                t,
                "\n  {} {} ({})",
                n.t("SUBCONJUNTO", "SUB-ASSEMBLY"),
                n.groups[g],
                n.piece_list(&n.group_pieces(g))
            );
        }
        for k in group.steps.clone() {
            let (title, body) = step_text(n, k);
            let _ = writeln!(t, "  [ ] {:>3}. {title}\n           {body}", k + 1);
        }
    }
    if grouped {
        let _ = writeln!(t, "\n  {}", n.t("UNIÓN FINAL", "FINAL ASSEMBLY"));
        for g in 1..kit.groups.len() {
            let (title, body) = join_text(n, g);
            let _ = writeln!(t, "  [ ] {:>3}. {title}\n           {body}", kit.steps.len() + g);
        }
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
    let (p, l, bed) = (&n.info.params, n.lang, &n.kit.bed);
    let printer = &n.info.printer;
    let depth = if p.depth_mm > 0.0 {
        let core = p.core_slot.map_or_else(
            || n.t("el más usado", "the most used colour").to_string(),
            |s| format!("{} {}", n.t("ranura", "slot"), s + 1),
        );
        match l {
            Lang::Es => format!("{:.1} mm sobre un núcleo de {core}", p.depth_mm),
            Lang::En => format!("{:.1} mm over a core of {core}", p.depth_mm),
        }
    } else {
        n.t("hasta el centro: piezas macizas", "to the centre: solid pieces").to_string()
    };
    let plates = match l {
        Lang::Es => format!(
            "{}; {:.0} mm al borde, {:.0} mm entre piezas{}",
            if bed.mix { "colores mezclados, con torre de purga" } else { "un color por placa, sin purga" },
            bed.margin_mm,
            bed.gap_mm,
            if bed.excluded.is_empty() { "" } else { ", lejos de las zonas que la impresora excluye" }
        ),
        Lang::En => format!(
            "{}; {:.0} mm to the edge, {:.0} mm between pieces{}",
            if bed.mix { "colours mixed, with a prime tower" } else { "one colour per plate, no purging" },
            bed.margin_mm,
            bed.gap_mm,
            if bed.excluded.is_empty() { "" } else { ", clear of the zones the printer excludes" }
        ),
    };
    let steps = kit_steps_text(n);
    vec![
        (
            n.t("Impresora", "Printer").into(),
            format!("{} · {} {} mm", printer.profile.name, n.t("boquilla de", "nozzle"), printer.nozzle_mm),
        ),
        (
            n.t("Detalle de las uniones", "Joint detail").into(),
            format!("{:.2} mm ({} {:.2} mm)", n.kit.voxel_mm, n.t("pedido", "asked"), p.voxel_mm),
        ),
        (n.t("Profundidad de cada color", "Depth of each colour").into(), depth),
        (
            n.t("Piezas mínimas", "Smallest pieces").into(),
            match l {
                Lang::Es => format!("{:.0} mm³; las menores se unen a su vecina", p.min_piece_mm3),
                Lang::En => format!("{:.0} mm³; smaller ones join their neighbour", p.min_piece_mm3),
            },
        ),
        (n.t("Placas", "Plates").into(), plates),
        (n.t("Armado", "Assembly").into(), steps),
        (n.t("Densidad del filamento", "Filament density").into(), format!("{:.2} g/cm³", n.info.density_g_cm3)),
    ]
}

fn kit_steps_text(n: &Names) -> String {
    let (kit, l) = (n.kit, n.lang);
    let total = kit.steps.len() + kit.groups.len().saturating_sub(1);
    match (l, kit.groups.len()) {
        (_, 1) => l.steps(total),
        (Lang::Es, g) => format!("{} en {g} subconjuntos", l.steps(total)),
        (Lang::En, g) => format!("{} in {g} sub-assemblies", l.steps(total)),
    }
}

fn summary_html(n: &Names) -> String {
    let (kit, info, labels, l) = (n.kit, n.info, &n.labels, n.lang);
    let mut h = String::with_capacity(1 << 15);
    let total: f32 = kit.pieces.iter().map(|p| p.volume_mm3).sum();
    let three_mf = escape(&n.three_mf);
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"{code}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} · {name}</title><style>{CSS}</style></head><body><main>\
<h1>{title} · {name}</h1>\
<div class=\"meta\"><span>{np}</span><span>{nc}</span><span>{pl}</span><span>{g:.0} {grams}</span></div>\
<p class=\"lead\">{lead}</p>",
        code = l.code(),
        title = n.t("Resumen del kit", "Kit summary"),
        name = escape(info.name),
        np = l.pieces(kit.pieces.len()),
        nc = l.colours(n.slots().len()),
        pl = l.plates(kit.plates.len()),
        g = n.grams(total),
        grams = n.t("g de filamento", "g of filament"),
        lead = match (l, n.has().guide, n.has().text) {
            (Lang::Es, true, _) => format!("{}; luego sigue la <a href=\"{}\">guía de armado</a>.", n.how_to_print(true), n.f.guide),
            (Lang::En, true, _) => format!("{}; then follow the <a href=\"{}\">assembly guide</a>.", n.how_to_print(true), n.f.guide),
            (Lang::Es, false, true) => format!("{}; luego sigue los pasos de <a href=\"{}\">{}</a>.", n.how_to_print(true), n.f.guide_txt, n.f.guide_txt),
            (Lang::En, false, true) => format!("{}; then follow the steps in <a href=\"{}\">{}</a>.", n.how_to_print(true), n.f.guide_txt, n.f.guide_txt),
            (_, false, false) => format!("{}.", n.how_to_print(true)),
        },
    );

    let review = n.review();
    let notes = n.notes();
    let _ = write!(h, "<h2>{}</h2>", n.t("Qué revisar", "What to check"));
    if review.is_empty() && notes.is_empty() {
        let _ = write!(
            h,
            "<p>{}</p>",
            n.t(
                "Nada: todas las piezas están cerradas, caben en la placa y no son demasiado finas para la boquilla.",
                "Nothing: every piece is closed, fits the plate and is not too thin for the nozzle."
            )
        );
    }
    if !review.is_empty() {
        let _ = write!(
            h,
            "<table><thead><tr><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            n.t("Pieza", "Piece"),
            n.t("Color", "Colour"),
            n.t("Motivo", "Why")
        );
        for (i, issues) in &review {
            let why: Vec<String> = issues.iter().map(|x| escape(&issue_text(l, x))).collect();
            let _ = write!(
                h,
                "<tr><td>{p}</td><td>{c}</td><td>{w}</td></tr>",
                p = n.guide_link(&labels[*i], &labels[*i], " class=\"mono\""),
                c = n.piece_colour(*i),
                w = why.join("; ")
            );
        }
        h.push_str("</tbody></table>");
    }
    for note in &notes {
        let _ = write!(h, "<p class=\"warn\">{}</p>", escape(note));
    }

    let _ = write!(
        h,
        "<h2>{}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
        n.t("Colores", "Colours"),
        n.t("Ranura", "Slot"),
        n.t("Filamento", "Filament"),
        n.t("Piezas", "Pieces"),
        n.t("Placas", "Plates"),
        n.t("Volumen", "Volume"),
        n.t("Peso", "Weight")
    );
    for (s, pieces, plates, mm3) in colour_rows(n) {
        let hex = n.hex(s);
        let _ = write!(
            h,
            "<tr><td>{f}</td><td>{sw}{name} <span class=\"mono\">{hex}</span></td><td>{pieces}</td><td>{plates}</td><td>{cm3:.1} cm³</td><td>{g:.0} g</td></tr>",
            f = s + 1,
            sw = swatch(&hex),
            name = escape(&n.filament(s)),
            cm3 = mm3 / 1000.0,
            g = n.grams(mm3),
        );
    }
    let _ = write!(
        h,
        "</tbody></table><p><small>{}</small></p>",
        n.t(
            "Volumen y peso de las piezas, sin soportes, relleno ni purga.",
            "Volume and weight of the pieces, without supports, infill or purging."
        )
    );

    let _ = write!(
        h,
        "<h2>{}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
        n.t("Placas", "Plates"),
        n.t("Placa", "Plate"),
        n.t("Ocupa", "Fills"),
        n.t("Piezas", "Pieces")
    );
    for (k, plate) in kit.plates.iter().enumerate() {
        let list: Vec<&str> = plate.pieces.iter().map(|&i| labels[i].as_str()).collect();
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{:.0} %</td><td class=\"mono\">{}</td></tr>",
            n.guide_link(&n.plate_anchor(k), &escape(&n.plate_name(k)), ""),
            plate.occupancy * 100.0,
            list.join(" ")
        );
    }
    h.push_str("</tbody></table>");

    let _ = write!(h, "<h2>{}</h2><table><tbody>", n.t("Cómo se cortó", "How it was cut"));
    for (k, v) in params_text(n) {
        let _ = write!(h, "<tr><th>{}</th><td>{}</td></tr>", escape(&k), escape(&v));
    }
    h.push_str("</tbody></table>");

    let c = n.has();
    let mut items: Vec<String> = vec![];
    if c.guide {
        items.push(match l {
            Lang::Es => format!(
                "<a href=\"{0}\">{0}</a>: imágenes de cada pieza, placa y paso, con casillas para marcar lo hecho.",
                n.f.guide
            ),
            Lang::En => format!(
                "<a href=\"{0}\">{0}</a>: pictures of every piece, plate and step, with boxes to tick what is done.",
                n.f.guide
            ),
        });
    }
    if c.text {
        items.push(match l {
            Lang::Es => format!("<a href=\"{0}\">{0}</a>: los pasos en texto.", n.f.guide_txt),
            Lang::En => format!("<a href=\"{0}\">{0}</a>: the steps as text.", n.f.guide_txt),
        });
    }
    if c.three_mf {
        items.push(match l {
            Lang::Es => format!("<a class=\"mono\" href=\"{three_mf}\">{three_mf}</a>: todas las piezas en sus placas, con su filamento."),
            Lang::En => format!("<a class=\"mono\" href=\"{three_mf}\">{three_mf}</a>: every piece on its plate, with its filament."),
        });
    }
    match c.stl {
        StlFiles::None => {}
        StlFiles::Piece => items.push(n.t("<span class=\"mono\">stl/</span>: un STL por pieza, en una carpeta por color, con el nombre de la pieza.", "<span class=\"mono\">stl/</span>: one STL per piece, in a folder per colour, named after the piece.").into()),
        StlFiles::Colour => items.push(n.t("<span class=\"mono\">stl/</span>: un STL por color, con sus piezas en su sitio.", "<span class=\"mono\">stl/</span>: one STL per colour, its pieces in place.").into()),
    }
    if c.guide {
        items.push(
            n.t(
                "<span class=\"mono\">img/</span>: las imágenes de la guía.",
                "<span class=\"mono\">img/</span>: the guide's pictures.",
            )
            .into(),
        );
    }
    let _ = write!(h, "<h2>{}</h2><ul class=\"files\">", n.t("Contenido", "Contents"));
    for li in items {
        let _ = write!(h, "<li>{li}</li>");
    }
    h.push_str("</ul></main></body></html>\n");
    h
}

fn readme_text(n: &Names) -> String {
    let (kit, info, labels, l) = (n.kit, n.info, &n.labels, n.lang);
    let total: f32 = kit.pieces.iter().map(|p| p.volume_mm3).sum();
    let follow = match (n.has().guide, n.has().text) {
        (true, true) => format!("{} ({} {})", n.f.guide, n.t("o", "or"), n.f.guide_txt),
        (true, false) => n.f.guide.to_string(),
        (false, true) => n.f.guide_txt.to_string(),
        (false, false) => n.f.summary.to_string(),
    };
    let mut t = match l {
        Lang::Es => format!(
            "Kit por colores · {}\n\n{}, {}, {}, {:.0} g de filamento.\n\n\
1. {}.\n2. Separa las piezas por número y arma el modelo con {follow}.\n   {} tiene todo esto con enlaces.\n\n",
            info.name,
            l.pieces(kit.pieces.len()),
            l.colours(n.slots().len()),
            l.plates(kit.plates.len()),
            n.grams(total),
            n.how_to_print(false),
            n.f.summary
        ),
        Lang::En => format!(
            "Colour kit · {}\n\n{}, {}, {}, {:.0} g of filament.\n\n\
1. {}.\n2. Sort the pieces by number and build the model with {follow}.\n   {} has all of this with links.\n\n",
            info.name,
            l.pieces(kit.pieces.len()),
            l.colours(n.slots().len()),
            l.plates(kit.plates.len()),
            n.grams(total),
            n.how_to_print(false),
            n.f.summary
        ),
    };
    let review = n.review();
    let notes = n.notes();
    let _ = writeln!(t, "{}", n.t("QUÉ REVISAR", "WHAT TO CHECK"));
    if review.is_empty() && notes.is_empty() {
        let _ = writeln!(
            t,
            "  {}",
            n.t(
                "Nada: todas las piezas están cerradas, caben en la placa y no son demasiado finas.",
                "Nothing: every piece is closed, fits the plate and is not too thin."
            )
        );
    }
    for (i, issues) in &review {
        let why: Vec<String> = issues.iter().map(|x| issue_text(l, x)).collect();
        let _ = writeln!(t, "  {} ({}): {}", labels[*i], n.piece_colour(*i), why.join("; "));
    }
    for note in &notes {
        let _ = writeln!(t, "  {note}");
    }
    let _ = writeln!(t, "\n{}", n.t("COLORES", "COLOURS"));
    for (s, pieces, plates, mm3) in colour_rows(n) {
        let _ = writeln!(
            t,
            "  {} {:>2}  {} {:<30} {:>10}  {:>8}  {:>7.1} cm³  {:>5.0} g",
            n.t("Ranura", "Slot"),
            s + 1,
            n.hex(s),
            n.filament(s),
            l.pieces(pieces),
            l.plates(plates),
            mm3 / 1000.0,
            n.grams(mm3)
        );
    }
    let _ = writeln!(t, "\n{}", n.t("CÓMO SE CORTÓ", "HOW IT WAS CUT"));
    for (k, v) in params_text(n) {
        let _ = writeln!(t, "  {k}: {v}");
    }
    let _ = writeln!(t, "\n{}", n.t("CONTENIDO", "CONTENTS"));
    let c = n.has();
    let rows: [(bool, &str, &str); 6] = [
        (true, n.f.summary, n.t("este resumen, con enlaces", "this summary, with links")),
        (c.guide, n.f.guide, n.t("imágenes de cada pieza, placa y paso", "pictures of every piece, plate and step")),
        (c.text, n.f.guide_txt, n.t("los pasos en texto", "the steps as text")),
        (c.three_mf, &n.three_mf, n.t("todas las piezas en sus placas", "every piece on its plate")),
        (
            c.stl != StlFiles::None,
            "stl/",
            if c.stl == StlFiles::Colour {
                n.t("un STL por color, con sus piezas en su sitio", "one STL per colour, its pieces in place")
            } else {
                n.t("un STL por pieza, en una carpeta por color", "one STL per piece, in a folder per colour")
            },
        ),
        (c.guide, "img/", n.t("las imágenes de la guía", "the guide's pictures")),
    ];
    for (_, file, what) in rows.iter().filter(|r| r.0) {
        let _ = writeln!(t, "  {file:<22}{what}");
    }
    t
}

pub fn write_kit_zip<W: Write + Seek>(out: W, kit: &Kit, info: &KitInfo) -> zip::result::ZipResult<KitSummary> {
    write_kit_zip_with(out, kit, info, &|_: Stage, _, _| true)
}

/// What the kit is busy with, for its progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Drawing the pictures.
    Pictures,
    /// Writing the files into the ZIP.
    Files,
}

/// [`write_kit_zip`], telling `progress` how far it has got (stage, done,
/// total); when `progress` answers false it stops with an `Interrupted` error.
pub fn write_kit_zip_with<W: Write + Seek>(
    out: W,
    kit: &Kit,
    info: &KitInfo,
    progress: &(dyn Fn(Stage, usize, usize) -> bool + Sync),
) -> zip::result::ZipResult<KitSummary> {
    let n = Names::new(kit, info);
    let c = n.has();
    let colours: Vec<[u8; 3]> = kit.pieces.iter().map(|p| rgb(&n.hex(p.slot))).collect();
    let total = picture_count(kit, c.three_mf, c.guide);
    let drawn = std::sync::atomic::AtomicUsize::new(0);
    let done = || progress(Stage::Pictures, drawn.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
    let pics = pictures(kit, &colours, &n.labels, &n.groups, Look::new(c.quality), c.three_mf, c.guide, &done)?;
    let paths = Paths::new(&n, &pics);

    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let stls: Vec<(String, Vec<usize>)> = {
        let mut by_file: Vec<(String, Vec<usize>)> = vec![];
        for (i, f) in n.files.iter().enumerate() {
            let Some(f) = f else { continue };
            match by_file.iter_mut().find(|b| &b.0 == f) {
                Some(b) => b.1.push(i),
                None => by_file.push((f.clone(), vec![i])),
            }
        }
        by_file
    };
    let images = paths.files(&pics).count();
    let files = 2 + usize::from(c.guide) + usize::from(c.text) + usize::from(c.three_mf) + stls.len() + images;
    let mut written = 0;
    let mut next =
        |z: &mut zip::ZipWriter<W>, name: &str, how: SimpleFileOptions, bytes: &[u8]| -> zip::result::ZipResult<()> {
            if !progress(Stage::Files, written, files) {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled").into());
            }
            z.start_file(name, how)?;
            z.write_all(bytes)?;
            written += 1;
            Ok(())
        };

    next(&mut z, n.f.readme, opts, readme_text(&n).as_bytes())?;
    next(&mut z, n.f.summary, opts, summary_html(&n).as_bytes())?;
    if c.guide {
        next(&mut z, n.f.guide, opts, guide_html(&n, &pics, &paths).as_bytes())?;
    }
    if c.text {
        next(&mut z, n.f.guide_txt, opts, guide_text(&n).as_bytes())?;
    }
    if c.three_mf {
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
        next(&mut z, &n.three_mf, stored, &buf.into_inner())?;
    }
    for (file, ids) in &stls {
        let mut stl = Vec::new();
        if c.stl == StlFiles::Piece {
            // One piece, as it prints.
            write_stl(&mut stl, &n.labels[ids[0]], &kit.placed[ids[0]].positions, &kit.pieces[ids[0]].tris)?;
        } else {
            // A colour: its pieces where they sit in the model.
            let (mut positions, mut tris) = (vec![], vec![]);
            for &i in ids {
                let base = positions.len() as u32;
                positions.extend_from_slice(&kit.pieces[i].positions);
                tris.extend(kit.pieces[i].tris.iter().map(|t| t.map(|v| v + base)));
            }
            write_stl(&mut stl, n.colour(kit.pieces[ids[0]].slot), &positions, &tris)?;
        }
        next(&mut z, file, opts, &stl)?;
    }
    // PNG is compressed already.
    for (path, png) in paths.files(&pics) {
        next(&mut z, path, stored, png)?;
    }
    progress(Stage::Files, files, files);
    z.finish()?;
    Ok(KitSummary {
        pieces: kit.pieces.len(),
        plates: kit.plates.len(),
        oversize: kit.placed.iter().filter(|p| !p.fits).count(),
        review: n.review().len(),
        merged: kit.merged,
        dropped: kit.dropped,
        pins: kit.pins.len(),
        unpinned: kit.unpinned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_core::split::{Piece, Split};
    use std::collections::HashMap;
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
            let kit = bp_core::kit::kit(
                Split { pieces: pieces(), voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() },
                bed.clone(),
            );
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
        let kit = bp_core::kit::kit(
            Split {
                pieces,
                voxel_mm: 0.5,
                merged: 3,
                dropped: 1,
                // A pin out of the base into the slab on it; no room on the sliver's joint.
                pins: vec![bp_core::PinJoin { pin: 0, socket: 1, axis: [0.0, 0.0, 1.0] }],
                unpinned: 1,
                // The sliver kept the voxel skin.
                exact_skin: Some(2),
            },
            Bed::plain(256.0, 256.0),
        );
        let palette = vec!["#202020".to_string(), "#F5F4F0".to_string()];
        let names = vec![None, Some("Bambu Lab PLA Basic Blanco".to_string())];
        let info = KitInfo {
            name: "Araña",
            palette: &palette,
            slots: &[],
            filament_names: &names,
            printer: &Printer::default(),
            params: KitSettings {
                voxel_mm: 0.5,
                depth_mm: 0.0,
                core_slot: None,
                min_piece_mm3: 30.0,
                mix: false,
                contents: Default::default(),
                pins: None,
            },
            density_g_cm3: 1.24,
            flush: Default::default(),
            lang: Lang::Es,
        };
        let mut buf = Cursor::new(Vec::new());
        let summary = write_kit_zip(&mut buf, &kit, &info).unwrap();
        assert_eq!(
            summary,
            KitSummary { pieces: 3, plates: 2, oversize: 0, review: 1, merged: 3, dropped: 1, pins: 1, unpinned: 1 }
        );
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
        assert!(
            summary_html.contains("1 espiga con su alojamiento encaja las piezas")
                && summary_html.contains("1 unión no tenía sitio para una y va solo pegada.")
                && summary_html.contains("1 pieza lleva la superficie del modelo aproximada a 0.50 mm"),
            "{summary_html}"
        );
        let readme = String::from_utf8(read(&mut zip, "LEEME.txt")).unwrap();
        assert!(readme.contains("P03 (Blanco)") && readme.contains("3 piezas, 2 colores, 2 placas"), "{readme}");
        // 5,74 cm³ of PLA weigh about 7 g.
        assert!(readme.contains(" 7 g de filamento"), "{readme}");
        assert!(guide.contains("Paso 2 / 3") && guide.contains("Pega P02 a P01"));
        assert!(guide.contains("Las espigas la guían: encájala en P01"), "{guide}");
        // Ticks are remembered per kit, and every step has one.
        assert_eq!(guide.matches("data-step=").count(), 3);
        assert!(guide.contains("data-kit=\"arana|3|3|") && guide.contains("localStorage"));
        // The pieces filter by colour and by what to check.
        assert!(
            guide.contains("data-filter=\"1\"")
                && guide.contains("data-filter=\"review\"")
                && guide.contains("data-review=\"1\"")
        );
        let txt = String::from_utf8(read(&mut zip, "guia-de-armado.txt")).unwrap();
        assert!(txt.contains("Placa 2") && txt.contains("! su lado más fino mide 0.5 mm"), "{txt}");
    }

    /// A kit of `n` cubes in a row, each glued to the next, alternating colours.
    fn row(n: usize) -> Kit {
        let pieces: Vec<Piece> = (0..n)
            .map(|i| {
                let x = i as f32 * 10.0;
                let (positions, tris) = cuboid([x, 0.0, 0.0], [x + 10.0, 10.0, 10.0]);
                let mut contacts = vec![];
                if i > 0 {
                    contacts.push((i - 1, 100.0));
                }
                if i + 1 < n {
                    contacts.push((i + 1, 100.0));
                }
                Piece { slot: (i % 2) as u8, positions, tris, volume_mm3: 1000.0, contacts }
            })
            .collect();
        bp_core::kit::kit(
            Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() },
            Bed::plain(256.0, 256.0),
        )
    }

    fn info<'a>(palette: &'a [String], lang: Lang) -> KitInfo<'a> {
        KitInfo {
            name: "Row",
            palette,
            slots: &[],
            filament_names: &[],
            printer: Box::leak(Box::new(Printer::default())),
            params: KitSettings {
                voxel_mm: 0.5,
                depth_mm: 0.0,
                core_slot: None,
                min_piece_mm3: 30.0,
                mix: false,
                contents: Default::default(),
                pins: None,
            },
            density_g_cm3: 1.24,
            flush: Default::default(),
            lang,
        }
    }

    #[test]
    fn many_pieces_come_in_sub_assemblies_with_glue_shown() {
        let kit = row(30);
        assert!(kit.groups.len() > 1);
        let palette = vec!["#D02020".to_string(), "#2050D0".to_string()];
        let mut buf = Cursor::new(Vec::new());
        write_kit_zip(&mut buf, &kit, &info(&palette, Lang::Es)).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        let guide = String::from_utf8(read(&mut zip, "guia-de-armado.html")).unwrap();
        for link in links(&guide) {
            assert!(names.contains(&link), "the guide points at {link}, which is not in the ZIP");
        }
        // A section per sub-assembly, and a step to join each one after the first.
        for g in 0..kit.groups.len() {
            assert!(guide.contains(&format!("id=\"sub-{}\"", group_name(g))));
        }
        assert!(guide.contains("id=\"final\"") && guide.contains("Une el subconjunto B"));
        assert!(names.contains(&"img/subconjunto-B.png".to_string()), "{names:?}");
        let total = kit.steps.len() + kit.groups.len() - 1;
        assert_eq!(guide.matches("data-step=").count(), total);
        // Glue is striped where the new piece goes: magenta on step 2's picture.
        let png = read(&mut zip, "img/paso-002.png");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut px = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut px).unwrap();
        let magenta = px.chunks(3).filter(|c| c[0] > 150 && c[1] < 90 && c[2] > 70 && c[2] < 170).count();
        assert!(magenta > 100, "{magenta} glue pixels");
        let txt = String::from_utf8(read(&mut zip, "guia-de-armado.txt")).unwrap();
        assert!(txt.contains("SUBCONJUNTO B") && txt.contains("UNIÓN FINAL") && txt.contains("[ ]"), "{txt}");
        // Read back, every STL is a closed surface.
        let stls: Vec<String> = names.iter().filter(|n| n.ends_with(".stl")).cloned().collect();
        assert_eq!(stls.len(), 30);
        for name in stls {
            let stl = read(&mut zip, &name);
            let count = u32::from_le_bytes(stl[80..84].try_into().unwrap()) as usize;
            assert_eq!(stl.len(), 84 + 50 * count, "{name}");
            let mut index: HashMap<[u32; 3], u32> = HashMap::new();
            let tris: Vec<[u32; 3]> = (0..count)
                .map(|t| {
                    std::array::from_fn(|v| {
                        let at = 84 + 50 * t + 12 + 12 * v;
                        let key = std::array::from_fn(|k| {
                            u32::from_le_bytes(stl[at + 4 * k..at + 4 * k + 4].try_into().unwrap())
                        });
                        let next = index.len() as u32;
                        *index.entry(key).or_insert(next)
                    })
                })
                .collect();
            assert!(bp_core::split::watertight(&tris), "{name} is not closed");
        }
    }

    fn zip_of(kit: &Kit, info: &KitInfo) -> (zip::ZipArchive<Cursor<Vec<u8>>>, Vec<String>) {
        let mut buf = Cursor::new(Vec::new());
        write_kit_zip(&mut buf, kit, info).unwrap();
        let zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names = zip.file_names().map(String::from).collect();
        (zip, names)
    }

    #[test]
    fn the_zip_holds_only_what_was_asked() {
        use crate::project_file::KitContents;
        let kit = row(6);
        let palette = vec!["#D02020".to_string(), "#2050D0".to_string()];
        let mut info = info(&palette, Lang::Es);
        info.params.contents =
            KitContents { three_mf: false, stl: StlFiles::Colour, guide: false, text: true, ..Default::default() };
        let (mut zip, names) = zip_of(&kit, &info);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, ["LEEME.txt", "guia-de-armado.txt", "resumen.html", "stl/1-rojo.stl", "stl/2-azul.stl"]);
        let summary = String::from_utf8(read(&mut zip, "resumen.html")).unwrap();
        for link in links(&summary) {
            assert!(names.contains(&link), "the summary points at {link}, which is not in the ZIP");
        }
        // A colour's STL holds its three pieces, where they sit in the model: 3 × 12 triangles.
        let stl = read(&mut zip, "stl/1-rojo.stl");
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 36);
        let readme = String::from_utf8(read(&mut zip, "LEEME.txt")).unwrap();
        assert!(
            readme.contains("Lamina los STL de stl/")
                && !readme.contains(".3mf")
                && !readme.contains("guia-de-armado.html"),
            "{readme}"
        );
    }

    #[test]
    fn sharper_pictures_weigh_more() {
        use crate::project_file::{KitContents, Quality};
        let kit = row(3);
        let palette = vec!["#D02020".to_string(), "#2050D0".to_string()];
        let size = |quality| {
            let mut info = info(&palette, Lang::Es);
            info.params.contents = KitContents { quality, ..Default::default() };
            let (mut zip, names) = zip_of(&kit, &info);
            names.iter().filter(|n| n.ends_with(".png")).map(|n| read(&mut zip, n).len()).sum::<usize>()
        };
        let (low, medium, high) = (size(Quality::Low), size(Quality::Medium), size(Quality::High));
        assert!(low < medium && medium < high, "{low} {medium} {high}");
    }

    #[test]
    fn progress_counts_up_and_cancel_stops_the_export() {
        let kit = row(5);
        let palette = vec!["#D02020".to_string(), "#2050D0".to_string()];
        let info = info(&palette, Lang::Es);
        let seen = std::sync::Mutex::new(vec![]);
        write_kit_zip_with(Cursor::new(Vec::new()), &kit, &info, &|stage, done, total| {
            assert!(done <= total);
            seen.lock().unwrap().push((stage, done, total));
            true
        })
        .unwrap();
        let seen = seen.into_inner().unwrap();
        let pictures = seen.iter().filter(|s| s.0 == Stage::Pictures).count();
        assert_eq!(pictures, picture_count(&kit, true, true));
        assert_eq!(seen.last().map(|s| (s.0, s.1 == s.2)), Some((Stage::Files, true)));
        // Cancelled halfway through the pictures: an Interrupted error, nothing written.
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let err = write_kit_zip_with(Cursor::new(Vec::new()), &kit, &info, &|_, _, _| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4
        })
        .unwrap_err();
        assert!(matches!(&err, zip::result::ZipError::Io(e) if e.kind() == std::io::ErrorKind::Interrupted), "{err}");
    }

    #[test]
    fn the_kit_speaks_english_when_asked() {
        let kit = row(4);
        let palette = vec!["#D02020".to_string(), "#2050D0".to_string()];
        let mut buf = Cursor::new(Vec::new());
        write_kit_zip(&mut buf, &kit, &info(&palette, Lang::En)).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        for f in [
            "README.txt",
            "summary.html",
            "assembly-guide.html",
            "assembly-guide.txt",
            "3mf/row_plates.3mf",
            "stl/1-red/P01.stl",
            "stl/2-blue/P02.stl",
        ] {
            assert!(names.contains(&f.to_string()), "{f} missing: {names:?}");
        }
        for page in ["assembly-guide.html", "summary.html"] {
            let html = String::from_utf8(read(&mut zip, page)).unwrap();
            assert!(html.contains("<html lang=\"en\">"));
            for link in links(&html) {
                assert!(names.contains(&link), "{page} points at {link}, which is not in the ZIP");
            }
        }
        let guide = String::from_utf8(read(&mut zip, "assembly-guide.html")).unwrap();
        assert!(guide.contains("Start with P01") && guide.contains("Glue P02 to P01") && guide.contains("Step 2 / 4"));
        let readme = String::from_utf8(read(&mut zip, "README.txt")).unwrap();
        assert!(readme.contains("4 pieces, 2 colours, 2 plates"), "{readme}");
        for spanish in ["Placa", "pieza", "Paso", "Ranura"] {
            assert!(!guide.contains(spanish) && !readme.contains(spanish), "{spanish} in the English kit");
        }
        let mut three_mf = zip::ZipArchive::new(Cursor::new(read(&mut zip, "3mf/row_plates.3mf"))).unwrap();
        let mut cfg = String::new();
        three_mf.by_name("Metadata/model_settings.config").unwrap().read_to_string(&mut cfg).unwrap();
        assert!(cfg.contains("Plate 1 · Red #D02020"), "{cfg}");
    }
}
