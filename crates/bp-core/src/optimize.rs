//! Colour clean-up for faster prints: every small island of one colour is a
//! reason for the printer to swap filament on the layers it spans, so islands
//! under a minimum area take the colour of the neighbour they share the most
//! border with.

use std::collections::HashMap;

use crate::geom;
use crate::mesh::MeshPart;
use crate::paint::Node;

/// Triangle neighbours across shared edges, with the shared edge length.
pub fn adjacency(part: &MeshPart) -> Vec<Vec<(u32, f32)>> {
    let mut edges: HashMap<(u32, u32), Vec<u32>> = HashMap::with_capacity(part.tris.len() * 3 / 2);
    for (i, t) in part.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(i as u32);
        }
    }
    let mut adj = vec![Vec::new(); part.tris.len()];
    for ((a, b), tris) in edges {
        let l = geom::len(geom::sub(part.positions[a as usize], part.positions[b as usize]));
        for &x in &tris {
            for &y in &tris {
                if x != y {
                    adj[x as usize].push((y, l));
                }
            }
        }
    }
    adj
}

/// Merges islands smaller than `min_area` (source units²) into their neighbours.
/// Triangles marked in `locked` (painted by hand) are never changed.
/// Returns how many triangles changed colour.
pub fn despeckle(
    part: &MeshPart,
    adj: &[Vec<(u32, f32)>],
    nodes: &mut [Node],
    locked: &[bool],
    min_area: f32,
) -> usize {
    let n = nodes.len();
    let mut changed = 0;
    // Two passes: merging one island can leave its neighbour small.
    for _ in 0..2 {
        let label: Vec<u8> = nodes.iter().map(Node::dominant).collect();
        let mut region = vec![u32::MAX; n];
        let mut stack = Vec::new();
        for seed in 0..n {
            if region[seed] != u32::MAX {
                continue;
            }
            let id = seed as u32;
            let mut members = vec![seed as u32];
            let mut area = 0.0f32;
            let mut border: HashMap<u8, f32> = HashMap::new();
            let mut has_lock = false;
            region[seed] = id;
            stack.push(seed as u32);
            while let Some(t) = stack.pop() {
                area += geom::area(&part.corners(t as usize));
                has_lock |= locked.get(t as usize).copied().unwrap_or(false);
                for &(o, l) in &adj[t as usize] {
                    if label[o as usize] == label[seed] {
                        if region[o as usize] == u32::MAX {
                            region[o as usize] = id;
                            members.push(o);
                            stack.push(o);
                        }
                    } else {
                        *border.entry(label[o as usize]).or_default() += l;
                    }
                }
            }
            if area >= min_area || has_lock {
                continue;
            }
            if let Some((&to, _)) = border.iter().max_by(|a, b| a.1.total_cmp(b.1)) {
                for &m in &members {
                    nodes[m as usize] = Node::Leaf(to);
                }
                changed += members.len();
            }
        }
        if changed == 0 {
            break;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(n: usize) -> MeshPart {
        // A row of quads, two triangles each.
        let mut positions = vec![];
        for x in 0..=n {
            positions.push([x as f32, 0.0, 0.0]);
            positions.push([x as f32, 1.0, 0.0]);
        }
        let mut tris = vec![];
        for x in 0..n as u32 {
            let (a, b, c, d) = (2 * x, 2 * x + 2, 2 * x + 3, 2 * x + 1);
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
        MeshPart { name: "s".into(), positions, tris, origin: None }
    }

    #[test]
    fn small_island_takes_neighbour_colour() {
        let part = strip(10);
        let adj = adjacency(&part);
        let mut nodes: Vec<Node> = (0..20).map(|_| Node::Leaf(0)).collect();
        // Triangles 8 and 9 share the diagonal of the fifth quad.
        nodes[8] = Node::Leaf(1);
        nodes[9] = Node::Leaf(1);
        let changed = despeckle(&part, &adj, &mut nodes, &[false; 20], 2.0);
        assert_eq!(changed, 2);
        assert!(nodes.iter().all(|n| *n == Node::Leaf(0)));
    }

    #[test]
    fn locked_island_survives() {
        let part = strip(10);
        let adj = adjacency(&part);
        let mut nodes: Vec<Node> = (0..20).map(|_| Node::Leaf(0)).collect();
        nodes[9] = Node::Leaf(1);
        let mut locked = [false; 20];
        locked[9] = true;
        despeckle(&part, &adj, &mut nodes, &locked, 2.0);
        assert_eq!(nodes[9], Node::Leaf(1));
    }
}
