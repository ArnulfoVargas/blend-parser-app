//! Cutting a region painted on the surface out of a part. A selection on the
//! surface does not say where the volume under it ends, so the closing surface
//! is always chosen explicitly:
//!
//! - a shell: the patch pushed inward along the surface normals by a thickness
//!   and joined to it by walls round its border. The rest of the part gets the
//!   same inner surface and walls, facing the other way, as a pocket the shell
//!   fits into;
//! - a cap: each border loop closed by a fan to its centre, moved a chosen depth
//!   into the part. The rest gets the same fan, facing the other way.
//!
//! Either way the region and the rest share their closing faces, so both are
//! closed when the part was, and they do not overlap. Nothing is assumed: a
//! border that runs along a hole or pinches at a corner, a cap that folds or
//! crosses the surface, walls thinner than the shell and a shell that folds in
//! a tight curve are refused with what went wrong.

use std::collections::HashMap;

use crate::bvh::{Bvh, TriSoup};
use crate::geom::{self, Vec3};
use crate::scene::Part;
use crate::separate;

/// How the cut region is closed, in the part's units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Closure {
    /// The patch pushed inward `thickness` along the surface normals, joined to it by walls.
    /// The rest keeps a pocket of that size; the piece is `clearance` smaller than the pocket
    /// round its border and under it, so the two fit together after printing.
    Shell { thickness: f32, clearance: f32 },
    /// Each border loop closed with a fan to its centre, moved `depth` into the part
    /// along the region's mean normal (negative bulges it outwards).
    Cap { depth: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionError {
    /// Nothing is selected.
    Empty,
    /// The border meets an edge shared by more than two faces: where the region ends is ambiguous.
    AmbiguousBorder,
    /// The border touches itself at a vertex, or two regions meet at one.
    PinchedBorder,
    /// A cap would fold over itself or cross the part's surface.
    AmbiguousCap,
    /// The part is thinner than the shell under `count` vertices of the region.
    ThinWall { count: usize },
    /// The shell's inner surface turns over at `count` faces: the surface curves tighter than the thickness.
    Folded { count: usize },
    /// The clearance is not less than the thickness, or pulling the border in by it folds `count` faces.
    Clearance { count: usize },
    /// The pieces would take more room than the part had: they overlap somewhere.
    Overlap,
}

/// A result part and, for each of its triangles, the triangle of the input it
/// lies on: the face it is, or for a shell's inner face the face it was pushed
/// in from. Closing walls and caps have none.
#[derive(Clone, Debug, Default)]
pub struct Piece {
    pub part: Part,
    pub origins: Vec<Option<u32>>,
    /// Every edge is used once each way: a closed, consistently wound surface.
    pub closed: bool,
    /// Enclosed volume, in the part's units cubed.
    pub volume: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Split {
    /// What is not selected, closed where the regions came off. No triangles when all was selected.
    pub rest: Piece,
    /// One per connected selected region, largest first.
    pub regions: Vec<Piece>,
    /// The faces added to close the rest, for the preview.
    pub closure: Vec<[Vec3; 3]>,
    /// Mean outward normal of each region's surface, the way it would come off.
    pub directions: Vec<Vec3>,
}

/// A triangle being built: vertices, face corners and material slot.
type Tri = ([u32; 3], [u32; 3], u16);

/// Splits `part` into the triangles marked in `selected` and the rest, each
/// closed with `closure`. Corners at the same position count as one.
pub fn split(part: &Part, selected: &[bool], closure: Closure) -> Result<Split, RegionError> {
    let n = part.tris.len();
    let is_selected = |t: usize| selected.get(t).copied().unwrap_or(false);
    if !(0..n).any(is_selected) {
        return Err(RegionError::Empty);
    }
    // Welded: a seam in the UVs must not look like a border.
    let ids = separate::weld_ids(&part.positions, 0.0);
    let tris: Vec<[u32; 3]> = part.tris.iter().map(|t| t.map(|v| ids[v as usize])).collect();
    let mut edges: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            edges.entry((t[k], t[(k + 1) % 3])).or_default().push(ti);
        }
    }
    let twin = |a: u32, b: u32| -> Result<Option<usize>, RegionError> {
        let fwd = edges.get(&(a, b)).map_or(0, Vec::len);
        match edges.get(&(b, a)).map(Vec::as_slice) {
            // A hole of the part: it stays as open as it was.
            None if fwd == 1 => Ok(None),
            Some([o]) if fwd == 1 => Ok(Some(*o)),
            _ => Err(RegionError::AmbiguousBorder),
        }
    };

    // Regions: selected triangles joined across their shared edges.
    let mut region = vec![usize::MAX; n];
    let mut count = 0;
    let mut border: Vec<Vec<(u32, u32, usize, usize)>> = vec![];
    for start in 0..n {
        if !is_selected(start) || region[start] != usize::MAX {
            continue;
        }
        region[start] = count;
        border.push(vec![]);
        let mut stack = vec![start];
        while let Some(ti) = stack.pop() {
            let t = tris[ti];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                match twin(a, b)? {
                    Some(o) if is_selected(o) => {
                        if region[o] == usize::MAX {
                            region[o] = count;
                            stack.push(o);
                        }
                    }
                    Some(_) => border[count].push((a, b, ti, k)),
                    None => {}
                }
            }
        }
        count += 1;
    }
    // Each border vertex starts exactly one border edge, or the border is not a set of simple loops.
    let mut starts: HashMap<u32, usize> = HashMap::new();
    for &(a, ..) in border.iter().flatten() {
        *starts.entry(a).or_default() += 1;
    }
    if starts.values().any(|&c| c > 1) {
        return Err(RegionError::PinchedBorder);
    }

    let mut positions = part.positions.clone();
    let corner = |ti: usize, k: usize| part.tri_loops[ti][k];
    let source = |ti: usize| part.tri_source.get(ti).copied().unwrap_or(0);
    let mut pieces: Vec<Vec<(Tri, Option<u32>)>> = (0..count).map(|_| vec![]).collect();
    let mut rest: Vec<(Tri, Option<u32>)> = vec![];
    for (ti, t) in tris.iter().enumerate() {
        let tri = (*t, part.tri_loops[ti], source(ti));
        if is_selected(ti) {
            pieces[region[ti]].push((tri, Some(ti as u32)));
        } else {
            rest.push((tri, Some(ti as u32)));
        }
    }
    let face_normal = |c: [Vec3; 3]| geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
    let corners = |t: [u32; 3], positions: &[Vec3]| t.map(|v| positions[v as usize]);
    // Area-weighted mean normal of each region, the way it comes off.
    let mut directions = vec![[0.0f32; 3]; count];
    for (ti, t) in tris.iter().enumerate() {
        if is_selected(ti) {
            directions[region[ti]] = geom::add(directions[region[ti]], face_normal(corners(*t, &positions)));
        }
    }
    let directions: Vec<Vec3> = directions.into_iter().map(geom::normalize).collect();
    let mut closing: Vec<[u32; 3]> = vec![];

    match closure {
        Closure::Shell { thickness, clearance } => {
            if clearance < 0.0 || clearance >= thickness {
                return Err(RegionError::Clearance { count: 0 });
            }
            // Vertex normals over the whole part, so the inner surface is the same from both sides.
            let mut normal = vec![[0.0f32; 3]; positions.len()];
            for t in &tris {
                let f = face_normal(corners(*t, &positions));
                for &v in t {
                    normal[v as usize] = geom::add(normal[v as usize], f);
                }
            }
            let normal: Vec<Vec3> = normal.into_iter().map(geom::normalize).collect();
            let mut soup = TriSoup::default();
            for (ti, t) in tris.iter().enumerate() {
                soup.push(corners(*t, &positions), 0, ti as u32);
            }
            let bvh = Bvh::build(&soup);
            let mut inner: HashMap<u32, u32> = HashMap::new();
            let mut thin = 0;
            for (ti, t) in tris.iter().enumerate() {
                if !is_selected(ti) {
                    continue;
                }
                for &v in t {
                    if inner.contains_key(&v) {
                        continue;
                    }
                    let p = positions[v as usize];
                    let down = geom::scale(normal[v as usize], -1.0);
                    // Whatever surface the inward ray meets first must lie past the shell.
                    let from = geom::add(p, geom::scale(down, thickness * 1e-3));
                    if bvh.raycast(&soup, from, down).is_some_and(|h| h.t < thickness * 1.02) {
                        thin += 1;
                    }
                    inner.insert(v, positions.len() as u32);
                    positions.push(geom::add(p, geom::scale(down, thickness)));
                }
            }
            if thin > 0 {
                return Err(RegionError::ThinWall { count: thin });
            }
            // With a clearance the piece has vertices of its own: its border pulled
            // in along the surface and its inner face that much short of the pocket's.
            let mut own: HashMap<u32, u32> = HashMap::new();
            let mut own_inner: HashMap<u32, u32> = HashMap::new();
            if clearance > 0.0 {
                let mut inward: HashMap<u32, Vec<Vec3>> = HashMap::new();
                for &(a, b, ..) in border.iter().flatten() {
                    let n = geom::normalize(geom::add(normal[a as usize], normal[b as usize]));
                    let left = geom::normalize(geom::cross(n, geom::sub(positions[b as usize], positions[a as usize])));
                    inward.entry(a).or_default().push(left);
                    inward.entry(b).or_default().push(left);
                }
                for (&v, dirs) in &inward {
                    let d = geom::normalize(dirs.iter().fold([0.0; 3], |s, &x| geom::add(s, x)));
                    // A corner's two edges both move back by the clearance.
                    let reach = clearance / dirs.iter().map(|&x| geom::dot(d, x)).fold(1.0f32, f32::min).max(0.3);
                    own.insert(v, positions.len() as u32);
                    positions.push(geom::add(positions[v as usize], geom::scale(d, reach)));
                }
                for v in inner.keys().copied().collect::<Vec<_>>() {
                    let from = own.get(&v).copied().unwrap_or(v);
                    let down = geom::scale(normal[v as usize], -(thickness - clearance));
                    own_inner.insert(v, positions.len() as u32);
                    positions.push(geom::add(positions[from as usize], down));
                }
            }
            let outer_of = |v: u32| own.get(&v).copied().unwrap_or(v);
            let inner_of = |v: u32| own_inner.get(&v).copied().unwrap_or(inner[&v]);
            let mut pulled = 0;
            for list in &mut pieces {
                for ((t, ..), _) in list.iter_mut() {
                    let moved = t.map(outer_of);
                    if moved != *t
                        && geom::dot(face_normal(corners(moved, &positions)), face_normal(corners(*t, &positions)))
                            <= 0.0
                    {
                        pulled += 1;
                    }
                    *t = moved;
                }
            }
            if pulled > 0 {
                return Err(RegionError::Clearance { count: pulled });
            }
            let mut folded = 0;
            for (ti, t) in tris.iter().enumerate() {
                if !is_selected(ti) {
                    continue;
                }
                let i = t.map(|v| inner[&v]);
                let pi = t.map(inner_of);
                let outer = face_normal(corners(*t, &positions));
                for f in [i, pi] {
                    if geom::dot(face_normal(corners(f, &positions)), outer) <= 0.0 {
                        folded += 1;
                    }
                }
                let l = part.tri_loops[ti];
                // The region's inner face looks into the pocket; the rest's looks out of it.
                pieces[region[ti]].push((([pi[2], pi[1], pi[0]], [l[2], l[1], l[0]], source(ti)), Some(ti as u32)));
                rest.push(((i, l, source(ti)), Some(ti as u32)));
                closing.push(i);
            }
            if folded > 0 {
                return Err(RegionError::Folded { count: folded });
            }
            for (r, edges) in border.iter().enumerate() {
                for &(a, b, ti, k) in edges {
                    let (la, lb) = (corner(ti, k), corner(ti, (k + 1) % 3));
                    let src = source(ti);
                    let walls =
                        |a: u32, b: u32, ia: u32, ib: u32| [([b, a, ia], [lb, la, la]), ([b, ia, ib], [lb, la, lb])];
                    for (t, l) in walls(outer_of(a), outer_of(b), inner_of(a), inner_of(b)) {
                        pieces[r].push(((t, l, src), None));
                    }
                    for (t, l) in walls(a, b, inner[&a], inner[&b]) {
                        let back = [t[2], t[1], t[0]];
                        rest.push(((back, [l[2], l[1], l[0]], src), None));
                        closing.push(back);
                    }
                }
            }
        }
        Closure::Cap { depth } => {
            let mut soup = TriSoup::default();
            for (ti, t) in tris.iter().enumerate() {
                soup.push(corners(*t, &positions), 0, ti as u32);
            }
            let bvh = Bvh::build(&soup);
            for (r, edges) in border.iter().enumerate() {
                let next: HashMap<u32, (u32, usize, usize)> =
                    edges.iter().map(|&(a, b, ti, k)| (a, (b, ti, k))).collect();
                let mut done: HashMap<u32, bool> = HashMap::new();
                let mut starts: Vec<u32> = next.keys().copied().collect();
                starts.sort_unstable();
                for start in starts {
                    if done.contains_key(&start) {
                        continue;
                    }
                    let mut ring = vec![];
                    let mut at = start;
                    loop {
                        done.insert(at, true);
                        let Some(&(to, ti, k)) = next.get(&at) else { return Err(RegionError::AmbiguousBorder) };
                        ring.push((at, to, ti, k));
                        at = to;
                        if at == start {
                            break;
                        }
                        if done.contains_key(&at) {
                            return Err(RegionError::PinchedBorder);
                        }
                    }
                    let centroid = ring.iter().fold([0.0; 3], |c, &(a, ..)| geom::add(c, positions[a as usize]));
                    let centroid = geom::scale(centroid, 1.0 / ring.len() as f32);
                    let hub = geom::sub(centroid, geom::scale(directions[r], depth));
                    let h = positions.len() as u32;
                    positions.push(hub);
                    // The region's cap runs its border backwards, as the face across it did.
                    let fan: Vec<[u32; 3]> = ring.iter().map(|&(a, b, ..)| [b, a, h]).collect();
                    let normals: Vec<Vec3> = fan.iter().map(|t| face_normal(corners(*t, &positions))).collect();
                    let sum = normals.iter().fold([0.0; 3], |s, &n| geom::add(s, n));
                    let scale = geom::len(sum).max(f32::MIN_POSITIVE);
                    if normals.iter().any(|&n| geom::dot(n, sum) <= 1e-6 * scale * geom::len(n)) {
                        return Err(RegionError::AmbiguousCap);
                    }
                    // Each spoke from the hub to the border must stay inside the part.
                    for &(a, ..) in &ring {
                        let d = geom::sub(positions[a as usize], hub);
                        let length = geom::len(d);
                        if length <= 0.0 {
                            return Err(RegionError::AmbiguousCap);
                        }
                        if bvh
                            .raycast(&soup, hub, geom::scale(d, 1.0 / length))
                            .is_some_and(|hit| hit.t < length * 0.98)
                        {
                            return Err(RegionError::AmbiguousCap);
                        }
                    }
                    for (&(a, b, ti, k), t) in ring.iter().zip(&fan) {
                        // One colour for the cap face: the surface's where the border runs.
                        let l = [corner(ti, k); 3];
                        let src = source(ti);
                        pieces[r].push(((*t, l, src), None));
                        let back = [a, b, h];
                        rest.push(((back, l, src), None));
                        closing.push(back);
                    }
                }
            }
        }
    }

    let build = |tris: Vec<(Tri, Option<u32>)>| piece(part, &positions, tris);
    let mut regions: Vec<(Piece, Vec3)> = pieces.into_iter().map(build).zip(directions).collect();
    let rest = build(rest);
    // Closed pieces that fit together never add up to more than the part.
    let whole = volume(part);
    let taken: f32 = regions.iter().map(|(p, _)| p.volume).sum::<f32>() + rest.volume;
    if regions.iter().all(|(p, _)| p.closed) && rest.closed && taken > whole + whole.abs() * 1e-4 + 1e-6 {
        return Err(RegionError::Overlap);
    }
    regions.sort_by_key(|(p, _)| std::cmp::Reverse(p.part.tris.len()));
    let (regions, directions) = regions.into_iter().unzip();
    Ok(Split { rest, regions, closure: closing.into_iter().map(|t| corners(t, &positions)).collect(), directions })
}

/// The triangles as a part of their own: only the vertices they use, the input's materials and corners.
fn piece(part: &Part, positions: &[Vec3], tris: Vec<(Tri, Option<u32>)>) -> Piece {
    let mut remap = vec![u32::MAX; positions.len()];
    let mut out = Part {
        name: part.name.clone(),
        sources: part.sources.clone(),
        uv_layers: part.uv_layers.clone(),
        color_layers: part.color_layers.clone(),
        object: part.object,
        ..Default::default()
    };
    let mut origins = Vec::with_capacity(tris.len());
    for ((t, l, src), origin) in tris {
        out.tris.push(t.map(|v| {
            if remap[v as usize] == u32::MAX {
                remap[v as usize] = out.positions.len() as u32;
                out.positions.push(positions[v as usize]);
            }
            remap[v as usize]
        }));
        out.tri_loops.push(l);
        out.tri_source.push(src);
        origins.push(origin);
    }
    let all: Vec<u32> = (0..out.tris.len() as u32).collect();
    let closed = !all.is_empty() && separate::closed(&out.positions, &out.tris, &all, 0.0);
    let volume = volume(&out);
    Piece { part: out, origins, closed, volume }
}

/// Signed enclosed volume, taken about the first vertex to keep it precise far from the origin.
pub fn volume(part: &Part) -> f32 {
    let Some(&o) = part.positions.first() else { return 0.0 };
    let v: f64 = (0..part.tris.len())
        .map(|i| {
            let [a, b, c] = part.corners(i).map(|p| geom::sub(p, o));
            geom::dot(a, geom::cross(b, c)) as f64 / 6.0
        })
        .sum();
    v as f32
}

/// Triangles of `part` joined to `seed` across shared edges or corners, as one connected piece.
pub fn connected(part: &Part, seeds: &[bool]) -> Vec<bool> {
    let tris: Vec<[u32; 3]> = {
        let ids = separate::weld_ids(&part.positions, 0.0);
        part.tris.iter().map(|t| t.map(|v| ids[v as usize])).collect()
    };
    let mut by_vertex: HashMap<u32, Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for &v in t {
            by_vertex.entry(v).or_default().push(ti);
        }
    }
    let mut out: Vec<bool> = (0..tris.len()).map(|t| seeds.get(t).copied().unwrap_or(false)).collect();
    let mut stack: Vec<usize> = (0..tris.len()).filter(|&t| out[t]).collect();
    while let Some(ti) = stack.pop() {
        for v in tris[ti] {
            for &o in &by_vertex[&v] {
                if !out[o] {
                    out[o] = true;
                    stack.push(o);
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::scene::ColorSource;

    /// A closed box made of `n`×`n` quads per face, welded, faces outwards.
    pub(crate) fn grid_box(lo: Vec3, hi: Vec3, n: usize) -> Part {
        let mut positions: Vec<Vec3> = vec![];
        let mut tris = vec![];
        let mut index: HashMap<[i64; 3], u32> = HashMap::new();
        let mut vertex = |p: Vec3, positions: &mut Vec<Vec3>| {
            *index.entry(p.map(|v| (v * 1e4).round() as i64)).or_insert_with(|| {
                positions.push(p);
                positions.len() as u32 - 1
            })
        };
        // Each face: an axis, a side, and the two axes across it in right-handed order.
        for axis in 0..3 {
            for side in [0, 1] {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let (u, v) = if side == 1 { (u, v) } else { (v, u) };
                for i in 0..n {
                    for j in 0..n {
                        let at = |di: usize, dj: usize| {
                            let mut p = [0.0; 3];
                            p[axis] = if side == 1 { hi[axis] } else { lo[axis] };
                            p[u] = lo[u] + (hi[u] - lo[u]) * (i + di) as f32 / n as f32;
                            p[v] = lo[v] + (hi[v] - lo[v]) * (j + dj) as f32 / n as f32;
                            p
                        };
                        let q = [at(0, 0), at(1, 0), at(1, 1), at(0, 1)].map(|p| vertex(p, &mut positions));
                        tris.push([q[0], q[1], q[2]]);
                        tris.push([q[0], q[2], q[3]]);
                    }
                }
            }
        }
        let count = tris.len() as u32;
        Part {
            name: "box".into(),
            positions,
            tri_loops: (0..count).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: vec![0; tris.len()],
            tris,
            sources: vec![ColorSource::Flat([0.5, 0.5, 0.5])],
            uv_layers: vec![vec![[0.5, 0.5]; count as usize * 3]],
            ..Default::default()
        }
    }

    /// The triangles of `part` whose centre lies on the top face, inside the square `lo`..`hi` on X and Y.
    fn top_patch(part: &Part, top: f32, lo: f32, hi: f32) -> Vec<bool> {
        (0..part.tris.len())
            .map(|i| {
                let c = part.corners(i);
                let m = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
                c.iter().all(|p| (p[2] - top).abs() < 1e-5) && (lo..=hi).contains(&m[0]) && (lo..=hi).contains(&m[1])
            })
            .collect()
    }

    #[test]
    fn a_shell_comes_off_and_leaves_a_pocket_it_fits() {
        let part = grid_box([0.0; 3], [10.0; 3], 5);
        let selected = top_patch(&part, 10.0, 2.0, 8.0);
        let s = split(&part, &selected, Closure::Shell { thickness: 1.0, clearance: 0.0 }).unwrap();
        assert_eq!(s.regions.len(), 1);
        let shell = &s.regions[0];
        assert!(shell.closed && s.rest.closed);
        // A 6 × 6 patch, 1 deep: its volume, and the rest is the box less it.
        assert!((shell.volume - 36.0).abs() < 1e-3, "{}", shell.volume);
        assert!((s.rest.volume - (1000.0 - 36.0)).abs() < 1e-2, "{}", s.rest.volume);
        assert!((s.directions[0][2] - 1.0).abs() < 1e-6);
        // The outer faces still know their triangle, the inner ones the face they came from.
        assert_eq!(shell.origins.iter().filter(|o| o.is_some()).count(), 2 * selected.iter().filter(|&&s| s).count());
        assert!(!s.closure.is_empty());
    }

    #[test]
    fn a_clearance_makes_the_shell_smaller_than_its_pocket() {
        let part = grid_box([0.0; 3], [10.0; 3], 5);
        let selected = top_patch(&part, 10.0, 2.0, 8.0);
        let s = split(&part, &selected, Closure::Shell { thickness: 1.0, clearance: 0.5 }).unwrap();
        let shell = &s.regions[0];
        assert!(shell.closed && s.rest.closed);
        // The pocket is still 6 × 6 × 1; the shell is 0.5 short of it all round and under it.
        assert!((s.rest.volume - (1000.0 - 36.0)).abs() < 1e-2, "{}", s.rest.volume);
        assert!((shell.volume - 5.0 * 5.0 * 0.5).abs() < 1e-2, "{}", shell.volume);
        let (lo, hi) = shell.part.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
            (std::array::from_fn(|i| lo[i].min(v[i])), std::array::from_fn(|i| hi[i].max(v[i])))
        });
        assert!(
            (lo[0] - 2.5).abs() < 1e-4 && (hi[0] - 7.5).abs() < 1e-4 && (lo[2] - 9.5).abs() < 1e-4,
            "{lo:?} {hi:?}"
        );

        // No clearance can be as thick as the shell, nor pull the border past the faces inside it.
        let too_thick = split(&part, &selected, Closure::Shell { thickness: 1.0, clearance: 1.0 });
        assert_eq!(too_thick.unwrap_err(), RegionError::Clearance { count: 0 });
        let too_wide = split(&part, &selected, Closure::Shell { thickness: 4.0, clearance: 3.0 });
        assert!(matches!(too_wide, Err(RegionError::Clearance { count }) if count > 0), "{too_wide:?}");
    }

    #[test]
    fn a_cap_closes_both_sides_with_the_same_fan() {
        let part = grid_box([0.0; 3], [10.0; 3], 5);
        let selected = top_patch(&part, 10.0, 2.0, 8.0);
        let flat = split(&part, &selected, Closure::Cap { depth: 0.0 }).unwrap();
        // A flat cap under a flat patch encloses nothing; still both sides are closed.
        assert!(flat.regions[0].closed && flat.rest.closed);
        assert!(flat.regions[0].volume.abs() < 1e-3);
        // Pushed 3 deep it is a pyramid on a 6 × 6 base: 36.
        let deep = split(&part, &selected, Closure::Cap { depth: 3.0 }).unwrap();
        assert!((deep.regions[0].volume - 36.0).abs() < 1e-2, "{}", deep.regions[0].volume);
        assert!((deep.rest.volume + deep.regions[0].volume - 1000.0).abs() < 1e-2);
        // Deeper than the box: the spokes leave the part.
        assert_eq!(split(&part, &selected, Closure::Cap { depth: 30.0 }).unwrap_err(), RegionError::AmbiguousCap);
    }

    #[test]
    fn a_shell_thicker_than_the_wall_or_a_pinched_border_is_refused() {
        let part = grid_box([0.0, 0.0, 0.0], [10.0, 10.0, 2.0], 5);
        let selected = top_patch(&part, 2.0, 2.0, 8.0);
        assert!(matches!(
            split(&part, &selected, Closure::Shell { thickness: 3.0, clearance: 0.0 }),
            Err(RegionError::ThinWall { .. })
        ));
        assert!(split(&part, &selected, Closure::Shell { thickness: 1.0, clearance: 0.0 }).is_ok());
        assert_eq!(
            split(&part, &vec![false; part.tris.len()], Closure::Shell { thickness: 1.0, clearance: 0.0 }).unwrap_err(),
            RegionError::Empty
        );

        // Two squares meeting at one corner pinch the border there.
        let part = grid_box([0.0; 3], [10.0; 3], 5);
        let a = top_patch(&part, 10.0, 2.0, 4.0);
        let b = top_patch(&part, 10.0, 4.0, 6.0);
        let both: Vec<bool> = a.iter().zip(&b).map(|(x, y)| *x || *y).collect();
        let r = split(&part, &both, Closure::Shell { thickness: 1.0, clearance: 0.0 });
        assert_eq!(r.unwrap_err(), RegionError::PinchedBorder);
    }

    #[test]
    fn separate_regions_come_off_as_separate_pieces_and_a_whole_box_needs_no_closing() {
        let part = grid_box([0.0; 3], [10.0; 3], 5);
        let a = top_patch(&part, 10.0, 0.0, 4.0);
        let b = top_patch(&part, 10.0, 6.0, 10.0);
        let both: Vec<bool> = a.iter().zip(&b).map(|(x, y)| *x || *y).collect();
        let s = split(&part, &both, Closure::Shell { thickness: 1.0, clearance: 0.0 }).unwrap();
        assert_eq!(s.regions.len(), 2);
        assert!(s.regions.iter().all(|r| r.closed) && s.rest.closed);

        let all = vec![true; part.tris.len()];
        let s = split(&part, &all, Closure::Cap { depth: 0.0 }).unwrap();
        assert!(s.rest.part.tris.is_empty() && s.closure.is_empty());
        assert!(s.regions[0].closed && (s.regions[0].volume - 1000.0).abs() < 1e-2);
    }

    #[test]
    fn a_hole_in_the_part_stays_open_and_is_reported() {
        let mut part = grid_box([0.0; 3], [10.0; 3], 5);
        // Take one face off the bottom: the box is open.
        let bottom = (0..part.tris.len()).find(|&i| part.corners(i).iter().all(|p| p[2] == 0.0)).unwrap();
        part.tris.remove(bottom);
        part.tri_loops.remove(bottom);
        part.tri_source.remove(bottom);
        let selected = top_patch(&part, 10.0, 2.0, 8.0);
        let s = split(&part, &selected, Closure::Shell { thickness: 1.0, clearance: 0.0 }).unwrap();
        assert!(s.regions[0].closed);
        assert!(!s.rest.closed);
    }

    #[test]
    fn connected_fills_the_piece_a_face_is_on() {
        let mut part = grid_box([0.0; 3], [1.0; 3], 1);
        let other = grid_box([3.0; 3], [4.0; 3], 1);
        let base = part.positions.len() as u32;
        part.positions.extend(&other.positions);
        part.tris.extend(other.tris.iter().map(|t| t.map(|v| v + base)));
        let mut seed = vec![false; part.tris.len()];
        seed[0] = true;
        let got = connected(&part, &seed);
        assert_eq!(got.iter().filter(|&&s| s).count(), 12);
        assert!(got[..12].iter().all(|&s| s));
    }
}
