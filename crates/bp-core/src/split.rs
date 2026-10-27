//! Splits a painted model into solid pieces, one or more per filament colour,
//! that fit back together into the whole part.
//!
//! The solid is voxelised and every inside voxel takes the colour of the
//! nearest point of the painted surface, so a leg painted black becomes a solid
//! black leg instead of a hollow skin. Optionally colours only reach `depth_mm`
//! inward and the rest becomes a core of one colour. Each connected region of
//! one colour is a piece, crumbs smaller than `min_piece_mm3` join a neighbour,
//! and every piece is meshed with marching tetrahedra: closed, and sharing the
//! same mating surface as its neighbours. Where a piece meets the outside its
//! vertices lie on the original surface.

use std::collections::{HashMap, VecDeque};

use rayon::prelude::*;

use crate::bvh::{Bvh, TriSoup};
use crate::geom::{self, Bary, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitParams {
    /// Voxel edge in millimetres; grown when the grid would be too large.
    pub voxel_mm: f32,
    /// How far each colour reaches inward; 0 lets it reach the middle.
    pub depth_mm: f32,
    /// Colour of the core when `depth_mm` > 0; `None` takes the most painted colour.
    pub core_slot: Option<u8>,
    /// Pieces smaller than this join the neighbour they touch most.
    pub min_piece_mm3: f32,
}

impl Default for SplitParams {
    fn default() -> Self {
        Self { voxel_mm: 0.5, depth_mm: 0.0, core_slot: None, min_piece_mm3: 20.0 }
    }
}

pub struct Piece {
    pub slot: u8,
    /// Millimetres, in place in the assembled model.
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub volume_mm3: f32,
    /// Neighbouring pieces and the area each one touches, in mm².
    pub contacts: Vec<(usize, f32)>,
}

pub struct Split {
    /// Sorted by colour, then largest first.
    pub pieces: Vec<Piece>,
    /// The voxel size actually used.
    pub voxel_mm: f32,
    /// Crumbs smaller than the least piece that went to a neighbour.
    pub merged: usize,
    /// Regions too thin to give a surface, left out.
    pub dropped: usize,
}

/// Positions and triangles of one mesh.
type MeshData = (Vec<Vec3>, Vec<[u32; 3]>);

const OUT: u8 = u8::MAX;
const NONE: u32 = u32::MAX;
const MAX_VOXELS: usize = 24_000_000;

/// Face, edge and corner neighbours.
const N26: [[i32; 3]; 26] = {
    let mut out = [[0; 3]; 26];
    let mut n = 0;
    let mut i = 0;
    while i < 27 {
        let d = [i % 3 - 1, i / 3 % 3 - 1, i / 9 - 1];
        if d[0] != 0 || d[1] != 0 || d[2] != 0 {
            out[n] = d;
            n += 1;
        }
        i += 1;
    }
    out
};

/// Neighbours joined by an edge of the tetrahedra below: the connectivity the
/// meshed surface actually has.
const N14: [[i32; 3]; 14] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
    [1, 1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, -1],
    [1, 1, 1],
    [-1, -1, -1],
];

const N6: [[i32; 3]; 6] = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

/// Six tetrahedra sharing the cube diagonal (Kuhn): each walks from corner 0
/// to corner 7 one axis at a time. Corners are bit masks x=1, y=2, z=4.
const TETS: [[usize; 4]; 6] = [[0, 1, 3, 7], [0, 1, 5, 7], [0, 2, 3, 7], [0, 2, 6, 7], [0, 4, 5, 7], [0, 4, 6, 7]];

struct Grid {
    origin: Vec3,
    h: f32,
    n: [usize; 3],
}

impl Grid {
    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }
    fn index(&self, c: [usize; 3]) -> usize {
        c[0] + self.n[0] * (c[1] + self.n[1] * c[2])
    }
    fn coords(&self, i: usize) -> [usize; 3] {
        [i % self.n[0], i / self.n[0] % self.n[1], i / (self.n[0] * self.n[1])]
    }
    fn center(&self, i: usize) -> Vec3 {
        let c = self.coords(i);
        std::array::from_fn(|a| self.origin[a] + (c[a] as f32 + 0.5) * self.h)
    }
    fn offsets<const K: usize>(&self, dirs: &[[i32; 3]; K]) -> [isize; K] {
        let (nx, ny) = (self.n[0] as isize, self.n[1] as isize);
        dirs.map(|d| d[0] as isize + nx * (d[1] as isize + ny * d[2] as isize))
    }
    /// True for voxels on the outermost layer, which are always outside.
    fn on_border(&self, i: usize) -> bool {
        let c = self.coords(i);
        (0..3).any(|a| c[a] == 0 || c[a] + 1 >= self.n[a])
    }
}

#[inline]
fn step(i: usize, off: isize) -> usize {
    (i as isize + off) as usize
}

/// Inside test by casting one ray per voxel column along each axis and
/// counting signed crossings (so overlapping closed parts make a union); a
/// voxel is inside when at least two of the three axes agree.
fn inside(grid: &Grid, bvh: &Bvh, soup: &TriSoup) -> Vec<bool> {
    let mut votes = vec![0u8; grid.len()];
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        let columns = grid.n[b] * grid.n[c];
        let spans: Vec<Vec<(u32, u32)>> = (0..columns)
            .into_par_iter()
            .map(|col| {
                let (ub, uc) = (col % grid.n[b], col / grid.n[b]);
                let mut o = [0.0f32; 3];
                // A tiny offset keeps rays off the edges of axis-aligned meshes.
                o[axis] = grid.origin[axis] - grid.h;
                o[b] = grid.origin[b] + (ub as f32 + 0.5 + 0.00137) * grid.h;
                o[c] = grid.origin[c] + (uc as f32 + 0.5 + 0.00291) * grid.h;
                let mut d = [0.0f32; 3];
                d[axis] = 1.0;
                let mut hits = Vec::new();
                bvh.ray_crossings(soup, o, d, &mut hits);
                hits.sort_by(|x, y| x.0.total_cmp(&y.0));
                let mut spans = Vec::new();
                let (mut winding, mut next, mut start) = (0i32, 0usize, None);
                for m in 0..grid.n[axis] {
                    let t = (m as f32 + 1.5) * grid.h;
                    while next < hits.len() && hits[next].0 < t {
                        winding += hits[next].1 as i32;
                        next += 1;
                    }
                    match (winding > 0, start) {
                        (true, None) => start = Some(m as u32),
                        (false, Some(s)) => {
                            spans.push((s, m as u32));
                            start = None;
                        }
                        _ => {}
                    }
                }
                if let Some(s) = start {
                    spans.push((s, grid.n[axis] as u32));
                }
                spans
            })
            .collect();
        for (col, spans) in spans.iter().enumerate() {
            let mut cell = [0usize; 3];
            cell[b] = col % grid.n[b];
            cell[c] = col / grid.n[b];
            for &(s, e) in spans {
                for m in s..e {
                    cell[axis] = m as usize;
                    votes[grid.index(cell)] += 1;
                }
            }
        }
    }
    votes.par_iter().enumerate().map(|(i, &v)| v >= 2 && !grid.on_border(i)).collect()
}

/// Connected regions of equal label (14-connected), `NONE` outside.
fn components(grid: &Grid, lab: &[u8]) -> (Vec<u32>, Vec<u32>, Vec<u8>) {
    let off = grid.offsets(&N14);
    let mut comp = vec![NONE; lab.len()];
    let (mut sizes, mut labels) = (vec![], vec![]);
    let mut stack = vec![];
    for start in 0..lab.len() {
        if lab[start] == OUT || comp[start] != NONE {
            continue;
        }
        let id = sizes.len() as u32;
        let l = lab[start];
        comp[start] = id;
        stack.push(start);
        let mut size = 0u32;
        while let Some(v) = stack.pop() {
            size += 1;
            for &o in &off {
                let n = step(v, o);
                if lab[n] == l && comp[n] == NONE {
                    comp[n] = id;
                    stack.push(n);
                }
            }
        }
        sizes.push(size);
        labels.push(l);
    }
    (comp, sizes, labels)
}

/// Gives every region smaller than `min_voxels` the label it touches most.
/// Regions that touch nothing (loose bits of the model) keep theirs.
/// Returns how many regions it gave away.
fn merge_crumbs(grid: &Grid, lab: &mut [u8], min_voxels: u32) -> usize {
    let off = grid.offsets(&N6);
    let mut merged = 0;
    for _ in 0..6 {
        let (comp, sizes, _) = components(grid, lab);
        let small: Vec<bool> = sizes.iter().map(|&s| s < min_voxels).collect();
        let mut tally: HashMap<(u32, u8), u32> = HashMap::new();
        for v in 0..lab.len() {
            let c = comp[v];
            if c == NONE || !small[c as usize] {
                continue;
            }
            for &o in &off {
                let n = lab[step(v, o)];
                if n != OUT && n != lab[v] {
                    *tally.entry((c, n)).or_default() += 1;
                }
            }
        }
        let mut best: HashMap<u32, (u8, u32)> = HashMap::new();
        for ((c, l), count) in tally {
            let e = best.entry(c).or_insert((l, 0));
            if count > e.1 || (count == e.1 && l < e.0) {
                *e = (l, count);
            }
        }
        if best.is_empty() {
            return merged;
        }
        merged += best.len();
        for v in 0..lab.len() {
            if comp[v] != NONE {
                if let Some(&(l, _)) = best.get(&comp[v]) {
                    lab[v] = l;
                }
            }
        }
    }
    merged
}

/// What decided each voxel's colour, to place the surface between two
/// regions where that decision flips instead of half way between voxels.
struct Field<'a> {
    grid: &'a Grid,
    lab: &'a [u8],
    seeds: &'a [(Vec3, u8)],
    seed_of: &'a [u32],
    core: Option<u8>,
    depth: f32,
}

impl Field<'_> {
    fn dist(&self, v: usize, seed: u32) -> f32 {
        geom::len(geom::sub(self.grid.center(v), self.seeds[seed as usize].0))
    }

    /// The voxel's seed and whether it fell in the core, if its label still
    /// comes from them (crumb merging may have overruled it).
    fn origin(&self, v: usize) -> Option<(u32, bool)> {
        let s = self.seed_of[v];
        if s == NONE {
            return None;
        }
        let in_core = self.core.is_some() && self.dist(v, s) > self.depth;
        let natural = if in_core { self.core } else { Some(self.seeds[s as usize].1) };
        (natural == Some(self.lab[v])).then_some((s, in_core))
    }

    /// Fraction of the way from `a` to `b` (`a` < `b`) where the colour changes.
    fn crossing(&self, a: usize, b: usize) -> f32 {
        let t = match (self.origin(a), self.origin(b)) {
            // Equidistant from the two seeds: the Voronoi face between them.
            (Some((sa, false)), Some((sb, false))) => {
                let ga = self.dist(a, sa) - self.dist(a, sb);
                let gb = self.dist(b, sa) - self.dist(b, sb);
                if ga < 0.0 && gb > 0.0 {
                    ga / (ga - gb)
                } else {
                    0.5
                }
            }
            // At `depth` from the coloured side's seed.
            (Some((s, false)), Some((_, true))) | (Some((_, true)), Some((s, false))) => {
                let (fa, fb) = (self.dist(a, s) - self.depth, self.dist(b, s) - self.depth);
                if (fa < 0.0) != (fb < 0.0) {
                    fa / (fa - fb)
                } else {
                    0.5
                }
            }
            _ => 0.5,
        };
        t.clamp(0.05, 0.95)
    }
}

/// Marching tetrahedra over the cells around one region's boundary.
fn mesh_region(field: &Field, comp: &[u32], id: u32, boundary: &[usize], bvh: &Bvh, soup: &TriSoup) -> RegionMesh {
    let (grid, lab) = (field.grid, field.lab);
    let (nx, ny) = (grid.n[0], grid.n[1]);
    let corner: [usize; 8] = std::array::from_fn(|b| (b & 1) + nx * (((b >> 1) & 1) + ny * (b >> 2)));
    let mut cells: Vec<usize> = boundary.iter().flat_map(|&v| (0..8).map(move |b| v - corner[b])).collect();
    cells.sort_unstable();
    cells.dedup();

    let member = |v: usize| comp[v] == id;
    let mut verts: HashMap<(usize, usize), u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut keys = Vec::new();
    let mut tris = Vec::new();
    // Where the surface crosses the edge from voxel `a` (in the region) to `b`
    // (not): on the model surface if `b` is outside, else where the colour
    // changes. Both regions compute the same point, so they mate exactly.
    let mut point = |a: usize, b: usize, positions: &mut Vec<Vec3>| -> u32 {
        *verts.entry((a.min(b), a.max(b))).or_insert_with(|| {
            keys.push((a.min(b), a.max(b)));
            let (pa, pb) = (grid.center(a), grid.center(b));
            let p = if lab[b] == OUT {
                bvh.raycast(soup, pa, geom::sub(pb, pa))
                    .filter(|hit| hit.t <= 1.0)
                    .map_or(geom::scale(geom::add(pa, pb), 0.5), |hit| hit.point)
            } else {
                let (lo, hi) = (a.min(b), a.max(b));
                let (plo, phi) = (grid.center(lo), grid.center(hi));
                geom::add(plo, geom::scale(geom::sub(phi, plo), field.crossing(lo, hi)))
            };
            positions.push(p);
            positions.len() as u32 - 1
        })
    };
    for cell in cells {
        let ids: [usize; 8] = corner.map(|c| cell + c);
        let ins = ids.map(member);
        if ins.iter().all(|&x| x) || ins.iter().all(|&x| !x) {
            continue;
        }
        for tet in TETS {
            let v = tet.map(|k| ids[k]);
            let inn: Vec<usize> = (0..4).filter(|&k| ins[tet[k]]).map(|k| v[k]).collect();
            let out: Vec<usize> = (0..4).filter(|&k| !ins[tet[k]]).map(|k| v[k]).collect();
            // Each face as the tetrahedron edges it crosses.
            let faces: Vec<[(usize, usize); 3]> = match (inn.len(), out.len()) {
                (1, 3) => vec![[(inn[0], out[0]), (inn[0], out[1]), (inn[0], out[2])]],
                (3, 1) => vec![[(inn[0], out[0]), (inn[1], out[0]), (inn[2], out[0])]],
                (2, 2) => {
                    let (ac, ad, bd, bc) = ((inn[0], out[0]), (inn[0], out[1]), (inn[1], out[1]), (inn[1], out[0]));
                    vec![[ac, ad, bd], [ac, bd, bc]]
                }
                _ => continue,
            };
            // Face away from the region, judged on the edge midpoints so that a
            // face squashed flat on the model surface still gets a definite side.
            let centroid = |s: &[usize]| {
                let sum = s.iter().fold([0.0f32; 3], |acc, &i| geom::add(acc, grid.center(i)));
                geom::scale(sum, 1.0 / s.len() as f32)
            };
            let outward = geom::sub(centroid(&out), centroid(&inn));
            for f in faces {
                let m = f.map(|(a, b)| geom::scale(geom::add(grid.center(a), grid.center(b)), 0.5));
                let n = geom::cross(geom::sub(m[1], m[0]), geom::sub(m[2], m[0]));
                let ids = f.map(|(a, b)| point(a, b, &mut positions));
                tris.push(if geom::dot(n, outward) < 0.0 { [ids[0], ids[2], ids[1]] } else { ids });
            }
        }
    }
    RegionMesh { positions, keys, tris }
}

/// Every directed edge is matched by as many in the opposite direction: the
/// solid is sealed and its inside well defined, though an edge may be shared
/// by four triangles where two sheets touch (slicers accept that).
pub fn watertight(tris: &[[u32; 3]]) -> bool {
    let mut edges: HashMap<(u32, u32), i32> = HashMap::with_capacity(tris.len() * 3);
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if a == b {
                return false;
            }
            *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
        }
    }
    !tris.is_empty() && edges.values().all(|&n| n == 0)
}

/// Drops pairs of triangles on the same three vertices facing opposite ways:
/// a fold left where a collapse flattened a thin spot.
fn cancel_folds(tris: Vec<[u32; 3]>) -> Vec<[u32; 3]> {
    let key = |t: &[u32; 3]| {
        let mut k = *t;
        k.sort_unstable();
        k
    };
    let mut count: HashMap<[u32; 3], u32> = HashMap::with_capacity(tris.len());
    for t in &tris {
        *count.entry(key(t)).or_default() += 1;
    }
    if count.len() == tris.len() {
        return tris;
    }
    tris.into_iter().filter(|t| count[&key(t)] == 1).collect()
}

/// One region's surface straight from marching tetrahedra. Each vertex
/// remembers the voxel edge it lies on; a neighbouring region's surface uses
/// the same edge for the same point, which is how the polish below keeps
/// mating faces together.
struct RegionMesh {
    positions: Vec<Vec3>,
    keys: Vec<(usize, usize)>,
    tris: Vec<[u32; 3]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// On the model's surface.
    Outer,
    /// Where two pieces meet, beside the model's surface: the colour border.
    Seam,
    /// Where two pieces meet, inside.
    Mate,
}

/// Rounds of smoothing on the model's surface and between pieces; each moves a
/// vertex at most half way to its neighbours. Steps one voxel high need many.
const SURFACE_ROUNDS: usize = 6;
const MATE_ROUNDS: usize = 40;
/// How far simplification may move a surface, in millimetres: well under a
/// layer, so the facets do not show in the print.
const SURFACE_ERROR_MM: f32 = 0.05;

/// Takes the voxel steps out of the surfaces, the same way for every piece so
/// they still mate exactly, then simplifies what only one piece owns.
///
/// - The colour border is pulled onto the model's surface, so neighbouring
///   colours meet there without a groove, and straightened along its length.
/// - Faces on the model slide along it to even out (tangential smoothing,
///   pushed back onto the model each round), losing the lumps of the grid.
/// - Faces between pieces are smoothed in place (Taubin, which does not shrink).
fn polish(regions: &[RegionMesh], comp: &[u32], bvh: &Bvh, soup: &TriSoup, h: f32) -> Vec<MeshData> {
    let mut id: HashMap<(usize, usize), u32> = HashMap::new();
    let (mut pos, mut kind) = (Vec::new(), Vec::new());
    let mut local: Vec<Vec<u32>> = Vec::with_capacity(regions.len());
    for r in regions {
        let mut ids = Vec::with_capacity(r.keys.len());
        for (&k, &p) in r.keys.iter().zip(&r.positions) {
            ids.push(*id.entry(k).or_insert_with(|| {
                pos.push(p);
                kind.push(if comp[k.0] == NONE || comp[k.1] == NONE { Kind::Outer } else { Kind::Mate });
                pos.len() as u32 - 1
            }));
        }
        local.push(ids);
    }
    drop(id);
    let mut nbrs: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for (r, ids) in regions.iter().zip(&local) {
        for t in &r.tris {
            let g = t.map(|v| ids[v as usize]);
            for k in 0..3 {
                let (a, b) = (g[k], g[(k + 1) % 3]);
                nbrs[a as usize].push(b);
                nbrs[b as usize].push(a);
            }
        }
    }
    nbrs.par_iter_mut().for_each(|n| {
        n.sort_unstable();
        n.dedup();
    });
    let seam: Vec<bool> = (0..pos.len())
        .map(|v| kind[v] == Kind::Mate && nbrs[v].iter().any(|&n| kind[n as usize] == Kind::Outer))
        .collect();
    for (v, s) in seam.iter().enumerate() {
        if *s {
            kind[v] = Kind::Seam;
        }
    }

    // Nearest point of the model and its normal, if within `reach`.
    let onto = |q: Vec3, reach: f32| -> Option<(Vec3, Vec3)> {
        let (t, b) = bvh.nearest_within(soup, q, reach)?;
        let c = &soup.corners[t as usize];
        Some((geom::at(c, b), geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])))))
    };
    let average = |pos: &[Vec3], v: usize, keep: &dyn Fn(Kind) -> bool| -> Option<Vec3> {
        let (mut sum, mut n) = ([0.0f32; 3], 0);
        for &w in &nbrs[v] {
            if keep(kind[w as usize]) {
                sum = geom::add(sum, pos[w as usize]);
                n += 1;
            }
        }
        (n > 0).then(|| geom::scale(sum, 1.0 / n as f32))
    };
    // The surface vertices with the model's normal under each; seams start
    // on the model, wherever the grid left them.
    let surface: Vec<usize> = (0..pos.len()).filter(|&v| kind[v] != Kind::Mate).collect();
    let mut normal: Vec<Option<Vec3>> = vec![None; pos.len()];
    let first: Vec<(Vec3, Option<Vec3>)> = surface
        .par_iter()
        .map(|&v| {
            let reach = if kind[v] == Kind::Seam { 1.5 * h } else { 0.75 * h };
            onto(pos[v], reach).map_or((pos[v], None), |(p, n)| (p, Some(n)))
        })
        .collect();
    for (&v, (p, n)) in surface.iter().zip(first) {
        pos[v] = p;
        normal[v] = n;
    }
    // Faces on the model slide along it; the border runs along itself.
    for _ in 0..SURFACE_ROUNDS {
        let moved: Vec<(Vec3, Option<Vec3>)> = surface
            .par_iter()
            .map(|&v| {
                let (p, n) = (pos[v], normal[v]);
                let Some(n) = n else { return (p, None) };
                let avg = if kind[v] == Kind::Seam {
                    average(&pos, v, &|k| k == Kind::Seam)
                } else {
                    average(&pos, v, &|k| k != Kind::Mate)
                };
                let Some(avg) = avg else { return (p, Some(n)) };
                let mut d = geom::sub(avg, p);
                if kind[v] == Kind::Outer {
                    d = geom::sub(d, geom::scale(n, geom::dot(d, n)));
                }
                onto(geom::add(p, geom::scale(d, 0.5)), 0.75 * h).map_or((p, Some(n)), |(q, m)| (q, Some(m)))
            })
            .collect();
        for (&v, (p, n)) in surface.iter().zip(moved) {
            pos[v] = p;
            normal[v] = n;
        }
    }
    // Taubin, a shrinking step and an inflating one, for the faces between pieces.
    let inside: Vec<usize> = (0..pos.len()).filter(|&v| kind[v] == Kind::Mate).collect();
    for f in std::iter::repeat_n([0.5f32, -0.53], MATE_ROUNDS).flatten() {
        let moved: Vec<Vec3> = inside
            .par_iter()
            .map(|&v| {
                average(&pos, v, &|_| true)
                    .map_or(pos[v], |avg| geom::add(pos[v], geom::scale(geom::sub(avg, pos[v]), f)))
            })
            .collect();
        for (&v, p) in inside.iter().zip(moved) {
            pos[v] = p;
        }
    }

    simplify_patches(regions, &local, comp, &pos, SURFACE_ERROR_MM)
}

/// A region and what lies across its patch: another region, `NONE` for the
/// outside, or a junction of several.
type PatchKey = (u32, u32);

/// Simplifies the surfaces patch by patch: each region's skin on the model,
/// and each face between two regions once, used as is by one and flipped by
/// the other, so they keep mating exactly. Vertices that more than one patch
/// uses stay put, which keeps every piece sealed. Faces where three regions,
/// or two and the outside, meet are left as they are.
fn simplify_patches(
    regions: &[RegionMesh],
    local: &[Vec<u32>],
    comp: &[u32],
    pos: &[Vec3],
    error: f32,
) -> Vec<MeshData> {
    // The patch of each face: the region on the other side of all its
    // vertices (`NONE` for the outside), or `JUNCTION` when they disagree.
    const JUNCTION: u32 = NONE - 1;
    let patch_of: Vec<Vec<u32>> = regions
        .iter()
        .enumerate()
        .map(|(r, m)| {
            let other = |v: u32| {
                let (a, b) = m.keys[v as usize];
                if comp[a] == r as u32 {
                    comp[b]
                } else {
                    comp[a]
                }
            };
            m.tris
                .iter()
                .map(|t| {
                    let o = other(t[0]);
                    if other(t[1]) == o && other(t[2]) == o {
                        o
                    } else {
                        JUNCTION
                    }
                })
                .collect()
        })
        .collect();
    let key = |r: usize, o: u32| -> PatchKey {
        match o {
            NONE | JUNCTION => (r as u32, o),
            _ => ((r as u32).min(o), (r as u32).max(o)),
        }
    };
    // A vertex is free only while one patch alone uses it.
    let mut owner: Vec<Option<PatchKey>> = vec![None; pos.len()];
    let mut shared = vec![false; pos.len()];
    for (r, (m, ids)) in regions.iter().zip(local).enumerate() {
        for (t, &o) in m.tris.iter().zip(&patch_of[r]) {
            let k = key(r, o);
            for &v in t {
                let g = ids[v as usize] as usize;
                match owner[g] {
                    None => owner[g] = Some(k),
                    Some(x) if x != k || o == JUNCTION => shared[g] = true,
                    _ => {}
                }
            }
        }
    }
    drop(owner);
    // Faces of each patch in global vertices, taken from the lower region.
    let mut patches: HashMap<PatchKey, Vec<[u32; 3]>> = HashMap::new();
    let mut junctions: Vec<Vec<[u32; 3]>> = vec![Vec::new(); regions.len()];
    for (r, (m, ids)) in regions.iter().zip(local).enumerate() {
        for (t, &o) in m.tris.iter().zip(&patch_of[r]) {
            let g = t.map(|v| ids[v as usize]);
            if o == JUNCTION {
                junctions[r].push(g);
            } else if o == NONE || (r as u32) < o {
                patches.entry(key(r, o)).or_default().push(g);
            }
        }
    }
    let adapter = meshopt::VertexDataAdapter::new(meshopt::typed_to_bytes(pos), std::mem::size_of::<Vec3>(), 0);
    let simplified: Vec<(PatchKey, Vec<[u32; 3]>)> = patches
        .into_par_iter()
        .map(|(k, tris)| {
            let Ok(adapter) = &adapter else { return (k, tris) };
            let indices: Vec<u32> = tris.iter().flatten().copied().collect();
            let out = meshopt::simplify_with_locks(
                &indices,
                adapter,
                &shared,
                0,
                error,
                meshopt::SimplifyOptions::ErrorAbsolute
                    | meshopt::SimplifyOptions::Sparse
                    | meshopt::SimplifyOptions::LockBorder
                    | meshopt::SimplifyOptions::RegularizeLight,
                None,
            );
            let out: Vec<[u32; 3]> = out.as_chunks::<3>().0.to_vec();
            (k, if out.is_empty() { tris } else { out })
        })
        .collect();
    let mut faces = junctions;
    for ((r, o), tris) in simplified {
        faces[r as usize].extend_from_slice(&tris);
        if o != NONE {
            faces[o as usize].extend(tris.iter().map(|t| [t[0], t[2], t[1]]));
        }
    }
    faces
        .into_par_iter()
        .map(|tris| {
            let tris = cancel_folds(tris);
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut positions = Vec::new();
            let tris = tris
                .iter()
                .map(|t| {
                    t.map(|g| {
                        *remap.entry(g).or_insert_with(|| {
                            positions.push(pos[g as usize]);
                            positions.len() as u32 - 1
                        })
                    })
                })
                .collect();
            (positions, tris)
        })
        .collect()
}

/// `soup` is the painted surface in millimetres; `label(soup_tri, bary)` is the
/// colour at a point of it.
pub fn split(soup: &TriSoup, label: impl Fn(u32, Bary) -> u8 + Sync, params: SplitParams) -> Split {
    if soup.is_empty() {
        return Split { pieces: vec![], voxel_mm: params.voxel_mm, merged: 0, dropped: 0 };
    }
    let bvh = Bvh::build(soup);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in soup.corners.iter().flatten() {
        lo = std::array::from_fn(|a| lo[a].min(p[a]));
        hi = std::array::from_fn(|a| hi[a].max(p[a]));
    }
    let mut h = params.voxel_mm.max(0.05);
    let dims = |h: f32| -> [usize; 3] { std::array::from_fn(|a| ((hi[a] - lo[a]) / h).ceil() as usize + 4) };
    while dims(h).iter().product::<usize>() > MAX_VOXELS {
        h *= 1.1;
    }
    let grid = Grid { origin: std::array::from_fn(|a| lo[a] - 2.0 * h), h, n: dims(h) };

    let inside = inside(&grid, &bvh, soup);
    let off6 = grid.offsets(&N6);
    let surface: Vec<usize> =
        (0..grid.len()).into_par_iter().filter(|&i| inside[i] && off6.iter().any(|&o| !inside[step(i, o)])).collect();
    // The nearest painted point to every surface voxel seeds the colours.
    let seeds: Vec<(Vec3, u8)> = surface
        .par_iter()
        .map(|&i| {
            let q = grid.center(i);
            bvh.nearest(soup, q).map(|(t, b)| (geom::at(&soup.corners[t as usize], b), label(t, b))).unwrap_or((q, 0))
        })
        .collect();

    // Spread each seed inward, keeping for every voxel the nearest seed.
    let off26 = grid.offsets(&N26);
    let mut seed_of = vec![NONE; grid.len()];
    let mut queue: VecDeque<usize> = VecDeque::with_capacity(surface.len() * 2);
    for (k, &i) in surface.iter().enumerate() {
        seed_of[i] = k as u32;
        queue.push_back(i);
    }
    let d2 = |a: Vec3, b: Vec3| {
        let d = geom::sub(a, b);
        geom::dot(d, d)
    };
    while let Some(v) = queue.pop_front() {
        let s = seed_of[v];
        let sp = seeds[s as usize].0;
        for &o in &off26 {
            let n = step(v, o);
            if !inside[n] || seed_of[n] == s {
                continue;
            }
            let c = grid.center(n);
            if seed_of[n] == NONE || d2(c, sp) < d2(c, seeds[seed_of[n] as usize].0) - 1e-9 {
                seed_of[n] = s;
                queue.push_back(n);
            }
        }
    }

    let core = (params.depth_mm > 0.0).then(|| {
        params.core_slot.unwrap_or_else(|| {
            let mut count = [0usize; 256];
            seeds.iter().for_each(|s| count[s.1 as usize] += 1);
            (0..256).max_by_key(|&l| count[l]).unwrap_or(0) as u8
        })
    });
    let depth2 = params.depth_mm * params.depth_mm;
    let mut lab: Vec<u8> = (0..grid.len())
        .into_par_iter()
        .map(|i| {
            if !inside[i] {
                return OUT;
            }
            let s = seed_of[i];
            if s == NONE {
                return core.unwrap_or(0);
            }
            let (p, l) = seeds[s as usize];
            match core {
                Some(c) if d2(grid.center(i), p) > depth2 => c,
                _ => l,
            }
        })
        .collect();
    drop(inside);

    let min_voxels = (params.min_piece_mm3 / (h * h * h)).ceil().max(1.0) as u32;
    let merged = merge_crumbs(&grid, &mut lab, min_voxels);
    let (comp, sizes, labels) = components(&grid, &lab);

    // Contact area between regions, and each region's boundary voxels.
    let mut faces: HashMap<(u32, u32), u32> = HashMap::new();
    let mut boundary: Vec<Vec<usize>> = vec![vec![]; sizes.len()];
    let off14 = grid.offsets(&N14);
    for v in 0..grid.len() {
        let c = comp[v];
        if c == NONE {
            continue;
        }
        for &o in &off6[..] {
            let n = comp[step(v, o)];
            if n != NONE && n > c {
                *faces.entry((c, n)).or_default() += 1;
            }
        }
        if off14.iter().any(|&o| comp[step(v, o)] != c) {
            boundary[c as usize].push(v);
        }
    }

    let field = Field { grid: &grid, lab: &lab, seeds: &seeds, seed_of: &seed_of, core, depth: params.depth_mm };
    let regions: Vec<RegionMesh> = (0..sizes.len())
        .into_par_iter()
        .map(|c| mesh_region(&field, &comp, c as u32, &boundary[c], &bvh, soup))
        .collect();
    let meshes = polish(&regions, &comp, &bvh, soup, h);

    // Order by colour, then largest first, and renumber the contacts.
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&c| !meshes[c].1.is_empty()).collect();
    let dropped = sizes.len() - order.len();
    order.sort_by_key(|&c| (labels[c], std::cmp::Reverse(sizes[c])));
    let mut new_id = vec![usize::MAX; sizes.len()];
    for (k, &c) in order.iter().enumerate() {
        new_id[c] = k;
    }
    let mut pieces: Vec<Piece> = Vec::with_capacity(order.len());
    let mut meshes: Vec<Option<MeshData>> = meshes.into_iter().map(Some).collect();
    for &c in &order {
        let (positions, tris) = meshes[c].take().unwrap_or_default();
        pieces.push(Piece {
            slot: labels[c],
            positions,
            tris,
            volume_mm3: sizes[c] as f32 * h * h * h,
            contacts: vec![],
        });
    }
    for ((a, b), n) in faces {
        let (a, b) = (new_id[a as usize], new_id[b as usize]);
        if a != usize::MAX && b != usize::MAX {
            let area = n as f32 * h * h;
            pieces[a].contacts.push((b, area));
            pieces[b].contacts.push((a, area));
        }
    }
    for p in &mut pieces {
        p.contacts.sort_by(|x, y| y.1.total_cmp(&x.1));
    }
    Split { pieces, voxel_mm: h, merged, dropped }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A closed box from `lo` to `hi`, outward normals.
    pub(crate) fn cuboid(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let p = (0..8).map(|b| [[lo[0], hi[0]][b & 1], [lo[1], hi[1]][(b >> 1) & 1], [lo[2], hi[2]][b >> 2]]).collect();
        let tris = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        (p, tris)
    }

    fn soup(p: &[Vec3], tris: &[[u32; 3]]) -> TriSoup {
        let mut s = TriSoup::default();
        for (i, t) in tris.iter().enumerate() {
            s.push(t.map(|v| p[v as usize]), 0, i as u32);
        }
        s
    }

    fn volume(p: &Piece) -> f32 {
        p.tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| p.positions[v as usize]);
                geom::dot(a, geom::cross(b, c)) / 6.0
            })
            .sum()
    }

    /// A 20 mm cube whose top face is colour 1 and the rest colour 0.
    fn painted_cube(params: SplitParams) -> Split {
        let (p, t) = cuboid([0.0; 3], [20.0; 3]);
        let s = soup(&p, &t);
        let top = |tri: u32, _b: Bary| u8::from(tri == 2 || tri == 3);
        split(&s, top, params)
    }

    #[test]
    fn colours_become_solid_closed_pieces_that_fill_the_part() {
        let s = painted_cube(SplitParams { voxel_mm: 1.0, ..Default::default() });
        assert_eq!(s.pieces.len(), 2);
        assert!(s.pieces.iter().all(|p| watertight(&p.tris)));
        // Solid, not skins: together they fill the 8000 mm³ cube.
        let total: f32 = s.pieces.iter().map(volume).sum();
        assert!((total - 8000.0).abs() < 400.0, "{total}");
        let top = s.pieces.iter().find(|p| p.slot == 1).unwrap();
        // Nearest-surface split of a cube: the top colour is a pyramid to the centre.
        assert!(volume(top) > 1000.0 && volume(top) < 1700.0, "{}", volume(top));
        assert!(s.pieces.iter().all(|p| p.contacts.len() == 1 && p.contacts[0].1 > 100.0));
        // Where they meet, both sides find the faces to glue. The contact counts
        // voxel faces, a staircase; the meshes are smooth, so a bit smaller.
        let (a, b) = (&s.pieces[0], &s.pieces[1]);
        for (x, y) in [(a, b), (b, a)] {
            let area: f32 = crate::kit::contact_faces(x, y, s.voxel_mm * 0.5)
                .iter()
                .map(|&t| {
                    let c = x.tris[t].map(|v| x.positions[v as usize]);
                    0.5 * crate::geom::len(crate::geom::cross(
                        crate::geom::sub(c[1], c[0]),
                        crate::geom::sub(c[2], c[0]),
                    ))
                })
                .sum();
            let staircase = x.contacts[0].1;
            assert!(area > 0.7 * staircase && area < 1.05 * staircase, "{area} vs {staircase}");
        }
    }

    #[test]
    fn depth_limits_the_colour_and_leaves_a_core() {
        let s = painted_cube(SplitParams { voxel_mm: 1.0, depth_mm: 3.0, core_slot: Some(0), min_piece_mm3: 1.0 });
        let top = s.pieces.iter().find(|p| p.slot == 1).unwrap();
        // A 20 × 20 × 3 mm slab, minus the edges where the sides are nearer.
        assert!(volume(top) > 600.0 && volume(top) < 1300.0, "{}", volume(top));
        assert!(s.pieces.iter().all(|p| watertight(&p.tris)));
    }

    #[test]
    fn crumbs_join_their_neighbour() {
        let (p, t) = cuboid([0.0; 3], [20.0; 3]);
        let s = soup(&p, &t);
        // A dot of colour 1 near one corner of the top face.
        let dot = |tri: u32, b: Bary| {
            let q = geom::at(&s.corners[tri as usize], b);
            u8::from(q[2] > 19.9 && q[0] < 4.0 && q[1] < 4.0)
        };
        let kept = split(&s, dot, SplitParams { voxel_mm: 1.0, min_piece_mm3: 0.5, ..Default::default() });
        assert_eq!(kept.pieces.len(), 2);
        let merged = split(&s, dot, SplitParams { voxel_mm: 1.0, min_piece_mm3: 50.0, ..Default::default() });
        assert_eq!(merged.pieces.len(), 1);
        assert!(watertight(&merged.pieces[0].tris));
    }

    #[test]
    fn outer_surface_stays_on_the_model() {
        let s = painted_cube(SplitParams { voxel_mm: 0.7, ..Default::default() });
        for p in &s.pieces {
            for v in &p.positions {
                assert!(v.iter().all(|&c| (-0.01..=20.01).contains(&c)), "{v:?}");
            }
        }
    }

    #[test]
    fn faces_between_pieces_are_mirror_images() {
        let s = painted_cube(SplitParams { voxel_mm: 1.0, ..Default::default() });
        let (a, b) = (&s.pieces[0], &s.pieces[1]);
        let key = |p: Vec3| p.map(f32::to_bits);
        let mirrored: std::collections::HashSet<_> = b
            .tris
            .iter()
            .map(|t| {
                let [x, y, z] = t.map(|v| key(b.positions[v as usize]));
                // Every rotation of the triangle, wound the other way.
                [[x, z, y], [z, y, x], [y, x, z]]
            })
            .flatten()
            .collect();
        let contact = crate::kit::contact_faces(a, b, 0.25);
        let matched = contact
            .iter()
            .filter(|&&t| mirrored.contains(&a.tris[t].map(|v| key(a.positions[v as usize]))))
            .count();
        // All but the strip where both meet the outside.
        assert!(matched as f32 > 0.9 * contact.len() as f32, "{matched} of {}", contact.len());
    }

    #[test]
    fn colours_meet_on_the_surface_without_a_groove() {
        // The top quarter of a cube in another colour: the border runs round
        // the flat sides at z = 15.
        let (p, t) = cuboid([0.0; 3], [20.0; 3]);
        let model = soup(&p, &t);
        let band = |tri: u32, b: Bary| u8::from(geom::at(&model.corners[tri as usize], b)[2] > 15.0);
        let s = split(&model, band, SplitParams { voxel_mm: 1.0, ..Default::default() });
        assert_eq!(s.pieces.len(), 2);
        let mut all = TriSoup::default();
        for p in &s.pieces {
            for t in &p.tris {
                all.push(t.map(|v| p.positions[v as usize]), 0, 0);
            }
        }
        let bvh = Bvh::build(&all);
        // Across the border on every side, away from the corners the grid rounds off.
        for i in 4..37 {
            let x = i as f32 * 0.5;
            for z in [14.0, 14.5, 15.0, 15.5, 16.0] {
                for q in [[x, 0.0, z], [x, 20.0, z], [0.0, x, z], [20.0, x, z]] {
                    let (t, bb) = bvh.nearest(&all, q).unwrap();
                    let d = geom::len(geom::sub(geom::at(&all.corners[t as usize], bb), q));
                    assert!(d < 0.1, "{q:?} is {d} mm from the pieces");
                }
            }
        }
    }
}
