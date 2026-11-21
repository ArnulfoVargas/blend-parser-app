//! The source scene as extracted from a `.blend`: geometry in millimetres plus
//! everything needed to know the colour at any point of the surface.

use crate::color::{Rgb, Sample};
use crate::geom::{self, Bary, Vec3};

/// An 8-bit sRGB texture, rows bottom-up (Blender's pixel order).
#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    /// Nearest texel with repeat wrapping; `v = 0` is the bottom row.
    #[inline]
    pub fn texel(&self, u: f32, v: f32) -> [u8; 3] {
        let (w, h) = (self.width as usize, self.height as usize);
        let x = ((u.rem_euclid(1.0) * w as f32) as usize).min(w - 1);
        let y = ((v.rem_euclid(1.0) * h as f32) as usize).min(h - 1);
        let i = (y * w + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// Where a material slot gets its base colour from.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorSource {
    /// A plain Base Color.
    Flat(Rgb),
    /// An image texture sampled through one of the part's UV layers.
    Image { image: u32, uv: u32 },
    /// A colour attribute (vertex colour), interpolated across the triangle.
    VertexColor { attr: u32 },
}

/// A material as shown to the user.
#[derive(Clone, Debug)]
pub struct MaterialInfo {
    pub name: String,
    /// `flat`, `image` or `vertex`.
    pub kind: String,
    /// What the UI words by kind: `WxH` pixels, the colour layer's name, or `#RRGGBB`.
    pub detail: String,
    pub faces: u32,
}

/// One Blender object, triangulated, in world millimetres.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// Loop (face corner) index of each triangle corner, for UVs and colour attributes.
    pub tri_loops: Vec<[u32; 3]>,
    /// Index into `sources` for each triangle (its material slot).
    pub tri_source: Vec<u16>,
    pub sources: Vec<ColorSource>,
    /// UV layers, one `[u, v]` per loop.
    pub uv_layers: Vec<Vec<[f32; 2]>>,
    /// Colour attributes, one sRGB colour per loop.
    pub color_layers: Vec<Vec<Rgb>>,
    /// The printed object the part belongs to: 0 for the model, more for pieces cut off it.
    pub object: u32,
    /// The plate its object prints on, 0 for the first; more once a model is split across plates.
    pub plate: u32,
}

impl Part {
    #[inline]
    pub fn corners(&self, tri: usize) -> [Vec3; 3] {
        let t = self.tris[tri];
        [self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]]
    }

    /// The colour at barycentric `b` of triangle `tri`.
    pub fn sample(&self, images: &[Image], tri: usize, b: Bary) -> Sample {
        let src = self.sources.get(self.tri_source[tri] as usize).unwrap_or(&ColorSource::Flat([0.8, 0.8, 0.8]));
        let l = self.tri_loops[tri];
        match *src {
            ColorSource::Flat(c) => Sample::Float(c),
            ColorSource::Image { image, uv } => {
                let uv = &self.uv_layers[uv as usize];
                let (a, bb, c) = (uv[l[0] as usize], uv[l[1] as usize], uv[l[2] as usize]);
                let u = a[0] * b[0] + bb[0] * b[1] + c[0] * b[2];
                let v = a[1] * b[0] + bb[1] * b[1] + c[1] * b[2];
                Sample::Byte(images[image as usize].texel(u, v))
            }
            ColorSource::VertexColor { attr } => {
                let col = &self.color_layers[attr as usize];
                let (a, bb, c) = (col[l[0] as usize], col[l[1] as usize], col[l[2] as usize]);
                Sample::Float(std::array::from_fn(|i| a[i] * b[0] + bb[i] * b[1] + c[i] * b[2]))
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub parts: Vec<Part>,
    pub images: Vec<Image>,
    pub materials: Vec<MaterialInfo>,
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.tris.len()).sum()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.parts.iter().flat_map(|p| p.positions.iter().copied());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        }))
    }

    /// Moves the scene so XY is centred on the origin and the lowest point sits
    /// on Z = 0; split across plates, the first plate's parts are centred and
    /// the rest keep their places beside it. Returns how far it moved.
    pub fn place_on_plate(&mut self) -> Vec3 {
        let Some((all_lo, all_hi)) = self.bounds() else { return [0.0; 3] };
        let first = self.parts.iter().filter(|p| p.plate == 0).flat_map(|p| p.positions.iter().copied());
        let (lo, hi) = first.fold((all_hi, all_lo), |(a, b), p| {
            (std::array::from_fn(|i| a[i].min(p[i])), std::array::from_fn(|i| b[i].max(p[i])))
        });
        let (lo, hi) = if lo[0] <= hi[0] { (lo, hi) } else { (all_lo, all_hi) };
        let shift = [-(lo[0] + hi[0]) * 0.5, -(lo[1] + hi[1]) * 0.5, -all_lo[2]];
        for p in &mut self.parts {
            for v in &mut p.positions {
                *v = geom::add(*v, shift);
            }
        }
        shift
    }
}
