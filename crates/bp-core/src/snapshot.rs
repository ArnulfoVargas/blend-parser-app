//! The open document as bytes, to save a project and open it again without
//! Blender: the extracted scene and every setting, hand painting and bucket
//! region on it. What can be worked out again (the working mesh, the painting
//! trees, the picker) is not stored; undo history is not kept.
//!
//! Little-endian, with a magic and a version up front. A newer version than
//! this build knows is refused rather than read wrong.

use crate::color::Rgb;
use crate::mesh::MeshOps;
use crate::project::{ColorOps, PrintOps};
use crate::scene::{ColorSource, Image, MaterialInfo, Part, Scene};

const MAGIC: &[u8; 4] = b"BPPJ";
pub const VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a project, or cut short.
    Corrupt,
    /// Saved by a newer BlendParser.
    Version(u32),
}

pub(crate) struct Writer(pub Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        let mut w = Self(Vec::new());
        w.0.extend(MAGIC);
        w.u32(VERSION);
        w
    }
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn f32s<const N: usize>(&mut self, v: [f32; N]) {
        v.into_iter().for_each(|x| self.f32(x));
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.len(b.len());
        self.0.extend(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    pub fn vec<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) {
        self.len(items.len());
        items.iter().for_each(|x| f(self, x));
    }
}

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

type R<T> = Result<T, SnapshotError>;

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> R<Self> {
        let mut r = Self { buf, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(SnapshotError::Corrupt);
        }
        match r.u32()? {
            v if v > VERSION => Err(SnapshotError::Version(v)),
            _ => Ok(r),
        }
    }
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or(SnapshotError::Corrupt)?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> R<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }
    /// A count, checked against what is left so a corrupt one cannot ask for gigabytes.
    pub fn len(&mut self) -> R<usize> {
        let n = self.u32()? as usize;
        if n > self.buf.len() - self.at {
            return Err(SnapshotError::Corrupt);
        }
        Ok(n)
    }
    pub fn f32(&mut self) -> R<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }
    pub fn f32s<const N: usize>(&mut self) -> R<[f32; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
    pub fn bytes(&mut self) -> R<Vec<u8>> {
        let n = self.len()?;
        Ok(self.take(n)?.to_vec())
    }
    pub fn str(&mut self) -> R<String> {
        String::from_utf8(self.bytes()?).map_err(|_| SnapshotError::Corrupt)
    }
    pub fn vec<T>(&mut self, mut f: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let n = self.len()?;
        (0..n).map(|_| f(self)).collect()
    }
    pub fn done(&self) -> bool {
        self.at == self.buf.len()
    }
}

pub(crate) fn write_scene(w: &mut Writer, s: &Scene) {
    w.vec(&s.images, |w, i| {
        w.str(&i.name);
        w.u32(i.width);
        w.u32(i.height);
        w.bytes(&i.rgb);
    });
    w.vec(&s.materials, |w, m| {
        w.str(&m.name);
        w.str(&m.kind);
        w.str(&m.detail);
        w.u32(m.faces);
    });
    w.vec(&s.parts, |w, p| {
        w.str(&p.name);
        w.vec(&p.positions, |w, v| w.f32s(*v));
        w.vec(&p.tris, |w, t| t.iter().for_each(|&i| w.u32(i)));
        w.vec(&p.tri_loops, |w, t| t.iter().for_each(|&i| w.u32(i)));
        w.vec(&p.tri_source, |w, &s| w.u32(s as u32));
        w.vec(&p.sources, |w, s| match *s {
            ColorSource::Flat(c) => {
                w.u8(0);
                w.f32s(c);
            }
            ColorSource::Image { image, uv } => {
                w.u8(1);
                w.u32(image);
                w.u32(uv);
            }
            ColorSource::VertexColor { attr } => {
                w.u8(2);
                w.u32(attr);
            }
        });
        w.vec(&p.uv_layers, |w, l| w.vec(l, |w, uv| w.f32s(*uv)));
        w.vec(&p.color_layers, |w, l| w.vec(l, |w, c| w.f32s(*c)));
        w.u32(p.object);
    });
}

fn index3(r: &mut Reader) -> R<[u32; 3]> {
    Ok([r.u32()?, r.u32()?, r.u32()?])
}

pub(crate) fn read_scene(r: &mut Reader) -> R<Scene> {
    let images = r.vec(|r| {
        let (name, width, height, rgb) = (r.str()?, r.u32()?, r.u32()?, r.bytes()?);
        if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
            return Err(SnapshotError::Corrupt);
        }
        Ok(Image { name, width, height, rgb })
    })?;
    let materials =
        r.vec(|r| Ok(MaterialInfo { name: r.str()?, kind: r.str()?, detail: r.str()?, faces: r.u32()? }))?;
    let parts = r.vec(|r| {
        let name = r.str()?;
        let positions = r.vec(|r| r.f32s())?;
        let tris = r.vec(index3)?;
        let tri_loops = r.vec(index3)?;
        let tri_source = r.vec(|r| Ok(r.u32()? as u16))?;
        let sources = r.vec(|r| match r.u8()? {
            0 => Ok(ColorSource::Flat(r.f32s()?)),
            1 => Ok(ColorSource::Image { image: r.u32()?, uv: r.u32()? }),
            2 => Ok(ColorSource::VertexColor { attr: r.u32()? }),
            _ => Err(SnapshotError::Corrupt),
        })?;
        let uv_layers = r.vec(|r| r.vec(|r| r.f32s()))?;
        let color_layers = r.vec(|r| r.vec(|r| r.f32s()))?;
        let part =
            Part { name, positions, tris, tri_loops, tri_source, sources, uv_layers, color_layers, object: r.u32()? };
        check_part(&part, images.len())?;
        Ok(part)
    })?;
    Ok(Scene { parts, images, materials })
}

/// Every index points inside what it indexes, so sampling the scene cannot panic.
fn check_part(p: &Part, images: usize) -> R<()> {
    let loops = p.uv_layers.iter().map(Vec::len).chain(p.color_layers.iter().map(Vec::len)).min();
    let ok = p.tris.iter().flatten().all(|&i| (i as usize) < p.positions.len())
        && p.tri_loops.len() == p.tris.len()
        && p.tri_source.len() == p.tris.len()
        && p.tri_loops.iter().flatten().all(|&l| loops.is_none_or(|n| (l as usize) < n))
        && p.sources.iter().all(|s| match *s {
            ColorSource::Flat(_) => true,
            ColorSource::Image { image, uv } => (image as usize) < images && (uv as usize) < p.uv_layers.len(),
            ColorSource::VertexColor { attr } => (attr as usize) < p.color_layers.len(),
        });
    if ok {
        Ok(())
    } else {
        Err(SnapshotError::Corrupt)
    }
}

pub(crate) fn write_mesh_ops(w: &mut Writer, o: &MeshOps) {
    w.f32(o.keep_ratio);
    w.f32(o.flatten_mm);
    w.bool(o.repair);
    w.f32(o.min_piece_mm);
    w.bool(o.orient);
    w.bool(o.fill_holes);
    w.bool(o.split_vertices);
    w.bool(o.drop_floating);
    w.bool(o.deep_clean);
}

pub(crate) fn read_mesh_ops(r: &mut Reader) -> R<MeshOps> {
    Ok(MeshOps {
        keep_ratio: r.f32()?,
        flatten_mm: r.f32()?,
        repair: r.bool()?,
        min_piece_mm: r.f32()?,
        orient: r.bool()?,
        fill_holes: r.bool()?,
        split_vertices: r.bool()?,
        drop_floating: r.bool()?,
        deep_clean: r.bool()?,
    })
}

pub(crate) fn write_ops(w: &mut Writer, c: &ColorOps, p: &PrintOps) {
    w.u8(c.border_detail);
    w.f32(c.min_island_mm2);
    w.f32(p.nozzle_mm);
    w.f32(p.layer_mm);
}

pub(crate) fn read_ops(r: &mut Reader) -> R<(ColorOps, PrintOps)> {
    Ok((
        ColorOps { border_detail: r.u8()?, min_island_mm2: r.f32()? },
        PrintOps { nozzle_mm: r.f32()?, layer_mm: r.f32()? },
    ))
}

pub(crate) fn write_rgb(w: &mut Writer, c: &Rgb) {
    w.f32s(*c);
}
