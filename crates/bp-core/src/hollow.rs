//! Hollowing: an inner wall `wall` away from the surface everywhere, so the
//! model prints as a shell, and optional drain holes through the base.
//!
//! The inner wall is the level set of the distance to the outside, on a voxel
//! grid: inside each voxel node is found by counting crossings up its column,
//! the distance by an exact Euclidean transform, and the wall by marching
//! tetrahedra. That surface is closed and cannot cross itself or the outside
//! (it is at least `wall` minus a voxel from it), and it faces into the cavity,
//! so the model and its cavity are one valid solid. Drains replace the faces
//! over a disc of the base and of the cavity floor with a ring of triangles and
//! join the two with a tube, so the shell stays closed.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::bvh::{Bvh, TriSoup};
use crate::geom::{self, Vec3};

/// Voxels along the model's longest side, at most.
pub const MAX_CELLS: usize = 256;
/// Points around a drain's circle.
const DRAIN_SIDES: usize = 24;

/// A closed, indexed surface: the inner wall before it joins the model.
#[derive(Clone, Debug, Default)]
pub struct Shell {
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
}

/// Why a model was not hollowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Some edge has one face, or more than two: inside and outside are not defined.
    Open,
    /// The wall leaves no room inside.
    NoRoom,
}

/// The inner wall of a closed surface, `wall` in from it, on voxels of `cell`.
/// `tris` is the whole closed surface (every part); units are the mesh's.
pub fn inner_wall(tris: &[[Vec3; 3]], wall: f32, cell: f32) -> Result<Shell, Refusal> {
    let Some(grid) = Grid::new(tris, cell) else { return Err(Refusal::NoRoom) };
    let inside = grid.inside(tris);
    let dist = grid.distance(&inside);
    // Nodes sample the solid: the surface lies about half a cell from the last inside node.
    let level = wall / cell + 0.5;
    let phi: Vec<f32> = dist
        .par_iter()
        .map(|&d2| {
            let v = d2.sqrt() - level;
            // Never exactly on the level, so no wall vertex sits on a node.
            if v.abs() < 1e-4 {
                -1e-4
            } else {
                v
            }
        })
        .collect();
    let shell = grid.march(&phi);
    if shell.tris.is_empty() {
        return Err(Refusal::NoRoom);
    }
    Ok(shell)
}

/// Fewer triangles for the same wall: flat stretches merge, and nothing moves
/// further than `error` (a fraction of a voxel, far less than the wall). The
/// shell stays closed.
pub fn lighten(shell: Shell, error: f32) -> Shell {
    // Pushed hard, the simplifier can fold a thin stretch into a repeated face:
    // each try is checked, with less error each time, else the wall stays as it is.
    // Regular triangles merge less but have kept the wall sealed where plain ones did not.
    use meshopt::SimplifyOptions as O;
    for (k, options) in [(1.0, O::ErrorAbsolute), (0.5, O::ErrorAbsolute), (1.0, O::ErrorAbsolute | O::Regularize)] {
        if let Some(light) = lighten_once(&shell, error * k, options) {
            return light;
        }
    }
    shell
}

fn lighten_once(shell: &Shell, error: f32, options: meshopt::SimplifyOptions) -> Option<Shell> {
    let indices: Vec<u32> = shell.tris.iter().flatten().copied().collect();
    let adapter =
        meshopt::VertexDataAdapter::new(meshopt::typed_to_bytes(&shell.positions), std::mem::size_of::<Vec3>(), 0)
            .ok()?;
    let out = meshopt::simplify(&indices, &adapter, 0, error, options, None);
    if out.len() < 12 {
        return None;
    }
    let mut remap = vec![u32::MAX; shell.positions.len()];
    let mut positions = vec![];
    let tris = out
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            t.map(|v| {
                if remap[v as usize] == u32::MAX {
                    remap[v as usize] = positions.len() as u32;
                    positions.push(shell.positions[v as usize]);
                }
                remap[v as usize]
            })
        })
        .collect::<Vec<[u32; 3]>>();
    sealed(&tris).then_some(Shell { positions, tris })
}

/// Every edge run once each way, and no face twice: closed and consistently wound.
pub fn sealed(tris: &[[u32; 3]]) -> bool {
    let mut edges: Vec<(u32, u32)> = tris.iter().flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3]))).collect();
    edges.sort_unstable();
    if edges.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    edges.iter().all(|&(a, b)| edges.binary_search(&(b, a)).is_ok())
}

/// The cell size for a model of `longest` side and a wall of `wall`: a third
/// of the wall, unless that passes [`MAX_CELLS`] along the longest side.
pub fn cell_for(longest: f32, wall: f32) -> f32 {
    (wall / 3.0).max(longest / MAX_CELLS as f32)
}

struct Grid {
    origin: Vec3,
    cell: f32,
    n: [usize; 3],
}

impl Grid {
    fn new(tris: &[[Vec3; 3]], cell: f32) -> Option<Self> {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in tris.iter().flatten() {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if cell.is_nan() || cell <= 0.0 || lo[0] > hi[0] {
            return None;
        }
        // Two empty cells around the model, so every node on the border is outside.
        let pad = 2.0 * cell;
        let origin = std::array::from_fn(|k| lo[k] - pad);
        let n = std::array::from_fn(|k| ((hi[k] - lo[k] + 2.0 * pad) / cell).ceil() as usize + 1);
        Some(Self { origin, cell, n })
    }

    #[inline]
    fn index(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.n[0] * (j + self.n[1] * k)
    }

    #[inline]
    fn node(&self, i: usize, j: usize, k: usize) -> Vec3 {
        [
            self.origin[0] + i as f32 * self.cell,
            self.origin[1] + j as f32 * self.cell,
            self.origin[2] + k as f32 * self.cell,
        ]
    }

    /// Which nodes are inside: up each column, the faces crossed below a node,
    /// +1 going in (facing down) and −1 going out, add up to more than zero.
    fn inside(&self, tris: &[[Vec3; 3]]) -> Vec<bool> {
        let [nx, ny, nz] = self.n;
        // Columns a hair off the node grid, so a ray never runs along an edge or through a vertex.
        let jitter = [self.cell * 1.234_567e-3, self.cell * 2.718_281e-3];
        let col = |i: usize, j: usize| {
            [self.origin[0] + i as f32 * self.cell + jitter[0], self.origin[1] + j as f32 * self.cell + jitter[1]]
        };
        let mut hits: Vec<Vec<(f32, i8)>> = vec![vec![]; nx * ny];
        for t in tris {
            let [a, b, c] = *t;
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if area == 0.0 {
                continue;
            }
            let sign: i8 = if area > 0.0 { -1 } else { 1 };
            let (x0, x1) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
            let (y0, y1) = (a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
            let i0 = (((x0 - self.origin[0]) / self.cell).floor().max(0.0)) as usize;
            let i1 = (((x1 - self.origin[0]) / self.cell).ceil() as usize).min(nx - 1);
            let j0 = (((y0 - self.origin[1]) / self.cell).floor().max(0.0)) as usize;
            let j1 = (((y1 - self.origin[1]) / self.cell).ceil() as usize).min(ny - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let [x, y] = col(i, j);
                    // Barycentrics in the XY projection.
                    let w0 = ((b[0] - x) * (c[1] - y) - (c[0] - x) * (b[1] - y)) / area;
                    let w1 = ((c[0] - x) * (a[1] - y) - (a[0] - x) * (c[1] - y)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    hits[i + nx * j].push((w0 * a[2] + w1 * b[2] + w2 * c[2], sign));
                }
            }
        }
        let columns: Vec<Vec<bool>> = hits
            .into_par_iter()
            .enumerate()
            .map(|(_, mut h)| {
                h.sort_by(|p, q| p.0.total_cmp(&q.0));
                let mut out = vec![false; nz];
                let (mut next, mut winding) = (0, 0i32);
                for (k, slot) in out.iter_mut().enumerate() {
                    let z = self.origin[2] + k as f32 * self.cell;
                    while next < h.len() && h[next].0 < z {
                        winding += h[next].1 as i32;
                        next += 1;
                    }
                    *slot = winding > 0;
                }
                out
            })
            .collect();
        let mut inside = vec![false; nx * ny * nz];
        for (c, column) in columns.into_iter().enumerate() {
            let (i, j) = (c % nx, c / nx);
            for (k, v) in column.into_iter().enumerate() {
                inside[self.index(i, j, k)] = v;
            }
        }
        inside
    }

    /// Squared distance, in cells, from each inside node to the nearest outside one (0 outside).
    fn distance(&self, inside: &[bool]) -> Vec<f32> {
        let [nx, ny, nz] = self.n;
        const FAR: f32 = 1e20;
        let mut f: Vec<f32> = inside.par_iter().map(|&x| if x { FAR } else { 0.0 }).collect();
        // Along X: rows are contiguous.
        f.par_chunks_mut(nx).for_each(edt_line);
        // Along Y, one Z slice at a time.
        f.par_chunks_mut(nx * ny).for_each(|slice| {
            let mut line = vec![0.0; ny];
            for i in 0..nx {
                for (j, v) in line.iter_mut().enumerate() {
                    *v = slice[i + nx * j];
                }
                edt_line(&mut line);
                for (j, v) in line.iter().enumerate() {
                    slice[i + nx * j] = *v;
                }
            }
        });
        // Along Z: each column worked out apart, then written back.
        let columns: Vec<Vec<f32>> = (0..nx * ny)
            .into_par_iter()
            .map(|c| {
                let mut line: Vec<f32> = (0..nz).map(|k| f[c + nx * ny * k]).collect();
                edt_line(&mut line);
                line
            })
            .collect();
        for (c, line) in columns.into_iter().enumerate() {
            for (k, v) in line.into_iter().enumerate() {
                f[c + nx * ny * k] = v;
            }
        }
        f
    }

    /// Marching tetrahedra on `phi` > 0 (the cavity). Each cell is cut into six
    /// tetrahedra around its main diagonal, the same in every cell, so the
    /// pieces meet edge to edge and the surface is closed. Faces point into the cavity.
    fn march(&self, phi: &[f32]) -> Shell {
        const TETS: [[usize; 4]; 6] =
            [[0, 1, 3, 7], [0, 2, 3, 7], [0, 2, 6, 7], [0, 4, 6, 7], [0, 4, 5, 7], [0, 1, 5, 7]];
        let [nx, ny, nz] = self.n;
        let mut shell = Shell::default();
        let mut on_edge: HashMap<(u32, u32), u32> = HashMap::new();
        for k in 0..nz - 1 {
            for j in 0..ny - 1 {
                for i in 0..nx - 1 {
                    let corner = |c: usize| self.index(i + (c & 1), j + ((c >> 1) & 1), k + ((c >> 2) & 1));
                    let ids: [usize; 8] = std::array::from_fn(corner);
                    let first = phi[ids[0]] > 0.0;
                    if ids.iter().all(|&v| (phi[v] > 0.0) == first) {
                        continue;
                    }
                    let pos: [Vec3; 8] =
                        std::array::from_fn(|c| self.node(i + (c & 1), j + ((c >> 1) & 1), k + ((c >> 2) & 1)));
                    for tet in TETS {
                        let (inn, out): (Vec<usize>, Vec<usize>) = tet.iter().partition(|&&c| phi[ids[c]] > 0.0);
                        if inn.is_empty() || out.is_empty() {
                            continue;
                        }
                        let mut point = |a: usize, b: usize| -> u32 {
                            let (va, vb) = (ids[a] as u32, ids[b] as u32);
                            let key = (va.min(vb), va.max(vb));
                            *on_edge.entry(key).or_insert_with(|| {
                                let (fa, fb) = (phi[ids[a]], phi[ids[b]]);
                                let t = (fa / (fa - fb)).clamp(0.01, 0.99);
                                shell.positions.push(geom::add(pos[a], geom::scale(geom::sub(pos[b], pos[a]), t)));
                                shell.positions.len() as u32 - 1
                            })
                        };
                        let toward =
                            geom::sub(centroid(inn.iter().map(|&c| pos[c])), centroid(out.iter().map(|&c| pos[c])));
                        let faces: Vec<[u32; 3]> = match (inn.len(), out.len()) {
                            (1, 3) => vec![[point(inn[0], out[0]), point(inn[0], out[1]), point(inn[0], out[2])]],
                            (3, 1) => vec![[point(out[0], inn[0]), point(out[0], inn[1]), point(out[0], inn[2])]],
                            _ => {
                                let (a, b, c, d) = (inn[0], inn[1], out[0], out[1]);
                                let q = [point(a, c), point(a, d), point(b, d), point(b, c)];
                                vec![[q[0], q[1], q[2]], [q[0], q[2], q[3]]]
                            }
                        };
                        for mut f in faces {
                            let [p0, p1, p2] = f.map(|v| shell.positions[v as usize]);
                            if geom::dot(geom::cross(geom::sub(p1, p0), geom::sub(p2, p0)), toward) < 0.0 {
                                f.swap(1, 2);
                            }
                            shell.tris.push(f);
                        }
                    }
                }
            }
        }
        shell
    }
}

fn centroid(points: impl Iterator<Item = Vec3>) -> Vec3 {
    let (s, n) = points.fold(([0.0; 3], 0.0), |(s, n), p| (geom::add(s, p), n + 1.0));
    geom::scale(s, 1.0 / n)
}

/// Felzenszwalb's 1D squared distance transform, in place.
fn edt_line(f: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let src = f.to_vec();
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f32; n + 1];
    let mut k = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    let inter =
        |q: usize, p: usize| ((src[q] + (q * q) as f32) - (src[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
    for q in 1..n {
        let mut s = inter(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = inter(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in f.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - v[k] as f32;
        *out = d * d + src[v[k]];
    }
}

// ---- drains --------------------------------------------------------------------

/// A surface the drill works on: indexed, with which faces it may replace.
pub struct Sheet<'a> {
    pub positions: &'a mut Vec<Vec3>,
    pub tris: &'a [[u32; 3]],
    /// Faces already given up (by an earlier drain).
    pub removed: &'a mut [bool],
}

/// What a drain adds to one sheet: faces over its own and its new vertices.
pub struct Ring {
    pub tris: Vec<[u32; 3]>,
    /// The circle's vertices, in order around it.
    pub circle: Vec<u32>,
}

/// Replaces the faces of `sheet` over the disc of radius `r` around `c` (in XY)
/// with a ring from their border to a circle of radius `r`, lying on the sheet.
/// `up` says which way the replaced faces must face (+1 up, −1 down); `start`
/// is the face the vertical line through `c` meets. None when the faces there
/// do not form one patch that faces that way.
pub fn ring(
    sheet: &mut Sheet,
    c: [f32; 2],
    r: f32,
    up: f32,
    start: usize,
    height: &dyn Fn([f32; 2]) -> Option<f32>,
) -> Option<Ring> {
    let tris = sheet.tris;
    let pos = |v: u32| sheet.positions[v as usize];
    let normal_z = |t: usize| {
        let [a, b, d] = tris[t].map(|v| sheet.positions[v as usize]);
        geom::normalize(geom::cross(geom::sub(b, a), geom::sub(d, a)))[2]
    };
    let reach = r * 1.25;
    let near = |t: usize| {
        let p: [[f32; 2]; 3] = tris[t].map(|v| [pos(v)[0], pos(v)[1]]);
        dist_to_triangle_2d(c, &p) <= reach
    };
    if sheet.removed[start] || normal_z(start) * up <= 0.0 {
        return None;
    }
    // The patch: faces reached through shared edges that come near the disc.
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, f) in tris.iter().enumerate() {
        if sheet.removed[t] {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (f[k], f[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut patch = vec![start];
    let mut seen = std::collections::HashSet::from([start]);
    let mut i = 0;
    while i < patch.len() {
        let t = patch[i];
        i += 1;
        for k in 0..3 {
            let (a, b) = (tris[t][k], tris[t][(k + 1) % 3]);
            for &o in &by_edge[&(a.min(b), a.max(b))] {
                // Only faces turned the right way: a wall there would bring the border inside the circle.
                if !seen.contains(&o) && near(o) && normal_z(o) * up > 0.2 {
                    seen.insert(o);
                    patch.push(o);
                }
            }
        }
    }
    // Its border, as one loop.
    let mut count: HashMap<(u32, u32), usize> = HashMap::new();
    let mut directed = vec![];
    for &t in &patch {
        for k in 0..3 {
            let (a, b) = (tris[t][k], tris[t][(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
            directed.push((a, b));
        }
    }
    let border: Vec<(u32, u32)> = directed.into_iter().filter(|&(a, b)| count[&(a.min(b), a.max(b))] == 1).collect();
    let next: HashMap<u32, u32> = border.iter().copied().collect();
    if next.len() != border.len() || border.is_empty() {
        return None;
    }
    let mut lp = vec![border[0].0];
    while lp.len() <= border.len() {
        let n = next[lp.last()?];
        if n == lp[0] {
            break;
        }
        lp.push(n);
    }
    if lp.len() != border.len() {
        return None;
    }
    // Every border vertex clear of the circle, or the ring would cross it.
    if lp.iter().any(|&v| ((pos(v)[0] - c[0]).hypot(pos(v)[1] - c[1])) <= r * 1.02) {
        return None;
    }
    let mut outer2: Vec<[f32; 2]> = lp.iter().map(|&v| [pos(v)[0], pos(v)[1]]).collect();
    // Seen from above, the border has to be a simple loop around the disc.
    if !simple_around(&outer2, c) {
        return None;
    }
    let mut outer_ids = lp.clone();
    if signed_area(&outer2) < 0.0 {
        outer2.reverse();
        outer_ids.reverse();
    }
    // The circle, clockwise so it reads as a hole; each point on the sheet.
    let mut hole2 = vec![];
    let mut circle = vec![];
    for s in 0..DRAIN_SIDES {
        let a = -(s as f32) / DRAIN_SIDES as f32 * std::f32::consts::TAU;
        let q = [c[0] + r * a.cos(), c[1] + r * a.sin()];
        let z = height(q)?;
        circle.push(sheet.positions.len() as u32);
        sheet.positions.push([q[0], q[1], z]);
        hole2.push(q);
    }
    let ids: Vec<u32> = outer_ids.iter().chain(&circle).copied().collect();
    let flat = triangulate_ring(&outer2, &hole2)?;
    let mut out = Vec::with_capacity(flat.len());
    // Ear clipping gives every face counter-clockwise seen from above, flat ones
    // too: a sheet facing down takes them all the other way round.
    for [a, b, d] in flat {
        let mut f = [ids[a], ids[b], ids[d]];
        if up < 0.0 {
            f.swap(1, 2);
        }
        out.push(f);
    }
    for t in patch {
        sheet.removed[t] = true;
    }
    // Clockwise seen from above; the tube follows it.
    Some(Ring { tris: out, circle })
}

/// Distance in the plane from `p` to a triangle (0 inside it).
fn dist_to_triangle_2d(p: [f32; 2], t: &[[f32; 2]; 3]) -> f32 {
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let s = [cross(t[0], t[1], p), cross(t[1], t[2], p), cross(t[2], t[0], p)];
    if s.iter().all(|&x| x >= 0.0) || s.iter().all(|&x| x <= 0.0) {
        return 0.0;
    }
    (0..3)
        .map(|k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let l = ab[0] * ab[0] + ab[1] * ab[1];
            let u = if l > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l).clamp(0.0, 1.0) } else { 0.0 };
            (p[0] - a[0] - u * ab[0]).hypot(p[1] - a[1] - u * ab[1])
        })
        .fold(f32::MAX, f32::min)
}

/// Whether the loop `p` never crosses itself and winds once around `c`.
fn simple_around(p: &[[f32; 2]], c: [f32; 2]) -> bool {
    let n = p.len();
    let o = |a: [f32; 2], b: [f32; 2], r: [f32; 2]| (b[0] - a[0]) * (r[1] - a[1]) - (b[1] - a[1]) * (r[0] - a[0]);
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (a, b, x, y) = (p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]);
            if o(a, b, x) * o(a, b, y) < 0.0 && o(x, y, a) * o(x, y, b) < 0.0 {
                return false;
            }
        }
    }
    // Winding number of the loop about the centre.
    let mut angle = 0.0f32;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        let (ua, ub) = ((a[1] - c[1]).atan2(a[0] - c[0]), (b[1] - c[1]).atan2(b[0] - c[0]));
        let mut d = ub - ua;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        angle += d;
    }
    (angle.abs() - std::f32::consts::TAU).abs() < 0.1
}

fn signed_area(p: &[[f32; 2]]) -> f32 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        * 0.5
}

/// Triangulates the region between `outer` (counter-clockwise) and `hole`
/// (clockwise, inside it). Indices run over `outer` then `hole`.
pub fn triangulate_ring(outer: &[[f32; 2]], hole: &[[f32; 2]]) -> Option<Vec<[usize; 3]>> {
    let n = outer.len();
    let pts: Vec<[f32; 2]> = outer.iter().chain(hole).copied().collect();
    let crosses = |a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]| {
        let o = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    };
    let edges: Vec<(usize, usize)> =
        (0..n).map(|i| (i, (i + 1) % n)).chain((0..hole.len()).map(|i| (n + i, n + (i + 1) % hole.len()))).collect();
    // A bridge from the hole's rightmost point to the closest outer point that sees it.
    let h = (0..hole.len()).max_by(|&a, &b| hole[a][0].total_cmp(&hole[b][0]))? + n;
    let mut order: Vec<usize> = (0..n).collect();
    let d2 = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
    order.sort_by(|&a, &b| d2(pts[a], pts[h]).total_cmp(&d2(pts[b], pts[h])));
    let o = *order.iter().find(|&&o| {
        edges.iter().all(|&(a, b)| a == o || b == o || a == h || b == h || !crosses(pts[o], pts[h], pts[a], pts[b]))
    })?;
    // One polygon: outer up to the bridge, round the hole, and back.
    let mut poly: Vec<usize> = (0..=o).collect();
    for k in 0..=hole.len() {
        poly.push(n + (h - n + k) % hole.len());
    }
    poly.extend(o..n);
    // Ear clipping.
    let mut out = vec![];
    let cross = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let mut guard = 0;
    while poly.len() > 3 {
        guard += 1;
        if guard > 10 * pts.len() * pts.len() + 100 {
            return None;
        }
        let m = poly.len();
        let mut clipped = false;
        // Proper ears first; when only flat ones are left (points in a line), those,
        // so no gap is left: the sliver pass flips them away later.
        for (i, flat) in (0..m).map(|i| (i, false)).chain((0..m).map(|i| (i, true))) {
            if clipped {
                break;
            }
            let (a, b, c) = (poly[(i + m - 1) % m], poly[i], poly[(i + 1) % m]);
            let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
            let turn = cross(pa, pb, pc);
            if if flat { turn < -1e-9 } else { turn <= 1e-12 } {
                continue;
            }
            let blocked = poly.iter().any(|&v| {
                if v == a || v == b || v == c || pts[v] == pa || pts[v] == pb || pts[v] == pc {
                    return false;
                }
                let p = pts[v];
                cross(pa, pb, p) > 0.0 && cross(pb, pc, p) > 0.0 && cross(pc, pa, p) > 0.0
            });
            if blocked {
                continue;
            }
            out.push([a, b, c]);
            poly.remove(i);
            clipped = true;
        }
        if !clipped {
            return None;
        }
    }
    if poly.len() == 3 {
        out.push([poly[0], poly[1], poly[2]]);
    }
    Some(out)
}

/// A soup and its tree over some faces, to cast vertical rays at them.
pub fn caster(positions: &[Vec3], tris: &[[u32; 3]], skip: &[bool]) -> (Bvh, TriSoup) {
    let mut soup = TriSoup::default();
    for (t, f) in tris.iter().enumerate() {
        if !skip.get(t).copied().unwrap_or(false) {
            soup.push(f.map(|v| positions[v as usize]), 0, t as u32);
        }
    }
    (Bvh::build(&soup), soup)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box `size` on each side from the origin, outward winding.
    pub(crate) fn cube(size: f32) -> Vec<[Vec3; 3]> {
        let p = |x: f32, y: f32, z: f32| [x * size, y * size, z * size];
        let v = [
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ];
        let f = [
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
        f.iter().map(|t| t.map(|i| v[i])).collect()
    }

    pub(crate) fn closed(tris: &[[u32; 3]]) -> bool {
        let mut count: HashMap<(u32, u32), i32> = HashMap::new();
        for t in tris {
            for k in 0..3 {
                *count.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        // Every edge once each way: closed and consistently wound.
        count.iter().all(|(&(a, b), &n)| n == 1 && count.get(&(b, a)) == Some(&1))
    }

    fn volume(positions: &[Vec3], tris: &[[u32; 3]]) -> f32 {
        tris.iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| positions[v as usize]);
                geom::dot(a, geom::cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn the_inner_wall_of_a_cube_is_a_closed_smaller_cube_facing_in() {
        let tris = cube(20.0);
        let wall = 2.0;
        let cell = cell_for(20.0, wall);
        let shell = inner_wall(&tris, wall, cell).unwrap();
        assert!(closed(&shell.tris));
        // It faces into the cavity: its volume comes out negative, about a 16 mm cube.
        let v = volume(&shell.positions, &shell.tris);
        assert!(v < 0.0 && (-v - 16f32.powi(3)).abs() / 16f32.powi(3) < 0.12, "{v}");
        // Never closer to the outside than the wall, give or take a cell.
        for p in &shell.positions {
            let d = (0..3).map(|k| p[k].min(20.0 - p[k])).fold(f32::MAX, f32::min);
            assert!(d > wall - cell && d < wall + cell, "{p:?} {d}");
        }
    }

    #[test]
    fn lightening_the_wall_keeps_it_closed_and_where_it_was() {
        let tris = cube(20.0);
        let cell = cell_for(20.0, 2.0);
        let shell = inner_wall(&tris, 2.0, cell).unwrap();
        let before = shell.tris.len();
        let light = lighten(shell, cell * 0.25);
        assert!(light.tris.len() * 10 < before, "{} -> {}", before, light.tris.len());
        assert!(closed(&light.tris));
        let v = -volume(&light.positions, &light.tris);
        assert!((v - 16f32.powi(3)).abs() / 16f32.powi(3) < 0.12, "{v}");
    }

    #[test]
    fn a_wall_too_thick_leaves_no_cavity() {
        assert_eq!(inner_wall(&cube(4.0), 3.0, 0.5).unwrap_err(), Refusal::NoRoom);
    }

    #[test]
    fn the_ring_between_a_square_and_a_circle_covers_the_difference() {
        let outer = [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let hole: Vec<[f32; 2]> = (0..DRAIN_SIDES)
            .map(|s| {
                let a = -(s as f32) / DRAIN_SIDES as f32 * std::f32::consts::TAU;
                [2.0 * a.cos(), 2.0 * a.sin()]
            })
            .collect();
        let tris = triangulate_ring(&outer, &hole).unwrap();
        let pts: Vec<[f32; 2]> = outer.iter().chain(&hole).copied().collect();
        let area: f32 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| pts[i]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0
            })
            .sum();
        let circle = signed_area(&hole).abs();
        assert!((area - (100.0 - circle)).abs() < 1e-3, "{area}");
        assert_eq!(tris.len(), outer.len() + hole.len());
    }
}
