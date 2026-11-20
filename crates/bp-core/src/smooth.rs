//! Rounding facets off: Loop subdivision and Taubin smoothing. Both keep sharp
//! edges (faces meeting at more than the crease angle), colour borders (faces of
//! different slots) and open or shared-by-many edges where they are, so a
//! low-poly model gets round without its painted borders or its corners melting.

use std::collections::HashMap;

use crate::geom::{self, Vec3};
use crate::mesh::MeshPart;

/// Taubin's pair: shrink by λ, then grow by μ, so the model keeps its volume.
const LAMBDA: f32 = 0.5;
const MU: f32 = -0.53;

/// The edges of a part and which of them stay sharp.
struct Edges {
    /// Edge of each triangle side: `side[t * 3 + k]` is the edge from corner k to k + 1.
    side: Vec<u32>,
    /// Ends of each edge, lower index first.
    ends: Vec<(u32, u32)>,
    /// Triangles on each edge (up to two kept) and how many there are.
    faces: Vec<([u32; 2], u32)>,
    crease: Vec<bool>,
}

impl Edges {
    fn new(part: &MeshPart, labels: Option<&[u8]>, crease_cos: f32) -> Self {
        let mut index: HashMap<(u32, u32), u32> = HashMap::with_capacity(part.tris.len() * 3 / 2);
        let mut side = Vec::with_capacity(part.tris.len() * 3);
        let mut ends = vec![];
        let mut faces: Vec<([u32; 2], u32)> = vec![];
        for (ti, t) in part.tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let key = (a.min(b), a.max(b));
                let e = *index.entry(key).or_insert_with(|| {
                    ends.push(key);
                    faces.push(([u32::MAX; 2], 0));
                    ends.len() as u32 - 1
                });
                let f = &mut faces[e as usize];
                if f.1 < 2 {
                    f.0[f.1 as usize] = ti as u32;
                }
                f.1 += 1;
                side.push(e);
            }
        }
        let normal = |t: u32| {
            let c = part.corners(t as usize);
            geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]))
        };
        let crease = faces
            .iter()
            .map(|&([x, y], n)| {
                if n != 2 {
                    return true;
                }
                if labels.is_some_and(|l| l.get(x as usize) != l.get(y as usize)) {
                    return true;
                }
                let (nx, ny) = (normal(x), normal(y));
                let l = geom::len(nx) * geom::len(ny);
                l == 0.0 || geom::dot(nx, ny) / l < crease_cos
            })
            .collect();
        Self { side, ends, faces, crease }
    }

    /// For each vertex: its neighbours across smooth edges and across sharp ones.
    fn rings(&self, vertices: usize) -> (Vec<Vec<u32>>, Vec<Vec<u32>>) {
        let mut all = vec![vec![]; vertices];
        let mut sharp = vec![vec![]; vertices];
        for (e, &(a, b)) in self.ends.iter().enumerate() {
            all[a as usize].push(b);
            all[b as usize].push(a);
            if self.crease[e] {
                sharp[a as usize].push(b);
                sharp[b as usize].push(a);
            }
        }
        (all, sharp)
    }
}

/// The corner of triangle `t` that is not on edge `(a, b)`.
fn opposite(part: &MeshPart, t: u32, (a, b): (u32, u32)) -> u32 {
    part.tris[t as usize].into_iter().find(|&v| v != a && v != b).unwrap_or(a)
}

fn average(points: impl Iterator<Item = Vec3>) -> Option<Vec3> {
    let (sum, n) = points.fold(([0.0; 3], 0), |(s, n), p| (geom::add(s, p), n + 1));
    (n > 0).then(|| geom::scale(sum, 1.0 / n as f32))
}

/// One level of Loop subdivision: every triangle becomes four. A vertex on one
/// sharp line follows that line as a curve; where sharp lines meet or end, it
/// stays put. `labels` (one per triangle, empty for none) is split along.
pub fn subdivide(part: &mut MeshPart, labels: &mut Vec<u8>, crease_deg: f32) {
    let has_labels = labels.len() == part.tris.len();
    let edges = Edges::new(part, has_labels.then_some(&labels[..]), crease_deg.to_radians().cos());
    let n = part.positions.len();
    let (all, sharp) = edges.rings(n);
    let p = &part.positions;
    let mut positions: Vec<Vec3> = (0..n)
        .map(|v| match sharp[v].len() {
            0 if !all[v].is_empty() => {
                let k = all[v].len() as f32;
                let beta = if all[v].len() == 3 { 3.0 / 16.0 } else { 3.0 / (8.0 * k) };
                let ring = all[v].iter().fold([0.0; 3], |s, &w| geom::add(s, p[w as usize]));
                geom::add(geom::scale(p[v], 1.0 - k * beta), geom::scale(ring, beta))
            }
            2 => geom::add(
                geom::scale(p[v], 0.75),
                geom::scale(geom::add(p[sharp[v][0] as usize], p[sharp[v][1] as usize]), 0.125),
            ),
            _ => p[v],
        })
        .collect();
    for (e, &(a, b)) in edges.ends.iter().enumerate() {
        let (pa, pb) = (p[a as usize], p[b as usize]);
        let point = if edges.crease[e] {
            geom::scale(geom::add(pa, pb), 0.5)
        } else {
            let [x, y] = edges.faces[e].0;
            let (c, d) = (p[opposite(part, x, (a, b)) as usize], p[opposite(part, y, (a, b)) as usize]);
            geom::add(geom::scale(geom::add(pa, pb), 0.375), geom::scale(geom::add(c, d), 0.125))
        };
        positions.push(point);
    }
    let mid = |t: usize, k: usize| n as u32 + edges.side[t * 3 + k];
    let mut tris = Vec::with_capacity(part.tris.len() * 4);
    let mut origin = part.origin.as_ref().map(|_| Vec::with_capacity(part.tris.len() * 4));
    let mut split_labels = Vec::with_capacity(if has_labels { labels.len() * 4 } else { 0 });
    for (t, &[v0, v1, v2]) in part.tris.iter().enumerate() {
        let (m01, m12, m20) = (mid(t, 0), mid(t, 1), mid(t, 2));
        tris.extend([[v0, m01, m20], [m01, v1, m12], [m20, m12, v2], [m01, m12, m20]]);
        if let (Some(out), Some(src)) = (&mut origin, &part.origin) {
            // Each child covers its part of the parent's spot in the source: colours stay exact.
            let (s, [b0, b1, b2]) = src[t];
            let (h01, h12, h20) = (geom::mid(b0, b1), geom::mid(b1, b2), geom::mid(b2, b0));
            out.extend([(s, [b0, h01, h20]), (s, [h01, b1, h12]), (s, [h20, h12, b2]), (s, [h01, h12, h20])]);
        }
        if has_labels {
            split_labels.extend([labels[t]; 4]);
        }
    }
    part.positions = positions;
    part.tris = tris;
    part.origin = origin;
    *labels = split_labels;
}

/// `passes` rounds of Taubin smoothing. Vertices off any sharp line move
/// towards their neighbours; a vertex on one sharp line moves only along it;
/// where sharp lines meet or end, it stays. Triangles and colours do not change.
pub fn smooth(part: &mut MeshPart, labels: &[u8], crease_deg: f32, passes: u32) {
    if passes == 0 || part.tris.is_empty() {
        return;
    }
    let labels = (labels.len() == part.tris.len()).then_some(labels);
    let edges = Edges::new(part, labels, crease_deg.to_radians().cos());
    let (all, sharp) = edges.rings(part.positions.len());
    let pull: Vec<Option<&Vec<u32>>> = (0..part.positions.len())
        .map(|v| match sharp[v].len() {
            0 if !all[v].is_empty() => Some(&all[v]),
            2 => Some(&sharp[v]),
            _ => None,
        })
        .collect();
    for _ in 0..passes {
        for k in [LAMBDA, MU] {
            let p = &part.positions;
            let next: Vec<Vec3> = (0..p.len())
                .map(|v| {
                    let Some(ring) = pull[v] else { return p[v] };
                    match average(ring.iter().map(|&w| p[w as usize])) {
                        Some(c) => geom::add(p[v], geom::scale(geom::sub(c, p[v]), k)),
                        None => p[v],
                    }
                })
                .collect();
            part.positions = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::ROOT;

    /// A closed cylinder of `sides` sides, radius 1, from z = 0 to z = 2: two
    /// triangles per side and a fan on each cap.
    pub(crate) fn cylinder(sides: u32) -> MeshPart {
        let mut positions = vec![];
        for z in [0.0, 2.0] {
            for i in 0..sides {
                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                positions.push([a.cos(), a.sin(), z]);
            }
        }
        positions.push([0.0, 0.0, 0.0]);
        positions.push([0.0, 0.0, 2.0]);
        let (bottom, top) = (2 * sides, 2 * sides + 1);
        let mut tris = vec![];
        for i in 0..sides {
            let j = (i + 1) % sides;
            tris.push([i, j, sides + j]);
            tris.push([i, sides + j, sides + i]);
            tris.push([bottom, j, i]);
            tris.push([top, sides + i, sides + j]);
        }
        let origin = Some((0..tris.len() as u32).map(|t| (t, ROOT)).collect());
        MeshPart { name: "cyl".into(), positions, tris, origin }
    }

    fn radii_at_middle(part: &MeshPart) -> Vec<f32> {
        part.positions.iter().filter(|p| (p[2] - 1.0).abs() < 0.3).map(|p| p[0].hypot(p[1])).collect()
    }

    #[test]
    fn a_48_sided_cylinder_rounds_off_and_keeps_its_rims() {
        let mut part = cylinder(48);
        let mut labels = vec![];
        let tris = part.tris.len();
        for _ in 0..2 {
            subdivide(&mut part, &mut labels, 30.0);
        }
        assert_eq!(part.tris.len(), tris * 16);
        // The rims are sharp: they stay on the caps' planes and on the circle.
        let rim: Vec<&Vec3> = part.positions.iter().filter(|p| p[2] == 0.0 || p[2] == 2.0).collect();
        let rim_radii: Vec<f32> = rim.iter().map(|p| p[0].hypot(p[1])).filter(|&r| r > 0.9).collect();
        assert_eq!(rim_radii.len(), 2 * 48 * 4, "4 times the points around each rim");
        assert!(
            rim_radii.iter().all(|&r| (r - 1.0).abs() < 0.005),
            "{:?}",
            rim_radii.iter().fold(1.0f32, |m, &r| m.min(r))
        );
        // Around the side, four times as many facets, all close to the true circle.
        let middle = radii_at_middle(&part);
        assert!(middle.len() >= 48 * 4);
        assert!(middle.iter().all(|&r| (r - 1.0).abs() < 0.005));
        // Closed still: every edge has two faces.
        let edges = Edges::new(&part, None, 0.0);
        assert!(edges.faces.iter().all(|f| f.1 == 2));
        // Each child keeps its share of the source triangle.
        let o = part.origin.as_ref().unwrap();
        assert_eq!(o.len(), part.tris.len());
        assert!(o.iter().all(|(_, b)| b.iter().all(|c| (c.iter().sum::<f32>() - 1.0).abs() < 1e-5)));
    }

    #[test]
    fn colour_borders_and_corners_stay_where_they_are() {
        // The cylinder painted in two halves along x = 0; colours change across the side.
        let mut part = cylinder(48);
        let mut labels: Vec<u8> = (0..part.tris.len()).map(|t| u8::from(part.corners(t)[0][0] > 0.0)).collect();
        let border: Vec<u32> = {
            let first: Vec<usize> = (0..part.tris.len()).collect();
            let mut seen: HashMap<u32, u8> = HashMap::new();
            let mut out = vec![];
            for t in first {
                for v in part.tris[t] {
                    match seen.get(&v) {
                        Some(&l) if l != labels[t] => out.push(v),
                        None => {
                            seen.insert(v, labels[t]);
                        }
                        _ => {}
                    }
                }
            }
            out
        };
        let before: Vec<Vec3> = border.iter().map(|&v| part.positions[v as usize]).collect();
        smooth(&mut part, &labels, 30.0, 5);
        subdivide(&mut part, &mut labels, 30.0);
        assert_eq!(labels.len(), part.tris.len());
        // Smoothing never moves a vertex where colours and rims meet; subdividing keeps old indices first.
        for (&v, b) in border.iter().zip(&before) {
            let now = part.positions[v as usize];
            if b[2] == 0.0 || b[2] == 2.0 {
                assert!(geom::len(geom::sub(now, *b)) < 0.02, "{b:?} -> {now:?}");
            }
        }
    }

    #[test]
    fn taubin_keeps_the_volume_and_flat_faces_stay_flat() {
        let mut part = cylinder(16);
        let vol = |p: &MeshPart| -> f32 {
            (0..p.tris.len())
                .map(|t| {
                    let [a, b, c] = p.corners(t);
                    geom::dot(a, geom::cross(b, c)) / 6.0
                })
                .sum()
        };
        let v0 = vol(&part);
        let mut labels = vec![];
        subdivide(&mut part, &mut labels, 30.0);
        smooth(&mut part, &labels, 30.0, 10);
        let v1 = vol(&part);
        assert!((v1 - v0).abs() / v0 < 0.05, "{v0} -> {v1}");
        // The caps are flat and bounded by sharp rims: they stay on their planes.
        // Nothing drifts just off a cap: a point is on its plane or clearly up the side.
        assert!(part
            .positions
            .iter()
            .all(|p| p[2].abs() < 1e-5 || (p[2] - 2.0).abs() < 1e-5 || (p[2] > 0.05 && p[2] < 1.95)));
    }
}
