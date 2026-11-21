//! The model's own surface as the pieces' skin. The voxel pieces decide where
//! one colour ends and the next begins inside; their outside only follows the
//! model to within the grid, rounding sharp edges. So each piece is grown a
//! little past the model and cut back to it with an exact boolean: what lies
//! on the outside is then the model's triangles, corners and all, and what
//! lies between pieces is still the voxel surface they share.
//!
//! Needs the model closed; when it is not, or a boolean fails, the piece keeps
//! the voxel skin.

use std::collections::HashMap;

use manifold_rust::manifold::Manifold;
use manifold_rust::types::{BooleanEngine, Error, MeshGL};

use super::MeshData;
use crate::bvh::TriSoup;
use crate::geom::Vec3;

fn mesh_gl(positions: &[Vec3], tris: &[[u32; 3]]) -> MeshGL {
    MeshGL {
        num_prop: 3,
        vert_properties: positions.iter().flatten().copied().collect(),
        tri_verts: tris.iter().flatten().copied().collect(),
        ..Default::default()
    }
}

fn solid(mesh: &MeshGL) -> Option<Manifold> {
    let m = Manifold::from_mesh_gl_robust(mesh);
    (m.status() == Error::NoError && !m.is_empty()).then_some(m)
}

/// The model as a solid, its corners welded where they coincide.
pub(super) fn body(soup: &TriSoup) -> Option<Manifold> {
    let mut id: HashMap<[u32; 3], u32> = HashMap::with_capacity(soup.len());
    let mut positions = Vec::new();
    let tris: Vec<[u32; 3]> = soup
        .corners
        .iter()
        .map(|c| {
            c.map(|p| {
                *id.entry(p.map(f32::to_bits)).or_insert_with(|| {
                    positions.push(p);
                    positions.len() as u32 - 1
                })
            })
        })
        .collect();
    solid(&mesh_gl(&positions, &tris))
}

/// `grown` cut back to the model, or `None` if that cannot be done.
pub(super) fn clip(body: &Manifold, grown: &MeshData) -> Option<MeshData> {
    let piece = solid(&mesh_gl(&grown.0, &grown.1))?;
    let cut = body.intersection_with_engine(&piece, BooleanEngine::Auto);
    if cut.status() != Error::NoError || cut.is_empty() {
        return None;
    }
    let gl = cut.get_mesh_gl(0);
    let stride = gl.num_prop as usize;
    let positions = gl.vert_properties.chunks_exact(stride).map(|v| [v[0], v[1], v[2]]).collect();
    let tris = gl.tri_verts.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    Some((positions, tris))
}
