//! Turning the painting's subdivisions into real triangles, so a colour border
//! that runs across a triangle of the model becomes an edge of the mesh. Each
//! split triangle is replaced by its leaves; wherever a leaf edge has a corner
//! of a smaller leaf on it (inside the triangle or across an edge from the
//! triangle beside it) the leaf is closed with a fan round its centre instead,
//! so the surface keeps every edge shared by exactly the faces it had.

use std::collections::{HashMap, HashSet};

use crate::geom::{self, Bary, Vec3, ROOT};
use crate::paint::Node;
use crate::scene::Part;
use crate::separate;

/// Barycentric points on a lattice of 1/N. Leaves halve their parent's edges,
/// so any paint depth up to 7 lands on it exactly.
const N: i32 = 128;
type P = [i32; 3];

pub struct Refined {
    pub part: Part,
    /// The input triangle each new triangle lies on.
    pub origins: Vec<u32>,
    /// Input triangles that were split along their painting.
    pub split: usize,
}

/// Where a vertex of the result comes from: an input corner, a point on an
/// input edge (its welded ends, low first, and how many steps from the low
/// one), or a point inside one triangle.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Corner(u32),
    Edge((u32, u32), i32),
    Inside(usize, P),
}

/// `part` with each triangle whose tree in `trees` is split replaced by its
/// leaves, the triangles beside them split to match. Corners at the same
/// position count as one.
pub fn refine(part: &Part, trees: &[Option<&Node>]) -> Refined {
    let ids = separate::weld_ids(&part.positions, 0.0);
    let tris: Vec<[u32; 3]> = part.tris.iter().map(|t| t.map(|v| ids[v as usize])).collect();
    let lattice = |b: Bary| -> P {
        let (x, y) = ((b[0] * N as f32).round() as i32, (b[1] * N as f32).round() as i32);
        [x, y, N - x - y]
    };
    let whole: [P; 3] = [[N, 0, 0], [0, N, 0], [0, 0, N]];
    let mut split = 0;
    let leaves: Vec<Vec<[P; 3]>> = (0..tris.len())
        .map(|ti| match trees.get(ti).copied().flatten() {
            Some(node) if node.is_split() => {
                split += 1;
                let mut out = vec![];
                node.for_each_leaf(&ROOT, &mut |t, _| out.push(t.map(lattice)));
                out
            }
            _ => vec![whole],
        })
        .collect();
    // A point with one zero coordinate lies on the input edge across from that corner.
    let on_edge = |t: [u32; 3], p: P| -> Option<((u32, u32), i32)> {
        let zeros: Vec<usize> = (0..3).filter(|&k| p[k] == 0).collect();
        let [o] = zeros[..] else { return None };
        let (m, n) = ((o + 1) % 3, (o + 2) % 3);
        let (a, b) = (t[m], t[n]);
        Some(if a < b { ((a, b), p[n]) } else { ((b, a), p[m]) })
    };
    let mut edge_points: HashMap<(u32, u32), HashSet<i32>> = HashMap::new();
    let mut own: Vec<HashSet<P>> = Vec::with_capacity(tris.len());
    for (t, ls) in tris.iter().zip(&leaves) {
        let points: HashSet<P> = ls.iter().flatten().copied().collect();
        for &p in &points {
            if let Some((edge, step)) = on_edge(*t, p) {
                edge_points.entry(edge).or_default().insert(step);
            }
        }
        own.push(points);
    }

    let mut out = Part {
        name: part.name.clone(),
        sources: part.sources.clone(),
        uv_layers: part.uv_layers.clone(),
        color_layers: part.color_layers.clone(),
        object: part.object,
        ..Default::default()
    };
    let loops = part.tri_loops.iter().flatten().map(|&l| l + 1).max().unwrap_or(0);
    let stored = part.uv_layers.iter().map(Vec::len).chain(part.color_layers.iter().map(Vec::len)).max().unwrap_or(0);
    let mut next_loop = loops.max(stored as u32);
    // Every layer as long as the corners in use, so new corners go after them.
    for layer in &mut out.uv_layers {
        layer.resize(next_loop as usize, [0.0; 2]);
    }
    for layer in &mut out.color_layers {
        layer.resize(next_loop as usize, [0.0; 3]);
    }
    let mut vertex: HashMap<Key, u32> = HashMap::new();
    let mut origins = vec![];
    for (ti, (t, ls)) in tris.iter().zip(&leaves).enumerate() {
        let corners: [Vec3; 3] = t.map(|v| part.positions[v as usize]);
        let l = part.tri_loops[ti];
        let src = part.tri_source.get(ti).copied().unwrap_or(0);
        let mut corner_loop: HashMap<P, u32> = (0..3).map(|k| (whole[k], l[k])).collect();
        // A face corner at barycentric `b`: its UVs and colours blended from the triangle's.
        let mut blend = |b: Bary, out: &mut Part| -> u32 {
            for layer in &mut out.uv_layers {
                let c = l.map(|i| layer[i as usize]);
                layer.push(std::array::from_fn(|k| c[0][k] * b[0] + c[1][k] * b[1] + c[2][k] * b[2]));
            }
            for layer in &mut out.color_layers {
                let c = l.map(|i| layer[i as usize]);
                layer.push(std::array::from_fn(|k| c[0][k] * b[0] + c[1][k] * b[1] + c[2][k] * b[2]));
            }
            next_loop += 1;
            next_loop - 1
        };
        let bary = |p: P| p.map(|x| x as f32 / N as f32);
        let mut point = |p: P, out: &mut Part, blend: &mut dyn FnMut(Bary, &mut Part) -> u32| -> (u32, u32) {
            let key = match (0..3).find(|&k| p[k] == N) {
                Some(k) => Key::Corner(t[k]),
                None => on_edge(*t, p).map_or(Key::Inside(ti, p), |(e, s)| Key::Edge(e, s)),
            };
            let v = *vertex.entry(key).or_insert_with(|| {
                out.positions.push(match key {
                    Key::Corner(c) => part.positions[c as usize],
                    // Worked out from the edge's own ends, so both triangles on it agree exactly.
                    Key::Edge((a, b), s) => {
                        let (pa, pb) = (part.positions[a as usize], part.positions[b as usize]);
                        geom::add(pa, geom::scale(geom::sub(pb, pa), s as f32 / N as f32))
                    }
                    Key::Inside(..) => geom::at(&corners, bary(p)),
                });
                out.positions.len() as u32 - 1
            });
            let c = *corner_loop.entry(p).or_insert_with(|| blend(bary(p), out));
            (v, c)
        };
        for leaf in ls {
            // The leaf's outline with every lattice point on its edges that a neighbour uses.
            let mut ring: Vec<P> = vec![];
            for k in 0..3 {
                let (a, b) = (leaf[k], leaf[(k + 1) % 3]);
                ring.push(a);
                let d: P = std::array::from_fn(|i| b[i] - a[i]);
                let steps = gcd(gcd(d[0].abs(), d[1].abs()), d[2].abs()).max(1);
                for s in 1..steps {
                    let q: P = std::array::from_fn(|i| a[i] + d[i] * s / steps);
                    let shared =
                        on_edge(*t, q).is_some_and(|(e, st)| edge_points.get(&e).is_some_and(|set| set.contains(&st)));
                    if own[ti].contains(&q) || shared {
                        ring.push(q);
                    }
                }
            }
            let corners: Vec<(u32, u32)> = ring.iter().map(|&p| point(p, &mut out, &mut blend)).collect();
            let fan: Vec<[(u32, u32); 3]> = if corners.len() == 3 {
                vec![[corners[0], corners[1], corners[2]]]
            } else {
                // A convex outline with points on its edges: a fan round its centre covers it.
                let centre: Bary = {
                    let sum = ring.iter().fold([0.0f32; 3], |s, &p| geom::add(s, bary(p)));
                    geom::scale(sum, 1.0 / ring.len() as f32)
                };
                out.positions.push(geom::at(&corners_of(&part.positions, *t), centre));
                let hub = (out.positions.len() as u32 - 1, blend(centre, &mut out));
                (0..corners.len()).map(|k| [corners[k], corners[(k + 1) % corners.len()], hub]).collect()
            };
            for f in fan {
                out.tris.push(f.map(|c| c.0));
                out.tri_loops.push(f.map(|c| c.1));
                out.tri_source.push(src);
                origins.push(ti as u32);
            }
        }
    }
    Refined { part: out, origins, split }
}

fn corners_of(positions: &[Vec3], t: [u32; 3]) -> [Vec3; 3] {
    t.map(|v| positions[v as usize])
}

fn gcd(a: i32, b: i32) -> i32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every edge used once each way: a closed, consistently wound surface.
    fn closed(p: &Part) -> bool {
        let all: Vec<u32> = (0..p.tris.len() as u32).collect();
        separate::closed(&p.positions, &p.tris, &all, 0.0)
    }

    fn area(p: &Part) -> f32 {
        (0..p.tris.len()).map(|t| geom::area(&p.corners(t))).sum()
    }

    #[test]
    fn split_triangles_become_their_leaves_and_the_box_stays_closed() {
        let part = crate::region::tests::grid_box([0.0; 3], [10.0; 3], 1);
        // One face split twice over in one corner, another split once.
        let deep = Node::Split(Box::new([
            Node::Split(Box::new([Node::Leaf(1), Node::Leaf(0), Node::Leaf(0), Node::Leaf(0)])),
            Node::Leaf(0),
            Node::Leaf(0),
            Node::Leaf(1),
        ]));
        let once = Node::Split(Box::new([Node::Leaf(0), Node::Leaf(1), Node::Leaf(1), Node::Leaf(1)]));
        let mut trees: Vec<Option<&Node>> = vec![None; part.tris.len()];
        trees[0] = Some(&deep);
        trees[5] = Some(&once);
        let r = refine(&part, &trees);
        assert_eq!(r.split, 2);
        assert!(closed(&r.part), "no T-junctions are left");
        assert!((area(&r.part) - area(&part)).abs() < 1e-3);
        assert!(r.part.tris.len() > part.tris.len() + 7);
        assert_eq!(r.origins.len(), r.part.tris.len());
        assert!(r.part.tri_loops.iter().flatten().all(|&l| (l as usize) < r.part.uv_layers[0].len()));
        // Untouched, nothing changes but the welding.
        let none = refine(&part, &vec![None; part.tris.len()]);
        assert_eq!(none.part.tris.len(), part.tris.len());
        assert_eq!(none.split, 0);
    }

    #[test]
    fn a_new_corner_takes_its_uv_from_where_it_lies() {
        let mut part = crate::region::tests::grid_box([0.0; 3], [10.0; 3], 1);
        // Give triangle 0 the UVs (0,0), (1,0), (0,1) at its corners.
        let l = part.tri_loops[0];
        for (k, uv) in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]].into_iter().enumerate() {
            part.uv_layers[0][l[k] as usize] = uv;
        }
        let once = Node::Split(Box::new([Node::Leaf(0), Node::Leaf(1), Node::Leaf(0), Node::Leaf(0)]));
        let mut trees: Vec<Option<&Node>> = vec![None; part.tris.len()];
        trees[0] = Some(&once);
        let r = refine(&part, &trees);
        // The middle leaf's corners are the edge midpoints: UVs of a half.
        let middle = (0..r.part.tris.len()).filter(|&t| r.origins[t] == 0).nth(3).unwrap();
        let uvs: Vec<[f32; 2]> = r.part.tri_loops[middle].iter().map(|&c| r.part.uv_layers[0][c as usize]).collect();
        assert!(uvs.iter().all(|uv| uv.iter().all(|&x| x == 0.0 || x == 0.5)), "{uvs:?}");
    }
}
