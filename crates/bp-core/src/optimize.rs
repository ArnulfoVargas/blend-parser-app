//! Colour clean-up for faster prints: every small island of one colour is a
//! reason for the printer to swap filament on the layers it spans, so islands
//! under a minimum area take the colour of the neighbour they share the most
//! border with.

use rayon::prelude::*;
use rustc_hash::FxHashMap as HashMap;

use crate::geom;
use crate::mesh::MeshPart;
use crate::paint::Node;

/// Triangle neighbours across shared edges, with the shared edge length, packed
/// in one buffer: `adj[t]` is the slice of triangle `t`'s neighbours. One
/// allocation instead of one per triangle matters on meshes of millions.
#[derive(Clone, Debug, Default)]
pub struct Adjacency {
    start: Vec<u32>,
    items: Vec<(u32, f32)>,
}

impl Adjacency {
    pub fn len(&self) -> usize {
        self.start.len().saturating_sub(1)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = &[(u32, f32)]> + '_ {
        (0..self.len()).map(move |t| &self[t])
    }
}

impl std::ops::Index<usize> for Adjacency {
    type Output = [(u32, f32)];
    #[inline]
    fn index(&self, t: usize) -> &[(u32, f32)] {
        &self.items[self.start[t] as usize..self.start[t + 1] as usize]
    }
}

/// Triangles per block when filling the adjacency in parallel.
const BLOCK: usize = 4096;

/// Triangle neighbours across shared edges, with the shared edge length. Two
/// triangles are neighbours across an edge once per edge they share.
pub fn adjacency(part: &MeshPart) -> Adjacency {
    let n = part.tris.len();
    let nv = part.positions.len();
    // Triangles round each vertex (counting sort).
    let mut first = vec![0u32; nv + 1];
    for t in &part.tris {
        for &v in t {
            first[v as usize + 1] += 1;
        }
    }
    for i in 0..nv {
        first[i + 1] += first[i];
    }
    let mut fill = first.clone();
    let mut round = vec![0u32; n * 3];
    for (i, t) in part.tris.iter().enumerate() {
        for &v in t {
            round[fill[v as usize] as usize] = i as u32;
            fill[v as usize] += 1;
        }
    }
    drop(fill);
    // Every other triangle round `a` that has the edge a–b too, once per such edge.
    let has_edge = |u: [u32; 3], a: u32, b: u32| {
        (0..3).filter(|&k| (u[k] == a && u[(k + 1) % 3] == b) || (u[k] == b && u[(k + 1) % 3] == a)).count()
    };
    let visit = |t: usize, f: &mut dyn FnMut(u32, u32, u32)| {
        let tri = part.tris[t];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            for &u in &round[first[a as usize] as usize..first[a as usize + 1] as usize] {
                if u as usize == t {
                    continue;
                }
                for _ in 0..has_edge(part.tris[u as usize], a, b) {
                    f(u, a, b);
                }
            }
        }
    };
    let counts: Vec<u32> = (0..n)
        .into_par_iter()
        .map(|t| {
            let mut c = 0;
            visit(t, &mut |_, _, _| c += 1);
            c
        })
        .collect();
    let mut start = Vec::with_capacity(n + 1);
    start.push(0u32);
    for c in &counts {
        start.push(start.last().unwrap() + c);
    }
    drop(counts);
    let mut items = vec![(0u32, 0.0f32); *start.last().unwrap() as usize];
    // Blocks of triangles own consecutive runs of `items`.
    let mut blocks = Vec::with_capacity(n.div_ceil(BLOCK));
    let mut rest = items.as_mut_slice();
    for b in (0..n).step_by(BLOCK) {
        let end = (b + BLOCK).min(n);
        let (mine, tail) = rest.split_at_mut((start[end] - start[b]) as usize);
        blocks.push((b, end, mine));
        rest = tail;
    }
    blocks.into_par_iter().for_each(|(b, end, out)| {
        let mut at = 0;
        for t in b..end {
            visit(t, &mut |u, a, bb| {
                let l = geom::len(geom::sub(part.positions[a as usize], part.positions[bb as usize]));
                out[at] = (u, l);
                at += 1;
            });
        }
    });
    Adjacency { start, items }
}

/// Merges islands smaller than `min_area` (source units²) into their neighbours.
/// Triangles marked in `locked` (painted by hand) are never changed.
/// Returns how many triangles changed colour.
pub fn despeckle(part: &MeshPart, adj: &Adjacency, nodes: &mut [Node], locked: &[bool], min_area: f32) -> usize {
    let n = nodes.len();
    let mut changed = 0;
    // Two passes: merging one island can leave its neighbour small.
    for _ in 0..2 {
        let label: Vec<u8> = nodes.par_iter().map(Node::dominant).collect();
        let mut region = vec![u32::MAX; n];
        let mut stack = Vec::new();
        for seed in 0..n {
            if region[seed] != u32::MAX {
                continue;
            }
            let id = seed as u32;
            let mut members = vec![seed as u32];
            let mut area = 0.0f32;
            let mut border: HashMap<u8, f32> = HashMap::default();
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

/// How many islands of one colour are smaller than `min_area` (source units²).
pub fn small_islands(part: &MeshPart, adj: &Adjacency, nodes: &[Node], min_area: f32) -> usize {
    let label: Vec<u8> = nodes.par_iter().map(Node::dominant).collect();
    let mut seen = vec![false; nodes.len()];
    let mut stack = Vec::new();
    let mut count = 0;
    for seed in 0..nodes.len() {
        if seen[seed] {
            continue;
        }
        seen[seed] = true;
        stack.push(seed as u32);
        let mut area = 0.0f32;
        while let Some(t) = stack.pop() {
            area += geom::area(&part.corners(t as usize));
            for &(o, _) in &adj[t as usize] {
                if !seen[o as usize] && label[o as usize] == label[seed] {
                    seen[o as usize] = true;
                    stack.push(o);
                }
            }
        }
        count += (area < min_area) as usize;
    }
    count
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
    fn counts_islands_under_the_area() {
        let part = strip(10);
        let adj = adjacency(&part);
        let mut nodes: Vec<Node> = (0..20).map(|_| Node::Leaf(0)).collect();
        nodes[8] = Node::Leaf(1);
        nodes[9] = Node::Leaf(1);
        // The 1 mm² island counts; the 9 mm² rest of the strip does not.
        assert_eq!(small_islands(&part, &adj, &nodes, 2.0), 1);
        assert_eq!(small_islands(&part, &adj, &nodes, 0.5), 0);
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
