//! Cutting a part across an arbitrary plane. Triangles that cross the
//! plane are split, their new corners interpolated in every UV and colour
//! layer, and each side's opening is closed with a flat cap, holes included,
//! so both pieces stay solid.

use std::collections::{HashMap, HashSet};

use crate::geom::{self, Vec3};
use crate::scene::Part;

/// A plane in the same coordinates as the mesh. Negative distances are the low side.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub point: Vec3,
    pub normal: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutError {
    InvalidPlane,
    Misses,
    InvalidGeometry,
}

impl Plane {
    pub fn new(point: Vec3, normal: Vec3) -> Result<Self, CutError> {
        let length = geom::len(normal);
        if !point.iter().chain(&normal).all(|v| v.is_finite()) || !length.is_finite() || length < 1e-8 {
            return Err(CutError::InvalidPlane);
        }
        Ok(Self { point, normal: geom::scale(normal, 1.0 / length) })
    }

    pub fn distance(self, p: Vec3) -> f32 {
        geom::dot(geom::sub(p, self.point), self.normal)
    }

    pub(crate) fn basis(self) -> (Vec3, Vec3) {
        let seed = if self.normal[0].abs() < 0.8 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let u = geom::normalize(geom::cross(seed, self.normal));
        (u, geom::cross(self.normal, u))
    }
}

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

/// One side of a cut: the piece and, per triangle, the surface triangle it came from.
type Side = (Part, Vec<Option<u32>>);

/// A connector's opening in one side's cap: a closed ring on the plane (a
/// convex polygon, in the part's coordinates) and the wall that runs from it
/// by `depth`. Into the side's solid it is a socket; out of it, a pin. Its
/// walls and end close the side again, so the piece stays solid.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub ring: Vec<Vec3>,
    pub depth: Vec3,
    /// 0 for the low side, 1 for the high one.
    pub side: usize,
}

/// Splits `part` where `axis` equals `value` into the piece below and the piece
/// above, each with its opening capped. Vertices within `eps` of the plane are
/// moved onto it. A side the part does not reach comes back with no triangles.
pub fn split(part: &Part, axis: usize, value: f32, eps: f32) -> [Part; 2] {
    split_with_origins(part, axis, value, eps).map(|(part, _)| part)
}

/// As `split`, with each surface triangle's parent index; new caps have no parent.
pub(crate) fn split_with_origins(part: &Part, axis: usize, value: f32, eps: f32) -> [(Part, Vec<Option<u32>>); 2] {
    let mut point = [0.0; 3];
    let mut normal = [0.0; 3];
    point[axis] = value;
    normal[axis] = 1.0;
    split_plane_with_origins(part, Plane { point, normal }, eps)
}

pub(crate) fn split_plane_with_origins(part: &Part, plane: Plane, eps: f32) -> [(Part, Vec<Option<u32>>); 2] {
    split_plane_with_features(part, plane, eps, &[]).0
}

/// The open rims a plane leaves on the low side of `part`, as closed loops of
/// points on the plane: section outlines and the holes in them.
pub(crate) fn section_loops(part: &Part, plane: Plane, eps: f32) -> Vec<Vec<Vec3>> {
    let sliced = slice(part, plane, eps);
    let on_plane: Vec<bool> = sliced.dist.iter().map(|&d| d == 0.0).collect();
    rims(&sliced.sides[0], &on_plane)
        .into_iter()
        .map(|(ring, _, _)| ring.iter().map(|&v| sliced.positions[v as usize]).collect())
        .collect()
}

/// As `split_plane_with_origins`, with connector openings in the caps. Also
/// says how many features found no section to sit in (they are left out).
pub(crate) fn split_plane_with_features(
    part: &Part,
    plane: Plane,
    eps: f32,
    features: &[Feature],
) -> ([Side; 2], usize) {
    let Sliced { part: welded, mut positions, mut dist, loops, sides, origins } = slice(part, plane, eps);
    let part = &welded;
    // Feature rings sit on the plane; their far ends do not.
    // Per side: each feature's ring on the plane, its far end and its depth.
    type Opening = (Vec<u32>, Vec<u32>, Vec3);
    let mut rings: [Vec<Opening>; 2] = [vec![], vec![]];
    for f in features.iter().filter(|f| f.side < 2 && f.ring.len() >= 3) {
        let base = positions.len() as u32;
        let n = f.ring.len() as u32;
        for &p in &f.ring {
            positions.push(geom::sub(p, geom::scale(plane.normal, plane.distance(p))));
            dist.push(0.0);
        }
        for &p in &f.ring {
            positions.push(geom::add(p, f.depth));
            dist.push(f32::NAN);
        }
        rings[f.side].push(((base..base + n).collect(), (base + n..base + 2 * n).collect(), f.depth));
    }
    let on_plane: Vec<bool> = dist.iter().map(|&d| d == 0.0).collect();
    let mut unplaced = 0;
    let mut out = sides.map(|tris| (tris, vec![]));
    for (side, (tris, caps)) in out.iter_mut().enumerate() {
        // Below the cut the cap faces up the axis; above, down it.
        let holes: Vec<Vec<u32>> = rings[side].iter().map(|r| r.0.clone()).collect();
        let (cap_tris, owners) = cap(tris, &positions, &on_plane, plane, side == 0, &holes);
        *caps = cap_tris;
        for ((ring, end, depth), owner) in rings[side].iter().zip(owners) {
            let Some((corner, src)) = owner else {
                unplaced += 1;
                continue;
            };
            caps.extend(
                walls(&positions, ring, end, *depth, plane.normal, side).into_iter().map(|t| (t, [corner; 3], src)),
            );
        }
    }
    let [low, high] = out.map(|side| piece(part, &positions, &loops, side));
    let [mut low_origins, mut high_origins] = origins;
    low_origins.resize(low.tris.len(), None);
    high_origins.resize(high.tris.len(), None);
    ([(low, low_origins), (high, high_origins)], unplaced)
}

/// The wall from a feature's ring to its far end and the end that closes it,
/// wound so the side stays a closed, outward-facing surface.
fn walls(positions: &[Vec3], ring: &[u32], end: &[u32], depth: Vec3, normal: Vec3, side: usize) -> Vec<[u32; 3]> {
    // Into the side's solid (a socket) its faces look into the hole; out of it (a pin), away from it.
    let into = (geom::dot(depth, normal) < 0.0) == (side == 0);
    let n = ring.len();
    let at = |i: u32| positions[i as usize];
    let centre = geom::scale(ring.iter().fold([0.0; 3], |c, &i| geom::add(c, at(i))), 1.0 / n as f32);
    let axis = geom::normalize(depth);
    let mut out = vec![];
    let mut face = |t: [u32; 3], want: Vec3| {
        let [a, b, c] = t.map(at);
        let normal = geom::cross(geom::sub(b, a), geom::sub(c, a));
        out.push(if geom::dot(normal, want) >= 0.0 { t } else { [t[0], t[2], t[1]] });
    };
    for k in 0..n {
        let j = (k + 1) % n;
        let mid = geom::scale(geom::add(at(ring[k]), at(ring[j])), 0.5);
        let off = geom::sub(mid, centre);
        let radial = geom::sub(off, geom::scale(axis, geom::dot(off, axis)));
        let want = if into { geom::scale(radial, -1.0) } else { radial };
        face([ring[k], ring[j], end[j]], want);
        face([ring[k], end[j], end[k]], want);
    }
    let want = if into { geom::scale(axis, -1.0) } else { axis };
    for k in 1..n - 1 {
        face([end[0], end[k], end[k + 1]], want);
    }
    out
}

/// A part split by a plane, before its openings are capped.
struct Sliced {
    part: Part,
    positions: Vec<Vec3>,
    /// Distance of each position to the plane; exactly 0 on it.
    dist: Vec<f32>,
    loops: Loops,
    sides: [Vec<Tri>; 2],
    origins: [Vec<Option<u32>>; 2],
}

fn slice(part: &Part, plane: Plane, eps: f32) -> Sliced {
    // Importers may duplicate positions at UV/material seams. Topology shares
    // positions, while face-corner attributes and source face indices stay separate.
    let mut welded = part.clone();
    let mut by_position = HashMap::new();
    let mut remap = Vec::with_capacity(part.positions.len());
    welded.positions.clear();
    for &p in &part.positions {
        let key = p.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
        let i = *by_position.entry(key).or_insert_with(|| {
            let i = welded.positions.len() as u32;
            welded.positions.push(p);
            i
        });
        remap.push(i);
    }
    for t in &mut welded.tris {
        *t = t.map(|v| remap[v as usize]);
    }
    let part = &welded;
    let mut positions = part.positions.clone();
    let mut dist: Vec<f32> = positions
        .iter_mut()
        .map(|p| {
            let d = plane.distance(*p);
            if d.abs() <= eps {
                *p = geom::sub(*p, geom::scale(plane.normal, d));
                0.0
            } else {
                d
            }
        })
        .collect();
    let mut loops = Loops::new(part);
    let mut sides: [Vec<Tri>; 2] = [vec![], vec![]];
    let mut origins: [Vec<Option<u32>>; 2] = [vec![], vec![]];
    let mut on_edge: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, (&t, &l)) in part.tris.iter().zip(&part.tri_loops).enumerate() {
        let src = part.tri_source.get(ti).copied().unwrap_or(0);
        let d = t.map(|v| dist[v as usize]);
        if d.iter().all(|&x| x <= 0.0) {
            sides[0].push((t, l, src));
            origins[0].push(Some(ti as u32));
            continue;
        }
        if d.iter().all(|&x| x >= 0.0) {
            sides[1].push((t, l, src));
            origins[1].push(Some(ti as u32));
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
                        x = geom::sub(x, geom::scale(plane.normal, plane.distance(x)));
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
                    origins[side].push(Some(ti as u32));
                }
            }
        }
    }
    Sliced { part: part.clone(), positions, dist, loops, sides, origins }
}

/// The closed rims of one side's opening: each ring of vertices runs the way
/// the cap has to (opposite to the side's own edges), with the corner and
/// material the cap takes there.
fn rims(tris: &[Tri], on_plane: &[bool]) -> Vec<(Vec<u32>, u32, u16)> {
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
    rings
}

/// The cap that closes one side's opening on the plane, with `extra` rings
/// left open as holes for connectors. For each of those, the corner and
/// material of the cap around it, or `None` when it falls in no section.
#[allow(clippy::type_complexity)]
fn cap(
    tris: &[Tri],
    positions: &[Vec3],
    on_plane: &[bool],
    plane: Plane,
    faces_up: bool,
    extra: &[Vec<u32>],
) -> (Vec<Tri>, Vec<Option<(u32, u16)>>) {
    // Plane coordinates with the cap's normal coming out of the screen.
    let (u, v) = plane.basis();
    let (u, v) = if faces_up { (u, v) } else { (v, u) };
    let at = |i: u32| {
        let p = geom::sub(positions[i as usize], plane.point);
        [geom::dot(p, u), geom::dot(p, v)]
    };
    let mut rings = rims(tris, on_plane);
    let rim_count = rings.len();
    let area = |ring: &[u32]| -> f32 {
        (0..ring.len())
            .map(|k| {
                let (p, q) = (at(ring[k]), at(ring[(k + 1) % ring.len()]));
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f32>()
            * 0.5
    };
    // Connector rings are holes: clockwise, like the cavities in a section.
    for ring in extra {
        let mut ring = ring.clone();
        if area(&ring) > 0.0 {
            ring.reverse();
        }
        rings.push((ring, 0, 0));
    }
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
        // A connector inside a cavity of its outline has no material round it.
        let in_cavity = |o: usize| {
            (0..rim_count).any(|c| {
                areas[c] < 0.0 && c != h && inside(p, &rings[c].0, &at) && inside(at(rings[c].0[0]), &rings[o].0, &at)
            })
        };
        if let Some(o) = owner.filter(|&o| h < rim_count || !in_cavity(o)) {
            holes[o].push(h);
        }
    }
    let owners = (rim_count..rings.len())
        .map(|h| holes.iter().position(|own| own.contains(&h)).map(|o| (rings[o].1, rings[o].2)))
        .collect();
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
        for (k, hole) in own.iter().enumerate() {
            bridge(&mut outline, hole, &own[k + 1..], &at);
        }
        // One colour for the whole cap: the surface's where its rim starts.
        out.extend(earclip(&outline, &at).into_iter().map(|t| (t, [*corner; 3], *src)));
    }
    (out, owners)
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
/// Outline vertices are tried nearest first; a cut is taken when it leaves
/// the vertex inside its corner of the outline and crosses no edge of the
/// outline, of this hole or of the holes still to join (`rest`).
fn bridge(outline: &mut Vec<u32>, hole: &[u32], rest: &[&Vec<u32>], at: &impl Fn(u32) -> [f32; 2]) {
    let hi = (0..hole.len()).max_by(|&a, &b| at(hole[a])[0].total_cmp(&at(hole[b])[0])).unwrap_or(0);
    let m = hole[hi];
    let h = at(m);
    let dist = |k: usize| {
        let p = at(outline[k]);
        (p[0] - h[0]).powi(2) + (p[1] - h[1]).powi(2)
    };
    let mut order: Vec<usize> = (0..outline.len()).collect();
    order.sort_by(|&a, &b| dist(a).total_cmp(&dist(b)));
    let n = outline.len();
    // The cut into vertex k runs inside the outline's corner there (counter-clockwise outline).
    let in_corner = |k: usize| {
        let o = at(outline[k]);
        let (prev, next) = (at(outline[(k + n - 1) % n]), at(outline[(k + 1) % n]));
        let angle = |p: [f32; 2]| (p[1] - o[1]).atan2(p[0] - o[0]);
        let turn = |from: f32, to: f32| (to - from).rem_euclid(std::f32::consts::TAU);
        let (e1, e0, d) = (angle(next), angle(prev), angle(h));
        let open = turn(e1, e0);
        let towards = turn(e1, d);
        towards > 1e-6 && towards < open - 1e-6
    };
    let edges = || {
        let ring = |r: &[u32]| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()])).collect::<Vec<_>>();
        let mut all = ring(outline);
        all.extend(ring(hole));
        for r in rest {
            all.extend(ring(r));
        }
        all
    };
    let all = edges();
    let clear = |o: u32| {
        let p = at(o);
        all.iter().all(|&(a, b)| a == o || b == o || a == m || b == m || !touches(h, p, at(a), at(b)))
    };
    let join = order.iter().copied().find(|&k| in_corner(k) && clear(outline[k])).unwrap_or(order[0]);
    let mut merged = Vec::with_capacity(outline.len() + hole.len() + 2);
    merged.extend_from_slice(&outline[..=join]);
    merged.extend(hole[hi..].iter().chain(&hole[..=hi]));
    merged.extend_from_slice(&outline[join..]);
    *outline = merged;
}

/// Segments `p`–`q` and `a`–`b` meet: cross, touch or overlap.
fn touches(p: [f32; 2], q: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let side = |x: [f32; 2], y: [f32; 2], z: [f32; 2]| {
        let c = (y[0] as f64 - x[0] as f64) * (z[1] as f64 - x[1] as f64)
            - (y[1] as f64 - x[1] as f64) * (z[0] as f64 - x[0] as f64);
        if c.abs() < 1e-12 {
            0
        } else {
            c.signum() as i32
        }
    };
    let within = |x: [f32; 2], y: [f32; 2], z: [f32; 2]| {
        z[0] >= x[0].min(y[0]) && z[0] <= x[0].max(y[0]) && z[1] >= x[1].min(y[1]) && z[1] <= x[1].max(y[1])
    };
    let (d1, d2, d3, d4) = (side(a, b, p), side(a, b, q), side(p, q, a), side(p, q, b));
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    (d1 == 0 && within(a, b, p))
        || (d2 == 0 && within(a, b, q))
        || (d3 == 0 && within(p, q, a))
        || (d4 == 0 && within(p, q, b))
}

fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn in_triangle(p: [f32; 2], t: [[f32; 2]; 3]) -> bool {
    let (a, b, c) = (cross(t[0], t[1], p), cross(t[1], t[2], p), cross(t[2], t[0], p));
    (a >= 0.0 && b >= 0.0 && c >= 0.0) || (a <= 0.0 && b <= 0.0 && c <= 0.0)
}

/// Ear clipping of a counter-clockwise polygon (bridged holes allowed).
/// A stalled triangulation is rejected by the caller's closure validation.
fn earclip(poly: &[u32], at: &impl Fn(u32) -> [f32; 2]) -> Vec<[u32; 3]> {
    if poly.len() < 3 {
        return vec![];
    }
    let pts: Vec<_> = poly.iter().map(|&v| at(v)).collect();
    let span = (0..2)
        .map(|axis| {
            pts.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max)
                - pts.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min)
        })
        .fold(0.0f32, f32::max);
    let eps = span * 2e-7;
    // Remove round-off collinearity while triangulating, then put every rim
    // segment back. Simply deleting these vertices would create T junctions.
    let mut kept: Vec<usize> = (0..poly.len()).collect();
    loop {
        let removable = (0..kept.len()).find(|&i| {
            let [a, b, c] =
                [kept[(i + kept.len() - 1) % kept.len()], kept[i], kept[(i + 1) % kept.len()]].map(|i| pts[i]);
            let length = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt();
            length > eps
                && cross(a, b, c).abs() <= eps * length
                && (b[0] - a[0]) * (b[0] - c[0]) + (b[1] - a[1]) * (b[1] - c[1]) <= 0.0
        });
        if kept.len() <= 3 {
            break;
        }
        if let Some(i) = removable {
            kept.remove(i);
        } else {
            break;
        }
    }
    let simple: Vec<_> = kept.iter().map(|&i| poly[i]).collect();
    let mut out = earclip_simple(&simple, at);
    if out.is_empty() {
        return out;
    }
    for i in 0..kept.len() {
        let (start, end) = (kept[i], kept[(i + 1) % kept.len()]);
        let mut chain = vec![poly[start]];
        let mut k = (start + 1) % poly.len();
        while k != end {
            chain.push(poly[k]);
            k = (k + 1) % poly.len();
        }
        chain.push(poly[end]);
        if chain.len() == 2 {
            continue;
        }
        let found = out.iter().enumerate().find_map(|(ti, t)| {
            (0..3).find(|&j| t[j] == poly[start] && t[(j + 1) % 3] == poly[end]).map(|j| (ti, t[(j + 2) % 3]))
        });
        let Some((ti, tip)) = found else { return vec![] };
        out.swap_remove(ti);
        out.extend(chain.windows(2).map(|edge| [edge[0], edge[1], tip]));
    }
    out
}

fn earclip_simple(poly: &[u32], at: &impl Fn(u32) -> [f32; 2]) -> Vec<[u32; 3]> {
    let n = poly.len();
    if n < 3 {
        return vec![];
    }
    let pts: Vec<[f32; 2]> = poly.iter().map(|&i| at(i)).collect();
    let span = (0..2)
        .map(|axis| {
            pts.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max)
                - pts.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min)
        })
        .fold(0.0f32, f32::max);
    let tolerance = span * span * 1e-12;
    let cross64 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        (b[0] as f64 - a[0] as f64) * (c[1] as f64 - a[1] as f64)
            - (b[1] as f64 - a[1] as f64) * (c[0] as f64 - a[0] as f64)
    };
    let mut remaining_area: f64 = (0..n).map(|i| cross64([0.0; 2], pts[i], pts[(i + 1) % n])).sum();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let (mut left, mut i, mut stall) = (n, 0, 0);
    let ear = |a: usize, b: usize, c: usize, next: &[usize]| {
        let tri = [pts[a], pts[b], pts[c]];
        if cross(tri[0], tri[1], tri[2]) <= tolerance {
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
        if stall > left {
            return vec![];
        }
        let area = cross64(pts[a], pts[i], pts[c]);
        // Never remove the last off-line corner, leaving a collinear remainder.
        if remaining_area - area > tolerance as f64 && ear(a, i, c, &next) {
            out.push([poly[a], poly[i], poly[c]]);
            remaining_area -= area;
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

/// Reject open, inconsistently wound, non-finite or degenerate results before commit.
pub(crate) fn valid_solid(part: &Part, eps: f32) -> bool {
    if part.tris.is_empty() || part.positions.iter().flatten().any(|v| !v.is_finite()) {
        return false;
    }
    let mut edges = HashMap::new();
    let mut volume = 0.0;
    let origin = part.positions[0];
    for (i, t) in part.tris.iter().enumerate() {
        let c = part.corners(i);
        if geom::area(&c) <= eps * eps {
            return false;
        }
        volume +=
            geom::dot(geom::sub(c[0], origin), geom::cross(geom::sub(c[1], origin), geom::sub(c[2], origin))) / 6.0;
        for k in 0..3 {
            *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0u32) += 1;
        }
    }
    volume.is_finite()
        && volume.abs() > eps.powi(3)
        && edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
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
        plate: part.plate,
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

    #[test]
    fn oblique_and_free_planes_preserve_volume_uvs_and_closed_edges() {
        let original = cube([0.0; 3], [10.0; 3]);
        for normal in [[1.0, 0.0, 1.0], [0.3, -0.7, 0.64], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]] {
            for point in [[5.0; 3], [0.001, 0.001, 0.001], [9.999; 3]] {
                let plane = Plane::new(point, normal).unwrap();
                let [(low, _), (high, _)] = split_plane_with_origins(&original, plane, 1e-6);
                assert!(valid_solid(&low, 1e-6) && valid_solid(&high, 1e-6), "{point:?} {normal:?}");
                assert!((volume(&low) + volume(&high) - 1000.0).abs() < 0.01);
                assert!(low.tri_loops.iter().flatten().all(|&i| (i as usize) < low.uv_layers[0].len()));
            }
        }
    }

    /// A regular polygon of `n` corners round `c` on the plane z = c[2].
    fn ring(c: Vec3, r: f32, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin(), c[2]]
            })
            .collect()
    }

    fn polygon_area(n: usize, r: f32) -> f32 {
        0.5 * n as f32 * r * r * (std::f32::consts::TAU / n as f32).sin()
    }

    #[test]
    fn a_socket_and_a_pin_leave_both_halves_closed_with_their_volumes() {
        let plane = Plane::new([0.0, 0.0, 5.0], [0.0, 0.0, 1.0]).unwrap();
        for n in [4, 32] {
            // A socket 3 deep into the low half and a pin 2.5 long down from the high half.
            let features = [
                Feature { ring: ring([5.0, 5.0, 5.0], 1.6, n), depth: [0.0, 0.0, -3.0], side: 0 },
                Feature { ring: ring([5.0, 5.0, 5.0], 1.4, n), depth: [0.0, 0.0, -2.5], side: 1 },
            ];
            let ([(low, low_o), (high, _)], unplaced) =
                split_plane_with_features(&cube([0.0; 3], [10.0; 3]), plane, 1e-6, &features);
            assert_eq!(unplaced, 0);
            assert!(valid_solid(&low, 1e-6) && valid_solid(&high, 1e-6), "n = {n}");
            let socket = polygon_area(n, 1.6) * 3.0;
            let pin = polygon_area(n, 1.4) * 2.5;
            assert!((volume(&low) - (500.0 - socket)).abs() < 1e-2, "{} vs {}", volume(&low), 500.0 - socket);
            assert!((volume(&high) - (500.0 + pin)).abs() < 1e-2, "{} vs {}", volume(&high), 500.0 + pin);
            // The new faces are not from the surface: they take the model's colours, no brush mark.
            assert_eq!(low_o.len(), low.tris.len());
            assert!(low_o.iter().filter(|o| o.is_none()).count() > n);
            assert!(low.tri_loops.iter().flatten().all(|&l| (l as usize) < low.uv_layers[0].len()));
        }
        // On a free plane too.
        let plane = Plane::new([5.0; 3], [0.3, -0.2, 0.9]).unwrap();
        let (u, v) = plane.basis();
        let at =
            |a: f32, r: f32| geom::add([5.0; 3], geom::add(geom::scale(u, r * a.cos()), geom::scale(v, r * a.sin())));
        let round: Vec<Vec3> = (0..24).map(|k| at(k as f32 / 24.0 * std::f32::consts::TAU, 1.5)).collect();
        let features = [
            Feature { ring: round.clone(), depth: geom::scale(plane.normal, -3.0), side: 0 },
            Feature { ring: round, depth: geom::scale(plane.normal, 3.0), side: 1 },
        ];
        let ([(low, _), (high, _)], unplaced) =
            split_plane_with_features(&cube([0.0; 3], [10.0; 3]), plane, 1e-6, &features);
        assert_eq!(unplaced, 0);
        assert!(valid_solid(&low, 1e-6) && valid_solid(&high, 1e-6));
        assert!((volume(&low) + volume(&high) - (1000.0 - 2.0 * polygon_area(24, 1.5) * 3.0)).abs() < 2e-2);
    }

    #[test]
    fn several_openings_in_one_cap_close_on_every_axis() {
        // Pseudo-random spots on a 40 × 30 section, cut across X, Y and Z, on both sides.
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for axis in 0..3 {
            let mut size = [30.0f32; 3];
            size[axis] = 100.0;
            size[(axis + 1) % 3] = 40.0;
            let part = cube([0.0; 3], size);
            let mut point = [0.0; 3];
            point[axis] = 50.0;
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            let plane = Plane::new(point, normal).unwrap();
            let (u, v) = plane.basis();
            for _ in 0..24 {
                // Up to four openings 2.7 mm across, kept 8 mm apart and 4 mm inside the edges.
                let mut centres: Vec<Vec3> = vec![];
                for _ in 0..(1 + (next() * 4.0) as usize) {
                    let mut c = point;
                    c[(axis + 1) % 3] = 4.0 + next() * 32.0;
                    c[(axis + 2) % 3] = 4.0 + next() * 22.0;
                    if centres.iter().all(|o| geom::len(geom::sub(*o, c)) > 8.0) {
                        centres.push(c);
                    }
                }
                let ring = |c: Vec3| -> Vec<Vec3> {
                    (0..32)
                        .map(|k| {
                            let a = k as f32 / 32.0 * std::f32::consts::TAU;
                            geom::add(c, geom::add(geom::scale(u, 2.7 * a.cos()), geom::scale(v, 2.7 * a.sin())))
                        })
                        .collect()
                };
                let features: Vec<Feature> = centres
                    .iter()
                    .flat_map(|&c| {
                        [0, 1].map(|side| Feature {
                            ring: ring(c),
                            depth: geom::scale(normal, if side == 0 { -6.0 } else { 6.0 }),
                            side,
                        })
                    })
                    .collect();
                let ([(low, _), (high, _)], unplaced) = split_plane_with_features(&part, plane, 1e-4, &features);
                assert_eq!(unplaced, 0);
                assert!(valid_solid(&low, 1e-4) && valid_solid(&high, 1e-4), "axis {axis}: {centres:?}");
            }
        }
    }

    #[test]
    fn a_feature_outside_the_section_or_in_a_cavity_is_left_out() {
        let plane = Plane::new([0.0, 0.0, 5.0], [0.0, 0.0, 1.0]).unwrap();
        let outside = [Feature { ring: ring([20.0, 5.0, 5.0], 1.0, 8), depth: [0.0, 0.0, -2.0], side: 0 }];
        let (_, unplaced) = split_plane_with_features(&cube([0.0; 3], [10.0; 3]), plane, 1e-6, &outside);
        assert_eq!(unplaced, 1);
        // A hollow box: the cavity spans 3..7, so its section is a ring.
        let mut shell = cube([0.0; 3], [10.0; 3]);
        let cavity = cube([3.0; 3], [7.0; 3]);
        let base = shell.positions.len() as u32;
        let loops = shell.tris.len() as u32 * 3;
        shell.positions.extend(cavity.positions);
        shell.tris.extend(cavity.tris.iter().map(|t| [t[0] + base, t[2] + base, t[1] + base]));
        shell.tri_loops.extend(cavity.tri_loops.iter().map(|t| t.map(|v| v + loops)));
        shell.tri_source.extend(cavity.tri_source);
        shell.uv_layers[0].extend(&cavity.uv_layers[0]);
        let in_cavity = [Feature { ring: ring([5.0, 5.0, 5.0], 1.0, 8), depth: [0.0, 0.0, -1.0], side: 0 }];
        assert_eq!(split_plane_with_features(&shell, plane, 1e-6, &in_cavity).1, 1);
        let in_wall = [Feature { ring: ring([1.5, 5.0, 5.0], 0.8, 8), depth: [0.0, 0.0, -2.0], side: 0 }];
        let ([(low, _), _], unplaced) = split_plane_with_features(&shell, plane, 1e-6, &in_wall);
        assert_eq!(unplaced, 0);
        assert!(valid_solid(&low, 1e-6));
        // The loops of the section: the outline and the cavity.
        assert_eq!(section_loops(&shell, plane, 1e-6).len(), 2);
    }

    #[test]
    fn a_free_plane_keeps_holes_open_and_rejects_invalid_geometry() {
        let mut shell = cube([0.0; 3], [10.0; 3]);
        let cavity = cube([3.0; 3], [7.0; 3]);
        let base = shell.positions.len() as u32;
        let loops = shell.tris.len() as u32 * 3;
        shell.positions.extend(cavity.positions);
        shell.tris.extend(cavity.tris.iter().map(|t| [t[0] + base, t[2] + base, t[1] + base]));
        shell.tri_loops.extend(cavity.tri_loops.iter().map(|t| t.map(|v| v + loops)));
        shell.tri_source.extend(cavity.tri_source);
        shell.uv_layers[0].extend(&cavity.uv_layers[0]);
        for normal in [[1.0, 0.0, 1.0], [0.3, -0.7, 0.64]] {
            let plane = Plane::new([5.0; 3], normal).unwrap();
            let sides = split_plane_with_origins(&shell, plane, 1e-5);
            for (side, _) in sides {
                assert!(valid_solid(&side, 1e-5), "{normal:?}");
                assert!((volume(&side) - 468.0).abs() < 0.01);
            }
        }
        shell.tris.pop();
        assert!(!valid_solid(&shell, 1e-5));
        assert_eq!(Plane::new([0.0; 3], [0.0; 3]).unwrap_err(), CutError::InvalidPlane);
        assert_eq!(Plane::new([f32::NAN; 3], [1.0; 3]).unwrap_err(), CutError::InvalidPlane);
    }
}
