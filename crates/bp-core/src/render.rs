//! A small software renderer for the assembly guide: orthographic, flat
//! shaded, z-buffered, supersampled, with an outline around every piece.

use crate::geom::{self, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// Already in place: washed out so the new piece stands out.
    Faded,
    Normal,
    /// The piece being added: full colour and a heavy outline.
    Focus,
    /// Where glue goes: striped, over the faces of the piece it lies on
    /// (an item with that piece's id and only those triangles).
    Glue,
}

pub struct Item<'a> {
    pub positions: &'a [Vec3],
    pub tris: &'a [[u32; 3]],
    pub rgb: [u8; 3],
    pub id: u32,
    pub style: Style,
}

pub const PAPER: [u8; 3] = [0xF4, 0xF1, 0xE8];
const INK: [u8; 3] = [0x1F, 0x2A, 0x24];
const FADE: [u8; 3] = [0xE4, 0xE0, 0xD6];
/// Glue stripes: a magenta no filament colour is mistaken for, and its pale tint.
pub const GLUE: [u8; 3] = [0xD8, 0x1B, 0x72];
const GLUE_PALE: [u8; 3] = [0xF6, 0xC4, 0xDB];
pub const NO_ID: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    centre: Vec3,
    scale: f32,
    offset: [f32; 2],
    pub width: usize,
    pub height: usize,
}

impl Camera {
    /// Looks at the box `lo..hi` from `azimuth` degrees around Z (0 = from the
    /// front, -Y) and `elevation` degrees above the horizon, framing it with a margin.
    pub fn new(azimuth: f32, elevation: f32, lo: Vec3, hi: Vec3, width: usize, height: usize) -> Self {
        let (a, e) = (azimuth.to_radians(), elevation.clamp(-89.0, 89.0).to_radians());
        let eye = [e.cos() * a.sin(), -e.cos() * a.cos(), e.sin()];
        let right = geom::normalize(geom::cross([0.0, 0.0, 1.0], eye));
        let up = geom::cross(eye, right);
        let centre = geom::scale(geom::add(lo, hi), 0.5);
        let mut cam = Self { eye, right, up, centre, scale: 1.0, offset: [0.0; 2], width, height };
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for b in 0..8 {
            let p = [[lo[0], hi[0]][b & 1], [lo[1], hi[1]][(b >> 1) & 1], [lo[2], hi[2]][b >> 2]];
            let d = geom::sub(p, centre);
            let s = [geom::dot(d, right), geom::dot(d, up)];
            min = [min[0].min(s[0]), min[1].min(s[1])];
            max = [max[0].max(s[0]), max[1].max(s[1])];
        }
        let margin = 0.08;
        cam.scale = ((width as f32 * (1.0 - 2.0 * margin)) / (max[0] - min[0]).max(1e-3))
            .min((height as f32 * (1.0 - 2.0 * margin)) / (max[1] - min[1]).max(1e-3));
        cam.offset = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
        cam
    }

    /// Pixel position and depth (larger is nearer) of a point.
    pub fn project(&self, p: Vec3) -> [f32; 3] {
        let d = geom::sub(p, self.centre);
        [
            self.width as f32 * 0.5 + (geom::dot(d, self.right) - self.offset[0]) * self.scale,
            self.height as f32 * 0.5 - (geom::dot(d, self.up) - self.offset[1]) * self.scale,
            geom::dot(d, self.eye),
        ]
    }

    fn supersampled(&self, k: usize) -> Self {
        Self { scale: self.scale * k as f32, width: self.width * k, height: self.height * k, ..*self }
    }
}

pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// RGB, row by row.
    pub rgb: Vec<u8>,
    /// Item id under each pixel, `NO_ID` for background.
    pub ids: Vec<u32>,
}

impl Frame {
    /// How many pixels show item `id`.
    pub fn count(&self, id: u32) -> usize {
        self.ids.iter().filter(|&&i| i == id).count()
    }

    /// Box around the visible pixels of item `id`: left, top, right, bottom as
    /// fractions of width and height.
    pub fn bbox(&self, id: u32) -> Option<[f32; 4]> {
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        for (k, _) in self.ids.iter().enumerate().filter(|(_, &i)| i == id) {
            let (x, y) = (k % self.width, k / self.width);
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x + 1), hi[1].max(y + 1)];
        }
        (lo[0] != usize::MAX).then(|| {
            let (w, h) = (self.width as f32, self.height as f32);
            [lo[0] as f32 / w, lo[1] as f32 / h, hi[0] as f32 / w, hi[1] as f32 / h]
        })
    }

    /// Centre of the visible pixels of item `id`, as fractions of width and height.
    pub fn anchor(&self, id: u32) -> Option<[f32; 2]> {
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0usize);
        for (k, &i) in self.ids.iter().enumerate() {
            if i == id {
                sx += (k % self.width) as f64;
                sy += (k / self.width) as f64;
                n += 1;
            }
        }
        (n > 0).then(|| {
            [((sx / n as f64 + 0.5) / self.width as f64) as f32, ((sy / n as f64 + 0.5) / self.height as f64) as f32]
        })
    }
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] as f32 * (1.0 - t) + b[i] as f32 * t)
}

fn float(c: [u8; 3]) -> [f32; 3] {
    c.map(f32::from)
}

/// Renders `items` at `ssaa`× resolution and averages down to the camera's size.
pub fn render(cam: &Camera, items: &[Item], ssaa: usize) -> Frame {
    let k = ssaa.max(1);
    let hi = cam.supersampled(k);
    let (w, h) = (hi.width, hi.height);
    let mut depth = vec![f32::MIN; w * h];
    let mut colour = vec![[0.0f32; 3]; w * h];
    let mut ids = vec![NO_ID; w * h];
    let mut style = vec![Style::Normal; w * h];
    let light = geom::normalize(geom::add(
        geom::add(geom::scale(cam.eye, 0.75), geom::scale(cam.up, 0.55)),
        geom::scale(cam.right, -0.35),
    ));

    for item in items {
        let base = match item.style {
            Style::Faded => mix(item.rgb, FADE, 0.72),
            // Lift very dark filaments a little so their shading still reads.
            _ => mix(item.rgb, [255; 3], 0.14),
        };
        let screen: Vec<[f32; 3]> = item.positions.iter().map(|&p| hi.project(p)).collect();
        for t in item.tris {
            let c = t.map(|v| item.positions[v as usize]);
            let n = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
            if geom::dot(n, cam.eye) <= 0.0 {
                continue;
            }
            let lit = 0.5 + 0.5 * geom::dot(n, light).max(0.0);
            let shade = base.map(|x| (x * lit).min(255.0));
            let glue = item.style == Style::Glue;
            let stripe = |x: usize, y: usize| {
                let c = if ((x + y) / (3 * k)).is_multiple_of(2) { GLUE } else { GLUE_PALE };
                float(c).map(|v| (v * (0.8 + 0.2 * lit)).min(255.0))
            };
            let [a, b, cc] = t.map(|v| screen[v as usize]);
            let area = (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = a[0].min(b[0]).min(cc[0]).floor().max(0.0) as usize;
            let x1 = (a[0].max(b[0]).max(cc[0]).ceil() as usize).min(w.saturating_sub(1));
            let y0 = a[1].min(b[1]).min(cc[1]).floor().max(0.0) as usize;
            let y1 = (a[1].max(b[1]).max(cc[1]).ceil() as usize).min(h.saturating_sub(1));
            if x0 > x1 || y0 > y1 || a[0].max(b[0]).max(cc[0]) < 0.0 || a[1].max(b[1]).max(cc[1]) < 0.0 {
                continue;
            }
            for y in y0..=y1 {
                let py = y as f32 + 0.5;
                for x in x0..=x1 {
                    let px = x as f32 + 0.5;
                    let w0 = ((b[0] - px) * (cc[1] - py) - (b[1] - py) * (cc[0] - px)) / area;
                    let w1 = ((cc[0] - px) * (a[1] - py) - (cc[1] - py) * (a[0] - px)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a[2] + w1 * b[2] + w2 * cc[2];
                    let i = y * w + x;
                    // Glue lies on faces already drawn, at their depth.
                    if z > depth[i] || (glue && z >= depth[i] - 1e-3) {
                        depth[i] = z;
                        colour[i] = if glue { stripe(x, y) } else { shade };
                        ids[i] = item.id;
                        style[i] = item.style;
                    }
                }
            }
        }
    }

    // Outlines: where the id changes, thicker around the focused piece.
    let mut out = colour.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let reach = if style[i] == Style::Focus { 2 * k } else { k.div_ceil(2) };
            let mut edge = false;
            'scan: for dy in -(reach as isize)..=reach as isize {
                for dx in -(reach as isize)..=reach as isize {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if ids[j] != ids[i] && (ids[i] != NO_ID || style[j] == Style::Focus) {
                        edge = true;
                        break 'scan;
                    }
                }
            }
            if edge {
                // Strong around the focused piece and the silhouette, light between pieces.
                let strong = style[i] == Style::Focus || ids[i] == NO_ID;
                out[i] = if strong { float(INK) } else { mix(INK, PAPER, 0.45) };
            } else if ids[i] == NO_ID {
                out[i] = float(PAPER);
            }
        }
    }

    // Average each k × k block.
    let (fw, fh) = (cam.width, cam.height);
    let mut rgb = vec![0u8; fw * fh * 3];
    let mut fids = vec![NO_ID; fw * fh];
    for y in 0..fh {
        for x in 0..fw {
            let mut sum = [0.0f32; 3];
            for sy in 0..k {
                for sx in 0..k {
                    let c = out[(y * k + sy) * w + x * k + sx];
                    sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                }
            }
            let n = (k * k) as f32;
            for (c, s) in rgb[(y * fw + x) * 3..][..3].iter_mut().zip(sum) {
                *c = (s / n).round().clamp(0.0, 255.0) as u8;
            }
            fids[y * fw + x] = ids[(y * k + k / 2) * w + x * k + k / 2];
        }
    }
    Frame { width: fw, height: fh, rgb, ids: fids }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::tests::cuboid;

    #[test]
    fn draws_a_cube_in_the_middle_with_its_id() {
        let (p, t) = cuboid([0.0; 3], [10.0; 3]);
        let cam = Camera::new(35.0, 25.0, [0.0; 3], [10.0; 3], 80, 60);
        let item = Item { positions: &p, tris: &t, rgb: [200, 40, 40], id: 7, style: Style::Normal };
        let f = render(&cam, &[item], 2);
        assert_eq!(f.rgb.len(), 80 * 60 * 3);
        assert_eq!(f.ids[30 * 80 + 40], 7);
        assert_eq!(f.ids[0], NO_ID);
        let a = f.anchor(7).unwrap();
        let b = f.bbox(7).unwrap();
        assert!(b[0] < a[0] && a[0] < b[2] && b[1] < a[1] && a[1] < b[3]);
        assert!((a[0] - 0.5).abs() < 0.1 && (a[1] - 0.5).abs() < 0.1);
        // Corners stay paper.
        assert_eq!(&f.rgb[..3], &PAPER);
    }

    #[test]
    fn glue_shows_over_the_faces_it_marks() {
        let (p, t) = cuboid([0.0; 3], [10.0; 3]);
        let cam = Camera::new(35.0, 25.0, [0.0; 3], [10.0; 3], 80, 60);
        // The top face (+Z).
        let top: Vec<[u32; 3]> =
            t.iter().copied().filter(|tri| tri.iter().all(|&v| p[v as usize][2] == 10.0)).collect();
        assert_eq!(top.len(), 2);
        let items = [
            Item { positions: &p, tris: &t, rgb: [40, 160, 40], id: 7, style: Style::Faded },
            Item { positions: &p, tris: &top, rgb: [0; 3], id: 7, style: Style::Glue },
        ];
        let f = render(&cam, &items, 1);
        let magenta = f.rgb.chunks(3).filter(|c| c[0] > 150 && c[1] < 120 && c[2] > 60).count();
        assert!(magenta > 50, "{magenta}");
        // Same piece: no outline between the glue and the faces around it.
        assert_eq!(f.count(7), f.ids.iter().filter(|&&i| i != NO_ID).count());
    }

    #[test]
    fn nearer_piece_hides_the_one_behind() {
        let (p, t) = cuboid([0.0; 3], [10.0; 3]);
        let back: Vec<Vec3> = p.iter().map(|v| [v[0], v[1] + 20.0, v[2]]).collect();
        let cam = Camera::new(0.0, 0.0, [0.0; 3], [10.0, 30.0, 10.0], 60, 60);
        let items = [
            Item { positions: &back, tris: &t, rgb: [0, 0, 255], id: 1, style: Style::Normal },
            Item { positions: &p, tris: &t, rgb: [255, 0, 0], id: 2, style: Style::Normal },
        ];
        let f = render(&cam, &items, 1);
        assert_eq!(f.count(1), 0);
        assert!(f.count(2) > 100);
    }
}
