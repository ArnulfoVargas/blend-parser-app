//! Cutting a part across a plane square to one axis. Triangles that cross the
//! plane are split, their new corners interpolated in every UV and colour
//! layer, and each side's opening is closed with a flat cap, holes included,
//! so both pieces stay solid.
//!
//! The cut can also join the pieces back: connectors placed automatically on
//! each cut face, a plug standing out of one piece into a hole in the other,
//! or holes in both for separate dowel pins. They are part of the cap, so the
//! pieces stay closed, and take the cap's colour.

use std::collections::{HashMap, HashSet};

use crate::geom::{self, Vec3};
use crate::scene::Part;

/// How the two pieces of a cut fit back together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointKind {
    /// A plug on one piece and its hole in the other.
    Plug,
    /// A hole in each piece and a separate pin that goes in both.
    Dowel,
}

/// Cross-section of a connector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointShape {
    Circle,
    Square,
    Hexagon,
}

impl JointShape {
    /// Polygon sides and the angle of the first corner.
    fn sides(self) -> (usize, f32) {
        match self {
            Self::Circle => (24, 0.0),
            Self::Square => (4, std::f32::consts::FRAC_PI_4),
            Self::Hexagon => (6, 0.0),
        }
    }
}

/// Connectors for a cut, in final millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joint {
    pub kind: JointKind,
    pub shape: JointShape,
    /// Width across flats (a circle's diameter).
    pub size_mm: f32,
    /// How far a plug stands out, or how deep a dowel goes into each piece.
    pub depth_mm: f32,
    /// Gap per side between a connector and its hole; holes are this much deeper too.
    pub tolerance_mm: f32,
    /// Connectors on each separate cut face; 0 picks how many by its size.
    pub count: u8,
}

impl Default for Joint {
    fn default() -> Self {
        Self {
            kind: JointKind::Plug,
            shape: JointShape::Circle,
            size_mm: 6.0,
            depth_mm: 5.0,
            tolerance_mm: 0.15,
            count: 0,
        }
    }
}

/// Least material left round a connector or under a hole, in millimetres.
const WALL_MM: f32 = 1.2;

/// Where a cut's connectors go and how they are scaled to the part.
#[derive(Clone, Copy, Debug)]
pub struct JointAt {
    pub joint: Joint,
    /// The piece the plugs stand out of: 0 below the plane, 1 above it.
    pub plug_side: usize,
    /// Millimetres per part unit along each axis.
    pub unit: Vec3,
}

/// Both pieces of a cut, and the dowel pins that join them, standing on Z = 0.
pub struct Cut {
    pub pieces: [Part; 2],
    pub pins: Vec<Part>,
    /// Connectors that found room on the cut faces.
    pub connectors: usize,
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

/// Splits `part` where `axis` equals `value` into the piece below and the piece
/// above, each with its opening capped. Vertices within `eps` of the plane are
/// moved onto it. A side the part does not reach comes back with no triangles.
pub fn split(part: &Part, axis: usize, value: f32, eps: f32) -> [Part; 2] {
    split_joined(part, axis, value, eps, None).pieces
}

/// [`split`], with connectors on the cut faces when `joint` is given.
pub fn split_joined(part: &Part, axis: usize, value: f32, eps: f32, joint: Option<&JointAt>) -> Cut {
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
    let mut rings_extra: [Vec<Vec<u32>>; 2] = [vec![], vec![]];
    let mut tris_extra: [Vec<Tri>; 2] = [vec![], vec![]];
    let mut pins = vec![];
    let mut connectors = 0;
    if let Some(j) = joint {
        let spots = place_connectors(part, &sides[0], &positions, &on_plane, axis, value, j);
        connectors = spots.len();
        for (k, spot) in spots.iter().enumerate() {
            for side in 0..2 {
                let plug = j.joint.kind == JointKind::Plug && side == j.plug_side;
                let (ring, tris) = connector(&mut positions, axis, value, j, spot, side, plug);
                rings_extra[side].push(ring);
                tris_extra[side].extend(tris);
            }
            if j.joint.kind == JointKind::Dowel {
                pins.push(pin(part, &loops, j, spot, k));
            }
        }
    }
    let mut out = sides.map(|tris| (tris, vec![]));
    for (side, (tris, caps)) in out.iter_mut().enumerate() {
        // Below the cut the cap faces up the axis; above, down it.
        *caps = cap(tris, &positions, &on_plane, axis, side == 0, &rings_extra[side]);
        caps.append(&mut tris_extra[side]);
    }
    let [low, high] = out;
    Cut { pieces: [piece(part, &positions, &loops, low), piece(part, &positions, &loops, high)], pins, connectors }
}

/// A connector's centre on the cut face (millimetres, plane coordinates) and
/// the corner and material its faces are coloured with.
struct Spot {
    at: [f32; 2],
    corner: u32,
    src: u16,
}

/// The two in-plane axes of a cut across `axis`.
fn plane_axes(axis: usize) -> (usize, usize) {
    ((axis + 1) % 3, (axis + 2) % 3)
}

/// Corner radius of a connector's polygon whose flats are `flat` from its centre.
fn corner_radius(shape: JointShape, flat: f32) -> f32 {
    let (n, _) = shape.sides();
    flat / (std::f32::consts::PI / n as f32).cos()
}

/// Points round a connector's outline, counter-clockwise, in plane millimetres.
fn outline(shape: JointShape, centre: [f32; 2], flat: f32) -> Vec<[f32; 2]> {
    let (n, start) = shape.sides();
    let r = corner_radius(shape, flat);
    (0..n)
        .map(|i| {
            let a = start + i as f32 * std::f32::consts::TAU / n as f32;
            [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
        })
        .collect()
}

/// Material along the axis from the cut face at `at` into side `side`, in millimetres.
fn depth_into(part: &Part, axis: usize, value: f32, unit: Vec3, at: [f32; 2], side: usize) -> f32 {
    let (u, v) = plane_axes(axis);
    let mut o = [0.0; 3];
    o[axis] = value;
    o[u] = at[0] / unit[u];
    o[v] = at[1] / unit[v];
    let mut d = [0.0; 3];
    d[axis] = if side == 0 { -1.0 } else { 1.0 };
    let nearest = (0..part.tris.len())
        .filter_map(|t| geom::ray_triangle(o, d, &part.corners(t)).map(|h| h.0))
        .fold(f32::INFINITY, f32::min);
    nearest * unit[axis]
}

/// Distance from `p` to segment `a`–`b`.
fn to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
    ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

fn ring_area(pts: &[[f32; 2]]) -> f32 {
    (0..pts.len())
        .map(|k| {
            let (p, q) = (pts[k], pts[(k + 1) % pts.len()]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f32>()
        * 0.5
}

fn inside_pts(p: [f32; 2], ring: &[[f32; 2]]) -> bool {
    let mut odd = false;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            odd = !odd;
        }
    }
    odd
}

/// Where the connectors go on each separate face of the cut: as far inside as
/// the face allows, spread apart, and only where both pieces are thick enough
/// behind the face for a hole. A face too small for one gets none.
fn place_connectors(
    part: &Part,
    below: &[Tri],
    positions: &[Vec3],
    on_plane: &[bool],
    axis: usize,
    value: f32,
    j: &JointAt,
) -> Vec<Spot> {
    let (u, v) = plane_axes(axis);
    let unit = j.unit;
    let mm = |i: u32| [positions[i as usize][u] * unit[u], positions[i as usize][v] * unit[v]];
    // Below the plane the cap faces up the axis: plane coordinates as they are.
    let rings = rim_rings(below, on_plane);
    let pts: Vec<Vec<[f32; 2]>> = rings.iter().map(|r| r.0.iter().map(|&i| mm(i)).collect()).collect();
    let areas: Vec<f32> = pts.iter().map(|p| ring_area(p)).collect();
    let jt = j.joint;
    let size = jt.size_mm.max(0.1);
    // Room the widest connector (a hole) needs round its centre.
    let reach = corner_radius(jt.shape, size / 2.0 + jt.tolerance_mm.max(0.0)) + WALL_MM;
    let need = jt.depth_mm.max(0.0) + jt.tolerance_mm.max(0.0) + WALL_MM;
    let mut out = vec![];
    for (o, outer) in pts.iter().enumerate() {
        if areas[o] <= 0.0 {
            continue;
        }
        // Its holes: clockwise rings whose smallest surrounding outline is this one.
        let holes: Vec<&Vec<[f32; 2]>> = (0..pts.len())
            .filter(|&h| {
                areas[h] < 0.0 && {
                    let p = pts[h][0];
                    (0..pts.len())
                        .filter(|&k| areas[k] > 0.0 && inside_pts(p, &pts[k]))
                        .min_by(|&a, &b| areas[a].total_cmp(&areas[b]))
                        == Some(o)
                }
            })
            .map(|h| &pts[h])
            .collect();
        let face = areas[o] + holes.iter().map(|h| ring_area(h)).sum::<f32>();
        let wanted = match jt.count {
            0 if face < 40.0 * size * size => 1,
            0 if face < 160.0 * size * size => 2,
            0 => 3,
            n => n as usize,
        };
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in outer {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let step = (reach * 0.5).max(((hi[0] - lo[0]) * (hi[1] - lo[1])).sqrt() / 60.0).max(1e-3);
        let edges = || {
            std::iter::once(outer)
                .chain(holes.iter().copied())
                .flat_map(|r| (0..r.len()).map(|k| (r[k], r[(k + 1) % r.len()])))
        };
        // Grid points inside the face with their clearance from its border.
        let mut cands: Vec<([f32; 2], f32)> = vec![];
        let mut y = lo[1] + step * 0.5;
        while y < hi[1] {
            let mut x = lo[0] + step * 0.5;
            while x < hi[0] {
                let p = [x, y];
                if inside_pts(p, outer) && !holes.iter().any(|h| inside_pts(p, h)) {
                    let clear = edges().map(|(a, b)| to_segment(p, a, b)).fold(f32::MAX, f32::min);
                    if clear >= reach {
                        cands.push((p, clear));
                    }
                }
                x += step;
            }
            y += step;
        }
        let deep = |p: [f32; 2]| {
            let sides: &[usize] = if jt.kind == JointKind::Plug { &[1 - j.plug_side] } else { &[0, 1] };
            sides.iter().all(|&s| depth_into(part, axis, value, unit, p, s) >= need)
        };
        let mut chosen: Vec<[f32; 2]> = vec![];
        let mut rejected = vec![false; cands.len()];
        while chosen.len() < wanted {
            // The deepest point first, then the one farthest from those taken.
            let score = |&(p, clear): &([f32; 2], f32)| -> f32 {
                let apart = chosen
                    .iter()
                    .map(|c| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt())
                    .fold(f32::MAX, f32::min);
                if chosen.is_empty() {
                    clear
                } else if apart < 2.0 * reach {
                    f32::MIN
                } else {
                    apart + clear
                }
            };
            let best = (0..cands.len())
                .filter(|&i| !rejected[i])
                .map(|i| (i, score(&cands[i])))
                .filter(|&(_, s)| s > f32::MIN)
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let Some((i, _)) = best else { break };
            rejected[i] = true;
            if deep(cands[i].0) {
                chosen.push(cands[i].0);
            }
        }
        let (_, corner, src) = rings[o];
        out.extend(chosen.into_iter().map(|at| Spot { at, corner, src }));
    }
    out
}

/// Flips each triangle whose normal points away from `want(centroid)`.
fn facing(positions: &[Vec3], tris: &mut [Tri], want: impl Fn(Vec3) -> Vec3) {
    for t in tris.iter_mut() {
        let c = t.0.map(|i| positions[i as usize]);
        let n = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
        let mid = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
        if geom::dot(n, want(mid)) < 0.0 {
            t.0.swap(1, 2);
            t.1.swap(1, 2);
        }
    }
}

/// One side's connector: its outline on the plane (vertex indices, to open in
/// the cap) and the walls and end that close it, a plug standing out of the
/// face or a hole into it.
fn connector(
    positions: &mut Vec<Vec3>,
    axis: usize,
    value: f32,
    j: &JointAt,
    spot: &Spot,
    side: usize,
    plug: bool,
) -> (Vec<u32>, Vec<Tri>) {
    let (u, v) = plane_axes(axis);
    let jt = j.joint;
    let tol = jt.tolerance_mm.max(0.0);
    let (flat, length) =
        if plug { (jt.size_mm / 2.0, jt.depth_mm) } else { (jt.size_mm / 2.0 + tol, jt.depth_mm + tol) };
    // Into this side's material, or out of it for a plug.
    let into = if side == 0 { -1.0 } else { 1.0 };
    let dir = if plug { -into } else { into };
    let world = |p: [f32; 2], h: f32| {
        let mut w = [0.0; 3];
        w[axis] = value + dir * h / j.unit[axis];
        w[u] = p[0] / j.unit[u];
        w[v] = p[1] / j.unit[v];
        w
    };
    let pts = outline(jt.shape, spot.at, flat);
    let n = pts.len();
    let base = positions.len() as u32;
    positions.extend(pts.iter().map(|&p| world(p, 0.0)));
    positions.extend(pts.iter().map(|&p| world(p, length)));
    let ring: Vec<u32> = (0..n as u32).map(|k| base + k).collect();
    let c = [spot.corner; 3];
    let mut walls = vec![];
    for k in 0..n as u32 {
        let k1 = (k + 1) % n as u32;
        let (p0, p1, q0, q1) = (base + k, base + k1, base + n as u32 + k, base + n as u32 + k1);
        walls.push(([p0, p1, q1], c, spot.src));
        walls.push(([p0, q1, q0], c, spot.src));
    }
    let axis_at = world(spot.at, 0.0);
    // A plug's walls face out from its middle, a hole's into it.
    let sign = if plug { 1.0 } else { -1.0 };
    facing(positions, &mut walls, |m| {
        let mut r = geom::sub(m, axis_at);
        r[axis] = 0.0;
        geom::scale(r, sign)
    });
    let mut end: Vec<Tri> = (1..n as u32 - 1)
        .map(|k| ([base + n as u32, base + n as u32 + k, base + n as u32 + k + 1], c, spot.src))
        .collect();
    // A plug's end faces on along it; a hole's bottom back at the face.
    let mut out = [0.0; 3];
    out[axis] = dir * sign;
    facing(positions, &mut end, |_| out);
    walls.append(&mut end);
    (ring, walls)
}

/// A dowel pin standing on Z = 0 at the origin: as wide as the connector and
/// long enough to reach the bottom of both holes but their tolerance.
fn pin(part: &Part, loops: &Loops, j: &JointAt, spot: &Spot, k: usize) -> Part {
    let jt = j.joint;
    let pts = outline(jt.shape, [0.0, 0.0], jt.size_mm / 2.0);
    let n = pts.len() as u32;
    let length = 2.0 * jt.depth_mm;
    let at = |p: [f32; 2], z: f32| [p[0] / j.unit[0], p[1] / j.unit[1], z / j.unit[2]];
    let mut positions: Vec<Vec3> = pts.iter().map(|&p| at(p, 0.0)).collect();
    positions.extend(pts.iter().map(|&p| at(p, length)));
    let mut tris: Vec<Tri> = vec![];
    for i in 0..n {
        let i1 = (i + 1) % n;
        tris.push(([i, i1, n + i1], [0; 3], 0));
        tris.push(([i, n + i1, n + i], [0; 3], 0));
    }
    let centre = [0.0, 0.0, length / 2.0 / j.unit[2]];
    facing(&positions, &mut tris, |m| {
        let mut r = geom::sub(m, centre);
        r[2] = 0.0;
        r
    });
    let mut ends: Vec<Tri> =
        (1..n - 1).flat_map(|i| [([0, i, i + 1], [0; 3], 0), ([n, n + i, n + i + 1], [0; 3], 0)]).collect();
    facing(&positions, &mut ends, |m| [0.0, 0.0, m[2] - centre[2]]);
    tris.append(&mut ends);
    let corner = spot.corner as usize;
    Part {
        name: format!("{} · {}", part.name, k + 1),
        positions,
        tri_loops: tris.iter().map(|t| t.1).collect(),
        tri_source: vec![0; tris.len()],
        tris: tris.iter().map(|t| t.0).collect(),
        sources: part.sources.get(spot.src as usize).cloned().into_iter().collect(),
        uv_layers: loops.uv.iter().map(|l| vec![l.get(corner).copied().unwrap_or_default()]).collect(),
        color_layers: loops.color.iter().map(|l| vec![l.get(corner).copied().unwrap_or_default()]).collect(),
        object: part.object,
    }
}

/// The closed rims of one side's opening on the plane, each with the corner and
/// material of the surface where it starts. Seen with the cap's normal coming
/// out of the screen, outlines run counter-clockwise and holes clockwise.
fn rim_rings(tris: &[Tri], on_plane: &[bool]) -> Vec<(Vec<u32>, u32, u16)> {
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
/// (connector outlines on the plane) left open in it.
fn cap(
    tris: &[Tri],
    positions: &[Vec3],
    on_plane: &[bool],
    axis: usize,
    faces_up: bool,
    extra: &[Vec<u32>],
) -> Vec<Tri> {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let (u, v) = if faces_up { (u, v) } else { (v, u) };
    let at = |i: u32| [positions[i as usize][u], positions[i as usize][v]];
    let rings = rim_rings(tris, on_plane);
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
    let mut holes: Vec<Vec<Vec<u32>>> = vec![vec![]; rings.len()];
    let owner = |p: [f32; 2]| {
        (0..rings.len())
            .filter(|&o| areas[o] > 0.0 && inside(p, &rings[o].0, &at))
            .min_by(|&a, &b| areas[a].total_cmp(&areas[b]))
    };
    for (h, ring) in rings.iter().enumerate() {
        if areas[h] >= 0.0 {
            continue;
        }
        if let Some(o) = owner(at(ring.0[0])) {
            holes[o].push(ring.0.clone());
        }
    }
    for ring in extra {
        // A hole runs clockwise.
        let mut ring = ring.clone();
        if area(&ring) > 0.0 {
            ring.reverse();
        }
        if let Some(o) = owner(at(ring[0])) {
            holes[o].push(ring);
        }
    }
    let mut out = vec![];
    for (o, (ring, corner, src)) in rings.iter().enumerate() {
        if areas[o] <= 0.0 {
            continue;
        }
        let mut outline = ring.clone();
        let mut own: Vec<&Vec<u32>> = holes[o].iter().collect();
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

    fn joint(kind: JointKind, shape: JointShape, count: u8) -> JointAt {
        JointAt {
            joint: Joint { kind, shape, size_mm: 4.0, depth_mm: 3.0, tolerance_mm: 0.2, count },
            plug_side: 0,
            unit: [1.0; 3],
        }
    }

    #[test]
    fn a_plug_stands_out_of_one_piece_into_a_hole_in_the_other() {
        let j = joint(JointKind::Plug, JointShape::Square, 1);
        let cut = split_joined(&cube([0.0; 3], [20.0, 20.0, 20.0]), 2, 10.0, 1e-5, Some(&j));
        let [low, high] = &cut.pieces;
        assert_eq!(cut.connectors, 1);
        assert!(cut.pins.is_empty());
        assert!(closed(low) && closed(high));
        // A 4 mm square plug 3 mm tall; its hole 4.4 mm wide and 3.2 mm deep.
        let half = 20.0 * 20.0 * 10.0;
        assert!((volume(low) - (half + 48.0)).abs() < 1e-2, "{}", volume(low));
        assert!((volume(high) - (half - 4.4 * 4.4 * 3.2)).abs() < 1e-2, "{}", volume(high));
        // The plug stands above the plane, centred on the face.
        let top = low.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        assert!((top - 13.0).abs() < 1e-4);
    }

    #[test]
    fn dowels_leave_holes_in_both_pieces_and_come_as_pins() {
        for shape in [JointShape::Circle, JointShape::Hexagon] {
            let j = joint(JointKind::Dowel, shape, 2);
            let cut = split_joined(&cube([0.0; 3], [40.0, 20.0, 20.0]), 0, 20.0, 1e-5, Some(&j));
            assert_eq!(cut.connectors, 2);
            assert_eq!(cut.pins.len(), 2);
            let [low, high] = &cut.pieces;
            assert!(closed(low) && closed(high));
            let half = 20.0 * 20.0 * 20.0;
            assert!(volume(low) < half - 1.0 && (volume(low) - volume(high)).abs() < 1e-2);
            for pin in &cut.pins {
                assert!(closed(pin) && volume(pin) > 0.0);
                let h = pin.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
                assert!((h - 6.0).abs() < 1e-4);
                assert!(pin.tri_loops.iter().flatten().all(|&l| (l as usize) < pin.uv_layers[0].len()));
            }
        }
    }

    #[test]
    fn auto_places_more_connectors_on_a_bigger_face_and_none_where_it_is_too_thin() {
        let j = joint(JointKind::Plug, JointShape::Circle, 0);
        let small = split_joined(&cube([0.0; 3], [10.0, 10.0, 20.0]), 2, 10.0, 1e-5, Some(&j));
        let big = split_joined(&cube([0.0; 3], [100.0, 30.0, 20.0]), 2, 10.0, 1e-5, Some(&j));
        assert_eq!(small.connectors, 1);
        assert!(big.connectors >= 2);
        assert!(big.pieces.iter().all(closed));
        // 2 mm above the cut leaves no room for a 3 mm deep hole.
        let thin = split_joined(&cube([0.0; 3], [20.0, 20.0, 12.0]), 2, 10.0, 1e-5, Some(&j));
        assert_eq!(thin.connectors, 0);
        assert!(thin.pieces.iter().all(closed));
    }

    #[test]
    fn connectors_skip_a_cavity_and_keep_the_pieces_closed() {
        let outer = cube([0.0; 3], [30.0, 30.0, 30.0]);
        let mut inner = cube([10.0; 3], [20.0; 3]);
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
        let j = joint(JointKind::Plug, JointShape::Circle, 2);
        let cut = split_joined(&both, 2, 15.0, 1e-5, Some(&j));
        assert_eq!(cut.connectors, 2);
        assert!(cut.pieces.iter().all(closed));
    }

    /// A closed UV sphere of radius `r`, faces outwards.
    fn sphere(r: f32, rings: u32, segs: u32) -> Part {
        let mut positions = vec![[0.0, 0.0, -r]];
        for i in 1..rings {
            let a = std::f32::consts::PI * i as f32 / rings as f32 - std::f32::consts::FRAC_PI_2;
            for j in 0..segs {
                let b = std::f32::consts::TAU * j as f32 / segs as f32;
                positions.push([r * a.cos() * b.cos(), r * a.cos() * b.sin(), r * a.sin()]);
            }
        }
        positions.push([0.0, 0.0, r]);
        let top = positions.len() as u32 - 1;
        let at = |i: u32, j: u32| 1 + (i - 1) * segs + j % segs;
        let mut tris = vec![];
        for j in 0..segs {
            tris.push([0, at(1, j + 1), at(1, j)]);
            tris.push([top, at(rings - 1, j), at(rings - 1, j + 1)]);
            for i in 1..rings - 1 {
                tris.push([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
                tris.push([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
            }
        }
        let n = tris.len() as u32;
        Part {
            name: "s".into(),
            positions,
            tri_loops: (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: vec![0; tris.len()],
            tris,
            sources: vec![ColorSource::Flat([0.5, 0.2, 0.1])],
            ..Default::default()
        }
    }

    #[test]
    fn a_round_face_takes_connectors_and_stays_closed() {
        let s = sphere(25.0, 24, 48);
        for (axis, kind) in [(0, JointKind::Plug), (2, JointKind::Dowel)] {
            let j = joint(kind, JointShape::Circle, 0);
            let cut = split_joined(&s, axis, 3.0, 1e-5, Some(&j));
            assert!(cut.connectors >= 2, "{}", cut.connectors);
            assert!(cut.pieces.iter().all(closed));
            let whole = volume(&s);
            let both = volume(&cut.pieces[0]) + volume(&cut.pieces[1]);
            assert!(both < whole && both > whole * 0.98, "{both} vs {whole}");
        }
    }
}
