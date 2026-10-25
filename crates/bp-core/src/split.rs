//! Splits a painted mesh into one closed solid per filament colour, for
//! printing colours as separate pieces or for slicers without paint support.
//!
//! Each colour becomes a shell: its patch of surface, the same patch pushed
//! inward along the vertex normals by `thickness`, and walls joining the two
//! along the patch border. Optionally one colour (the main one) is exported as
//! the whole closed part instead, so the shells can sit on it as inlays.

use crate::geom::{self, Vec3};
use crate::mesh::Mesh;

pub struct ColorPiece {
    pub slot: u8,
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// True when this is the whole part rather than a shell.
    pub solid: bool,
}

/// Area-weighted vertex normals of a part.
fn vertex_normals(positions: &[Vec3], tris: &[[u32; 3]]) -> Vec<Vec3> {
    let mut n = vec![[0.0f32; 3]; positions.len()];
    for t in tris {
        let c = [positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]];
        let fnrm = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
        for &v in t {
            n[v as usize] = geom::add(n[v as usize], fnrm);
        }
    }
    n.into_iter().map(geom::normalize).collect()
}

/// `labels[part][tri]` is each triangle's colour. `thickness` is in mesh units.
pub fn color_pieces(
    mesh: &Mesh,
    labels: &[Vec<u8>],
    slots: usize,
    thickness: f32,
    solid_slot: Option<u8>,
) -> Vec<ColorPiece> {
    let mut pieces: Vec<ColorPiece> = (0..slots)
        .map(|s| ColorPiece { slot: s as u8, positions: vec![], tris: vec![], solid: Some(s as u8) == solid_slot })
        .collect();
    for (part, labels) in mesh.parts.iter().zip(labels) {
        let normals = vertex_normals(&part.positions, &part.tris);
        for piece in pieces.iter_mut() {
            if piece.solid {
                let base = piece.positions.len() as u32;
                piece.positions.extend_from_slice(&part.positions);
                piece.tris.extend(part.tris.iter().map(|t| t.map(|v| v + base)));
                continue;
            }
            let patch: Vec<[u32; 3]> =
                part.tris.iter().zip(labels).filter(|(_, &l)| l == piece.slot).map(|(t, _)| *t).collect();
            if patch.is_empty() {
                continue;
            }
            // Local vertices: outer at 2k, inner at 2k + 1.
            let mut local = std::collections::HashMap::new();
            let mut id = |v: u32, positions: &mut Vec<Vec3>| -> u32 {
                *local.entry(v).or_insert_with(|| {
                    let p = part.positions[v as usize];
                    positions.push(p);
                    positions.push(geom::sub(p, geom::scale(normals[v as usize], thickness)));
                    positions.len() as u32 - 2
                })
            };
            let mut edges = std::collections::HashSet::new();
            for t in &patch {
                let o =
                    [id(t[0], &mut piece.positions), id(t[1], &mut piece.positions), id(t[2], &mut piece.positions)];
                piece.tris.push(o);
                piece.tris.push([o[2] + 1, o[1] + 1, o[0] + 1]);
                for k in 0..3 {
                    edges.insert((o[k], o[(k + 1) % 3]));
                }
            }
            // Border edges have no opposite edge in the patch: close them with a wall.
            for &(a, b) in &edges {
                if !edges.contains(&(b, a)) {
                    piece.tris.push([b, a, a + 1]);
                    piece.tris.push([b, a + 1, b + 1]);
                }
            }
        }
    }
    pieces.retain(|p| !p.tris.is_empty());
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{MeshPart, RepairReport};
    use crate::optimize::adjacency;

    fn cube() -> Mesh {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let tris = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        Mesh {
            parts: vec![MeshPart { name: "c".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
        }
    }

    fn closed(p: &ColorPiece) -> bool {
        let part = MeshPart { name: String::new(), positions: p.positions.clone(), tris: p.tris.clone(), origin: None };
        adjacency(&part).iter().all(|n| n.len() == 3)
    }

    #[test]
    fn each_colour_is_a_closed_shell() {
        let m = cube();
        // Top face slot 1, the rest slot 0.
        let labels = vec![(0..12).map(|i| if i == 2 || i == 3 { 1 } else { 0 }).collect::<Vec<u8>>()];
        let pieces = color_pieces(&m, &labels, 2, 0.1, None);
        assert_eq!(pieces.len(), 2);
        let top = pieces.iter().find(|p| p.slot == 1).unwrap();
        // 2 outer + 2 inner + 4 border edges * 2 wall triangles.
        assert_eq!(top.tris.len(), 12);
        assert!(closed(top));
        assert!(pieces.iter().all(closed));
    }

    #[test]
    fn main_colour_can_be_the_whole_solid() {
        let m = cube();
        let labels = vec![(0..12).map(|i| if i == 2 || i == 3 { 1 } else { 0 }).collect::<Vec<u8>>()];
        let pieces = color_pieces(&m, &labels, 2, 0.1, Some(0));
        let main = pieces.iter().find(|p| p.slot == 0).unwrap();
        assert!(main.solid && main.tris.len() == 12);
    }
}
