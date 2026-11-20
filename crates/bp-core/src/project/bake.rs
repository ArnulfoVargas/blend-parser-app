//! The geometry the cutting tools work on: each source part with the clean-up
//! a cut needs already applied, so open seams, repeated or collapsed faces and
//! stray fins in the file do not stop a cut, a split or connectors.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use super::Project;
use crate::mesh::{Mesh, MeshOps, MeshPart};
use crate::scene::{Part, Scene};

/// A source part as the cutting tools see it, in source units (before the
/// stretch), and for each of its triangles the source triangle it lies in.
#[derive(Clone, Debug)]
pub(crate) struct CutSource {
    pub part: Part,
    pub from: Vec<u32>,
}

/// Shared between a project and the copies the previews make: the key says
/// whether the stored parts are still the project's.
pub(crate) type CutSourceCache = Arc<Mutex<Option<(u64, Arc<Vec<CutSource>>)>>>;

impl Project {
    /// Clean-up a cut needs: welded seams, no collapsed or repeated faces,
    /// no stray fins, holes capped and faces wound the same way. The user's
    /// own clean-up settings come on top (loose pieces, split fans).
    fn cut_ops(&self) -> MeshOps {
        MeshOps {
            repair: true,
            deep_clean: true,
            fill_holes: true,
            orient: true,
            split_vertices: self.mesh_ops.split_vertices,
            min_piece_mm: self.mesh_ops.min_piece_mm,
            ..MeshOps::default()
        }
    }

    fn cut_source_key(&self, ops: &MeshOps) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{ops:?}").hash(&mut h);
        self.scale.to_bits().hash(&mut h);
        for (part, src) in self.scene.parts.iter().zip(&self.source_positions) {
            (part.tris.len(), src.len(), part.object, part.plate).hash(&mut h);
            // A turn, a move or a cut changes these.
            for i in [0, src.len() / 3, src.len() / 2, src.len().saturating_sub(1)] {
                if let Some(v) = src.get(i) {
                    v.map(f32::to_bits).hash(&mut h);
                }
            }
        }
        h.finish()
    }

    /// Every source part ready to be cut. Cached while the scene stays the same.
    pub(crate) fn cut_sources(&self) -> Arc<Vec<CutSource>> {
        let ops = self.cut_ops();
        let key = self.cut_source_key(&ops);
        if let Ok(guard) = self.cut_cache.lock() {
            if let Some((k, v)) = guard.as_ref() {
                if *k == key {
                    return v.clone();
                }
            }
        }
        let scene = Scene {
            parts: self
                .scene
                .parts
                .iter()
                .zip(&self.source_positions)
                .map(|(p, src)| Part { positions: src.clone(), ..p.clone() })
                .collect(),
            images: vec![],
            materials: vec![],
        };
        let (shape, _) = Mesh::build(&scene, ops, self.scale);
        let baked: Vec<CutSource> = scene
            .parts
            .iter()
            .zip(&shape.parts)
            .map(|(source, cleaned)| bake_part(source, cleaned))
            .collect();
        let baked = Arc::new(baked);
        if let Ok(mut guard) = self.cut_cache.lock() {
            *guard = Some((key, baked.clone()));
        }
        baked
    }
}

/// `cleaned` (from [`Mesh::build`] on `source`) as a part of its own: each
/// corner takes the UVs and colours the source has at that point, and each
/// triangle its material. A part whose link to the source was lost, or that
/// lost every triangle, stays as it was.
pub(crate) fn bake_part(source: &Part, cleaned: &MeshPart) -> CutSource {
    let unchanged = || CutSource { part: source.clone(), from: (0..source.tris.len() as u32).collect() };
    let Some(origin) = cleaned.origin.as_ref().filter(|o| o.len() == cleaned.tris.len()) else {
        return unchanged();
    };
    if cleaned.tris.is_empty() || origin.iter().any(|(src, _)| *src as usize >= source.tris.len()) {
        return unchanged();
    }
    let mut uv_layers: Vec<Vec<[f32; 2]>> = source.uv_layers.iter().map(|_| Vec::with_capacity(cleaned.tris.len() * 3)).collect();
    let mut color_layers: Vec<Vec<[f32; 3]>> =
        source.color_layers.iter().map(|_| Vec::with_capacity(cleaned.tris.len() * 3)).collect();
    let mut tri_loops = Vec::with_capacity(cleaned.tris.len());
    let mut tri_source = Vec::with_capacity(cleaned.tris.len());
    let mut loops = 0u32;
    for &(src, bary) in origin {
        let l = source.tri_loops.get(src as usize).copied().unwrap_or([0; 3]);
        let mut corners = [0u32; 3];
        for (k, b) in bary.iter().enumerate() {
            for (out, layer) in uv_layers.iter_mut().zip(&source.uv_layers) {
                let at = |i: usize| layer.get(l[i] as usize).copied().unwrap_or([0.0; 2]);
                let (a, bb, c) = (at(0), at(1), at(2));
                out.push([a[0] * b[0] + bb[0] * b[1] + c[0] * b[2], a[1] * b[0] + bb[1] * b[1] + c[1] * b[2]]);
            }
            for (out, layer) in color_layers.iter_mut().zip(&source.color_layers) {
                let at = |i: usize| layer.get(l[i] as usize).copied().unwrap_or([0.8; 3]);
                let (a, bb, c) = (at(0), at(1), at(2));
                out.push(std::array::from_fn(|i| a[i] * b[0] + bb[i] * b[1] + c[i] * b[2]));
            }
            corners[k] = loops;
            loops += 1;
        }
        tri_loops.push(corners);
        tri_source.push(source.tri_source.get(src as usize).copied().unwrap_or(0));
    }
    let part = Part {
        name: source.name.clone(),
        positions: cleaned.positions.clone(),
        tris: cleaned.tris.clone(),
        tri_loops,
        tri_source,
        sources: source.sources.clone(),
        uv_layers,
        color_layers,
        object: source.object,
        plate: source.plate,
    };
    CutSource { part, from: origin.iter().map(|(src, _)| *src).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ColorSource;

    /// An open box (no top) with its seams unwelded: every face has its own corners.
    fn open_box() -> Part {
        let quads = [
            ([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]),
            ([0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [10.0, 0.0, 10.0], [10.0, 0.0, 0.0]),
            ([10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [10.0, 10.0, 10.0], [10.0, 10.0, 0.0]),
            ([10.0, 10.0, 0.0], [10.0, 10.0, 10.0], [0.0, 10.0, 10.0], [0.0, 10.0, 0.0]),
            ([0.0, 10.0, 0.0], [0.0, 10.0, 10.0], [0.0, 0.0, 10.0], [0.0, 0.0, 0.0]),
        ];
        let mut part = Part { name: "box".into(), sources: vec![ColorSource::Flat([0.5; 3])], ..Default::default() };
        for (a, b, c, d) in quads {
            let i = part.positions.len() as u32;
            part.positions.extend([a, b, c, d]);
            for t in [[i, i + 2, i + 1], [i, i + 3, i + 2]] {
                part.tris.push(t);
                let l = part.tri_loops.len() as u32 * 3;
                part.tri_loops.push([l, l + 1, l + 2]);
                part.tri_source.push(0);
            }
        }
        part.uv_layers = vec![(0..part.tris.len() * 3).map(|i| [i as f32, 0.0]).collect()];
        part
    }

    #[test]
    fn baked_part_is_welded_capped_and_keeps_its_uvs() {
        let source = open_box();
        let scene = Scene { parts: vec![source.clone()], images: vec![], materials: vec![] };
        let ops = MeshOps { repair: true, deep_clean: true, fill_holes: true, orient: true, ..MeshOps::default() };
        let (shape, _) = Mesh::build(&scene, ops, 1.0);
        let baked = bake_part(&source, &shape.parts[0]);
        assert!(crate::cut::valid_solid(&baked.part, 1e-5), "the hole is capped and the seams welded");
        assert_eq!(baked.from.len(), baked.part.tris.len());
        assert_eq!(baked.part.uv_layers[0].len(), baked.part.tris.len() * 3);
        assert_eq!(baked.part.tri_source.len(), baked.part.tris.len());
    }

    #[test]
    fn an_open_unwelded_model_with_a_stray_face_can_be_cut() {
        let mut part = open_box();
        // A loose face floating inside, the kind a file leaves behind.
        let i = part.positions.len() as u32;
        part.positions.extend([[4.0, 4.0, 5.0], [6.0, 4.0, 5.0], [5.0, 6.0, 5.0]]);
        part.tris.push([i, i + 1, i + 2]);
        let l = part.tri_loops.len() as u32 * 3;
        part.tri_loops.push([l, l + 1, l + 2]);
        part.tri_source.push(0);
        part.uv_layers[0].extend([[0.0; 2]; 3]);
        let scene = Scene { parts: vec![part], images: vec![], materials: vec![] };
        let mut p = Project::new(scene, 1);
        assert_eq!(p.cut_plane([0.0, 0.0, 3.0], [0.0, 0.0, 1.0], true, true, false, &[]), Ok(()));
        assert_eq!(p.object_count(), 2);
        assert!(p.undo());
        assert_eq!(p.object_count(), 1);
    }
}
