//! Cutting a part across a plane square to one axis. Triangles that cross the
//! plane are split, their new corners interpolated in every UV and colour
//! layer, and each side's opening is closed with a flat cap, holes included,
//! so both pieces stay solid.

use std::collections::{HashMap, HashSet};

use crate::geom::Vec3;
use crate::scene::Part;

/// Face corners (loops) with their UVs and colours, grown as the cut adds corners.
struct Loops {
    uv: Vec<Vec<[f32; 2]>>,
    color: Vec<Vec<[f32; 3]>>,
    count: u32,
}

impl Loops {
    fn new(part: &Part) -> Self {
        let used = part.tri_loops.iter().flatten().map(|&l| l + 1).max().unwrap_or(0);
        let stored =
            part.uv_layers.iter().map(Vec::len).chain(part.color_layers.iter().map(Vec::len)).max().unwrap_or(0);
        Self { uv: part.uv_layers.clone(), color: part.color_layers.clone(), count: used.max(stored as u32) }
    }

    /// A new corner `f` of the way from corner `a` to corner `b`.
    fn lerp(&mut self, a: u32, b: u32, f: f32) -> u32 {
        let mix = |x: f32, y: f32| x + (y - x) * f;
        for layer in &mut self.uv {
            let (p, q) = (layer[a as usize], layer[b as usize]);
            layer.push([mix(p[0], q[0]), mix(p[1], q[1])]);
        }
        for layer in &mut self.color {
            let (p, q) = (layer[a as usize], layer[b as usize]);
            layer.push(std::array::from_fn(|i| mix(p[i], q[i])));
        }
        self.count += 1;
        self.count - 1
    }
}

/// A triangle of one side: vertices, corners and material slot.
type Tri = ([u32; 3], [u32; 3], u16);

/// Splits `part` where `axis` equals `value` into the piece below and the piece
/// above, each with its opening capped. Vertices within `eps` of the plane are
/// moved onto it. A side the part does not reach comes back with no triangles.
pub fn split(part: &Part, axis: usize, value: f32, eps: f32) -> [Part; 2] {
    let mut positions = part.positions.clone();
    let mut dist: Vec<f32> = positions
        .iter_mut()
        .map(|p| {
            let d = p[axis] - value;
            if d.abs() <= eps {
                p[axis] = value;
                0.0
            } else {
                d
            }
        })
        .collect();
    let mut loops = Loops::new(part);
    let mut sides: [Vec<Tri>; 2] = [vec![], vec![]];
    let mut on_edge: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, (&t, &l)) in part.tris.iter().zip(&part.tri_loops).enumerate() {
        let src = part.tri_source.get(ti).copied().unwrap_or(0);
        let d = t.map(|v| dist[v as usize]);
        if d.iter().all(|&x| x <= 0.0) {
            sides[0].push((t, l, src));
            continue;
        }
        if d.iter().all(|&x| x >= 0.0) {
            sides[1].push((t, l, src));
            continue;
        }
        // Round the triangle, each corner goes to its side and each crossing to both.
        let mut poly: [Vec<(u32, u32)>; 2] = [vec![], vec![]];
        for a in 0..3 {
            let b = (a + 1) % 3;
            if d[a] <= 0.0 {
                poly[0].push((t[a], l[a]));
            }
            if d[a] >= 0.0 {
                poly[1].push((t[a], l[a]));
            }
            if (d[a] < 0.0 && d[b] > 0.0) || (d[a] > 0.0 && d[b] < 0.0) {
                let key = (t[a].min(t[b]), t[a].max(t[b]));
                // Worked out from the edge's own order, so both triangles on it get the same point.
                let v = match on_edge.get(&key) {
                    Some(&v) => v,
                    None => {
                        let (p, q) = (key.0 as usize, key.1 as usize);
                        let g = dist[p] / (dist[p] - dist[q]);
                        let mut x: Vec3 =
                            std::array::from_fn(|i| positions[p][i] + (positions[q][i] - positions[p][i]) * g);
                        x[axis] = value;
                        positions.push(x);
                        dist.push(0.0);
                        let v = positions.len() as u32 - 1;
                        on_edge.insert(key, v);
                        v
                    }
                };
                let corner = loops.lerp(l[a], l[b], d[a] / (d[a] - d[b]));
                poly[0].push((v, corner));
                poly[1].push((v, corner));
            }
        }
        // Each side of a cut triangle is convex: a fan covers it.
        for (side, corners) in poly.iter().enumerate() {
            for k in 1..corners.len().saturating_sub(1) {
                let (a, b, c) = (corners[0], corners[k], corners[k + 1]);
                if a.0 != b.0 && b.0 != c.0 && a.0 != c.0 {
                    sides[side].push(([a.0, b.0, c.0], [a.1, b.1, c.1], src));
                }
            }
        }
    }
    let on_plane: Vec<bool> = dist.iter().map(|&d| d == 0.0).collect();
    let mut out = sides.map(|tris| (tris, vec![]));
    for (side, (tris, caps)) in out.iter_mut().enumerate() {
        // Below the cut the cap faces up the axis; above, down it.
        *caps = cap(tris, &positions, &on_plane, axis, side == 0);
    }
    let [low, high] = out;
    [piece(part, &positions, &loops, low), piece(part, &positions, &loops, high)]
}

/// The cap that closes one side's opening on the plane.
fn cap(tris: &[Tri], positions: &[Vec3], on_plane: &[bool], axis: usize, faces_up: bool) -> Vec<Tri> {
    // Edges on the plane that only one triangle of this side uses are the opening's rim.
    let mut edges: HashMap<(u32, u32), (u32, u16)> = HashMap::new();
    for (t, l, src) in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if on_plane[a as usize] && on_plane[b as usize] {
                edges.insert((a, b), (l[(k + 1) % 3], *src));
            }
        }
    }
    // The cap runs each rim edge the other way: from b to a.
    let mut next: HashMap<u32, (u32, u32, u16)> = HashMap::new();
    for (&(a, b), &(corner, src)) in &edges {
        if !edges.contains_key(&(b, a)) {
            next.insert(b, (a, corner, src));
        }
    }
    // Plane coordinates with the cap's normal coming out of the screen.
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let (u, v) = if faces_up { (u, v) } else { (v, u) };
    let at = |i: u32| [positions[i as usize][u], positions[i as usize][v]];
    let mut rings: Vec<(Vec<u32>, u32, u16)> = vec![];
    let mut seen = HashSet::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if seen.contains(&start) {
            continue;
        }
        let (mut ring, mut at_vertex, mut closed) = (vec![], start, false);
        let (_, corner, src) = next[&start];
        while let Some(&(to, _, _)) = next.get(&at_vertex) {
            if !seen.insert(at_vertex) {
                break;
            }
            ring.push(at_vertex);
            at_vertex = to;
            if at_vertex == start {
                closed = true;
                break;
            }
        }
        // An open rim (the part was not closed) has nothing to cap.
        if closed && ring.len() >= 3 {
            rings.push((ring, corner, src));
        }
    }
    let area = |ring: &[u32]| -> f32 {
        (0..ring.len())
            .map(|k| {
                let (p, q) = (at(ring[k]), at(ring[(k + 1) % ring.len()]));
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f32>()
            * 0.5
    };
    let areas: Vec<f32> = rings.iter().map(|r| area(&r.0)).collect();
    // Counter-clockwise rings are outlines, clockwise ones holes in the smallest outline round them.
    let mut holes: Vec<Vec<usize>> = vec![vec![]; rings.len()];
    for (h, ring) in rings.iter().enumerate() {
        if areas[h] >= 0.0 {
            continue;
        }
        let p = at(ring.0[0]);
        let owner = (0..rings.len())
            .filter(|&o| areas[o] > 0.0 && inside(p, &rings[o].0, &at))
            .min_by(|&a, &b| areas[a].total_cmp(&areas[b]));
        if let Some(o) = owner {
            holes[o].push(h);
        }
    }
    let mut out = vec![];
    for (o, (ring, corner, src)) in rings.iter().enumerate() {
        if areas[o] <= 0.0 {
            continue;
        }
        let mut outline = ring.clone();
        let mut own: Vec<&Vec<u32>> = holes[o].iter().map(|&h| &rings[h].0).collect();
        // Rightmost hole first, as each bridge has to see the outline it joins.
        let right = |r: &Vec<u32>| r.iter().map(|&i| at(i)[0]).fold(f32::MIN, f32::max);
        own.sort_by(|a, b| right(b).total_cmp(&right(a)));
        for hole in own {
            bridge(&mut outline, hole, &at);
        }
        // One colour for the whole cap: the surface's where its rim starts.
        out.extend(earclip(&outline, &at).into_iter().map(|t| (t, [*corner; 3], *src)));
    }
    out
}

fn inside(p: [f32; 2], ring: &[u32], at: &impl Fn(u32) -> [f32; 2]) -> bool {
    let mut odd = false;
    for k in 0..ring.len() {
        let (a, b) = (at(ring[k]), at(ring[(k + 1) % ring.len()]));
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            odd = !odd;
        }
    }
    odd
}

/// Joins a hole to its outline with a two-way cut from the hole's rightmost
/// vertex to an outline vertex it sees, so one polygon can be ear-clipped.
fn bridge(outline: &mut Vec<u32>, hole: &[u32], at: &impl Fn(u32) -> [f32; 2]) {
    let hi = (0..hole.len()).max_by(|&a, &b| at(hole[a])[0].total_cmp(&at(hole[b])[0])).unwrap_or(0);
    let h = at(hole[hi]);
    // The nearest outline edge a ray to the right hits; its right end is a candidate.
    let mut best: Option<(f32, usize)> = None;
    for k in 0..outline.len() {
        let (pk, qk) = (k, (k + 1) % outline.len());
        let (p, q) = (at(outline[pk]), at(outline[qk]));
        if (p[1] <= h[1]) == (q[1] <= h[1]) {
            continue;
        }
        let x = p[0] + (h[1] - p[1]) * (q[0] - p[0]) / (q[1] - p[1]);
        if x >= h[0] && best.is_none_or(|(bx, _)| x < bx) {
            best = Some((x, if p[0] > q[0] { pk } else { qk }));
        }
    }
    let dist = |k: usize| {
        let p = at(outline[k]);
        (p[0] - h[0]).powi(2) + (p[1] - h[1]).powi(2)
    };
    let mut join = match best {
        Some((_, k)) => k,
        None => (0..outline.len()).min_by(|&a, &b| dist(a).total_cmp(&dist(b))).unwrap_or(0),
    };
    // An outline vertex inside the triangle hole–hit–candidate would block the view: take the nearest such.
    if let Some((x, _)) = best {
        let c = at(outline[join]);
        let tri = [h, [x, h[1]], c];
        if let Some(k) = (0..outline.len())
            .filter(|&k| k != join && in_triangle(at(outline[k]), tri))
            .min_by(|&a, &b| dist(a).total_cmp(&dist(b)))
        {
            join = k;
        }
    }
    let mut merged = Vec::with_capacity(outline.len() + hole.len() + 2);
    merged.extend_from_slice(&outline[..=join]);
    merged.extend(hole[hi..].iter().chain(&hole[..=hi]));
    merged.extend_from_slice(&outline[join..]);
    *outline = merged;
}

fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn in_triangle(p: [f32; 2], t: [[f32; 2]; 3]) -> bool {
    let (a, b, c) = (cross(t[0], t[1], p), cross(t[1], t[2], p), cross(t[2], t[0], p));
    (a >= 0.0 && b >= 0.0 && c >= 0.0) || (a <= 0.0 && b <= 0.0 && c <= 0.0)
}

/// Ear clipping of a counter-clockwise polygon (bridged holes allowed). When no
/// ear is left, as with collinear runs, the next corner is clipped anyway, so it
/// always ends and every rim edge is covered.
fn earclip(poly: &[u32], at: &impl Fn(u32) -> [f32; 2]) -> Vec<[u32; 3]> {
    let n = poly.len();
    if n < 3 {
        return vec![];
    }
    let pts: Vec<[f32; 2]> = poly.iter().map(|&i| at(i)).collect();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let (mut left, mut i, mut stall) = (n, 0, 0);
    let ear = |a: usize, b: usize, c: usize, next: &[usize]| {
        let tri = [pts[a], pts[b], pts[c]];
        if cross(tri[0], tri[1], tri[2]) <= 0.0 {
            return false;
        }
        let mut k = next[c];
        while k != a {
            // A bridge repeats vertices: those are the ear's own corners, not in its way.
            let same = [a, b, c].iter().any(|&x| poly[x] == poly[k]);
            if !same && in_triangle(pts[k], tri) {
                return false;
            }
            k = next[k];
        }
        true
    };
    while left > 3 {
        let (a, c) = (prev[i], next[i]);
        if ear(a, i, c, &next) || stall > left {
            out.push([poly[a], poly[i], poly[c]]);
            next[a] = c;
            prev[c] = a;
            left -= 1;
            stall = 0;
            i = c;
        } else {
            i = next[i];
            stall += 1;
        }
    }
    out.push([poly[prev[i]], poly[i], poly[next[i]]]);
    out
}

/// One side as a part of its own: only the vertices it uses, the original's materials.
fn piece(part: &Part, positions: &[Vec3], loops: &Loops, (tris, caps): (Vec<Tri>, Vec<Tri>)) -> Part {
    let mut remap = vec![u32::MAX; positions.len()];
    let mut kept = Vec::new();
    let mut out = Part {
        name: part.name.clone(),
        sources: part.sources.clone(),
        uv_layers: loops.uv.clone(),
        color_layers: loops.color.clone(),
        object: part.object,
        ..Default::default()
    };
    for (t, l, src) in tris.into_iter().chain(caps) {
        out.tris.push(t.map(|v| {
            if remap[v as usize] == u32::MAX {
                remap[v as usize] = kept.len() as u32;
                kept.push(positions[v as usize]);
            }
            remap[v as usize]
        }));
        out.tri_loops.push(l);
        out.tri_source.push(src);
    }
    out.positions = kept;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ColorSource;

    /// A closed axis-aligned box from `lo` to `hi`, faces outwards.
    fn cube(lo: Vec3, hi: Vec3) -> Part {
        let c = |i: usize| {
            [
                if i & 1 > 0 { hi[0] } else { lo[0] },
                if i & 2 > 0 { hi[1] } else { lo[1] },
                if i & 4 > 0 { hi[2] } else { lo[2] },
            ]
        };
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let tris: Vec<[u32; 3]> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
        let n = tris.len() as u32;
        Part {
            name: "c".into(),
            positions: (0..8).map(c).collect(),
            tri_loops: (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: vec![0; tris.len()],
            tris,
            sources: vec![ColorSource::Flat([0.5, 0.2, 0.1])],
            uv_layers: vec![vec![[0.25, 0.75]; n as usize * 3]],
            ..Default::default()
        }
    }

    /// Every edge used once each way: a closed, consistently wound surface.
    fn closed(p: &Part) -> bool {
        let mut e = HashMap::new();
        for t in &p.tris {
            for k in 0..3 {
                *e.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        e.iter().all(|(&(a, b), &n)| n == 1 && e.get(&(b, a)) == Some(&1))
    }

    fn volume(p: &Part) -> f32 {
        p.tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| p.positions[v as usize]);
                crate::geom::dot(a, crate::geom::cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn a_cut_box_gives_two_closed_boxes() {
        for axis in 0..3 {
            let [low, high] = split(&cube([0.0; 3], [10.0, 20.0, 30.0]), axis, [4.0, 5.0, 6.0][axis], 1e-5);
            assert!(closed(&low) && closed(&high), "axis {axis}");
            let whole = 10.0 * 20.0 * 30.0;
            let below = whole * [0.4, 0.25, 0.2][axis];
            assert!((volume(&low) - below).abs() < 1e-2, "{} vs {below}", volume(&low));
            assert!((volume(&high) - (whole - below)).abs() < 1e-2);
            // New corners got their own UVs.
            assert!(low.tri_loops.iter().flatten().all(|&l| (l as usize) < low.uv_layers[0].len()));
        }
    }

    #[test]
    fn a_hollow_box_keeps_its_cavity_in_the_cap() {
        // A box with a box-shaped cavity: the inner shell faces inwards.
        let outer = cube([0.0; 3], [10.0; 3]);
        let mut inner = cube([3.0; 3], [7.0; 3]);
        for t in &mut inner.tris {
            t.swap(1, 2);
        }
        let mut both = outer.clone();
        let base = both.positions.len() as u32;
        both.positions.extend(&inner.positions);
        let loops = both.tri_loops.len() as u32 * 3;
        both.tris.extend(inner.tris.iter().map(|t| t.map(|v| v + base)));
        both.tri_loops.extend(inner.tri_loops.iter().map(|l| l.map(|v| v + loops)));
        both.tri_source.extend(&inner.tri_source);
        both.uv_layers[0].extend(&inner.uv_layers[0]);
        let [low, high] = split(&both, 2, 5.0, 1e-5);
        assert!(closed(&low) && closed(&high));
        let solid = 1000.0 - 64.0;
        assert!((volume(&low) - solid / 2.0).abs() < 1e-2, "{}", volume(&low));
        assert!((volume(&high) - solid / 2.0).abs() < 1e-2);
    }

    #[test]
    fn a_plane_past_the_part_leaves_one_side_empty() {
        let [low, high] = split(&cube([0.0; 3], [1.0; 3]), 2, 5.0, 1e-5);
        assert_eq!(low.tris.len(), 12);
        assert!(high.tris.is_empty());
    }
}
