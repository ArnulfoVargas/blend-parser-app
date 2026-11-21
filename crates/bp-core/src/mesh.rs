//! The working geometry that gets painted and exported, derived from the source
//! scene by the mesh operations: simplification and base flattening.

use std::collections::HashMap;

use crate::bvh::TriSoup;
use crate::geom::{self, BaryTri, Vec3, ROOT};
use crate::scene::Scene;

#[derive(Clone, Debug, Default)]
pub struct MeshPart {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// For each triangle, the source triangle it lies in and its corners in that
    /// triangle's barycentrics. `None` once simplification has broken the link.
    pub origin: Option<Vec<(u32, BaryTri)>>,
}

impl MeshPart {
    #[inline]
    pub fn corners(&self, tri: usize) -> [Vec3; 3] {
        let t = self.tris[tri];
        [self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]]
    }

    /// Vertices where the surface bends: some face around them turns more
    /// than `deg` from their normal. Eyes, fingers and reliefs are made of them.
    pub fn bent_vertices(&self, deg: f32) -> Vec<bool> {
        let normals = self.vertex_normals();
        let limit = deg.to_radians().cos();
        let mut bent = vec![false; self.positions.len()];
        for (ti, t) in self.tris.iter().enumerate() {
            let c = self.corners(ti);
            let face = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
            if geom::len(face) == 0.0 {
                continue;
            }
            let face = geom::normalize(face);
            for &v in t {
                if geom::dot(face, normals[v as usize]) < limit {
                    bent[v as usize] = true;
                }
            }
        }
        bent
    }

    /// Unit normal of each vertex: the area-weighted sum of its faces'.
    pub fn vertex_normals(&self) -> Vec<Vec3> {
        let mut n = vec![[0.0f32; 3]; self.positions.len()];
        for (ti, t) in self.tris.iter().enumerate() {
            let c = self.corners(ti);
            let face = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
            for &v in t {
                n[v as usize] = geom::add(n[v as usize], face);
            }
        }
        n.into_iter().map(|v| if geom::len(v) > 0.0 { geom::normalize(v) } else { v }).collect()
    }

    /// Keeps the triangles marked in `keep` (and their origins), then drops unused vertices.
    pub fn retain(&mut self, keep: &[bool]) {
        let mut it = keep.iter();
        self.tris.retain(|_| *it.next().unwrap_or(&true));
        if let Some(o) = &mut self.origin {
            let mut it = keep.iter();
            o.retain(|_| *it.next().unwrap_or(&true));
        }
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::with_capacity(self.positions.len());
        for t in &mut self.tris {
            for v in t.iter_mut() {
                if remap[*v as usize] == u32::MAX {
                    remap[*v as usize] = positions.len() as u32;
                    positions.push(self.positions[*v as usize]);
                }
                *v = remap[*v as usize];
            }
        }
        self.positions = positions;
    }

    /// Connected piece of each triangle (triangles sharing a vertex are connected).
    pub fn components(&self) -> Vec<u32> {
        fn find(p: &mut [u32], mut x: u32) -> u32 {
            while p[x as usize] != x {
                p[x as usize] = p[p[x as usize] as usize];
                x = p[x as usize];
            }
            x
        }
        let mut parent: Vec<u32> = (0..self.positions.len() as u32).collect();
        for t in &self.tris {
            let a = find(&mut parent, t[0]);
            for &v in &t[1..] {
                let b = find(&mut parent, v);
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
        let mut ids: HashMap<u32, u32> = HashMap::new();
        self.tris
            .iter()
            .map(|t| {
                let r = find(&mut parent, t[0]);
                let n = ids.len() as u32;
                *ids.entry(r).or_insert(n)
            })
            .collect()
    }
}

/// Geometry operations, in final millimetres (after the user's scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshOps {
    /// Fraction of triangles to keep, 1.0 keeps the mesh as is.
    pub keep_ratio: f32,
    /// Height of the slice cut flat against the plate, 0.0 leaves the base alone.
    pub flatten_mm: f32,
    /// Weld duplicate vertices, drop degenerate and repeated triangles.
    pub repair: bool,
    /// Loose pieces whose bounding box diagonal is under this are removed; 0 keeps all.
    pub min_piece_mm: f32,
    /// Make every piece's faces agree on their winding and point outwards.
    pub orient: bool,
    /// Close holes with a flat fan that takes the colour of the face beside it.
    pub fill_holes: bool,
    /// Give a vertex shared by separate fans of faces one copy per fan.
    pub split_vertices: bool,
    /// Lower every piece that floats above the plate until it touches it.
    pub drop_floating: bool,
    /// Weld and drop slivers at the printability tolerance, and remove stray
    /// faces hanging off the surface by one edge.
    pub deep_clean: bool,
    /// Simplifying keeps the vertices where the colour changes, so borders stay sharp.
    pub protect_colors: bool,
    /// Simplifying weighs the surface's bend, so curved and small details keep more triangles.
    pub protect_detail: bool,
    /// Levels of Loop subdivision (each makes every triangle four), up to [`MAX_SUBDIVIDE`].
    pub subdivide: u8,
    /// Rounds of Taubin smoothing, which do not shrink the model.
    pub smooth: u8,
    /// Faces meeting at more than this many degrees keep a sharp edge when subdividing or smoothing.
    pub crease_deg: f32,
    /// Wall thickness in final millimetres to hollow the model to; 0 leaves it solid.
    pub hollow_mm: f32,
    /// Drain holes through the base into the cavity, and their diameter in final millimetres.
    pub drains: u8,
    pub drain_mm: f32,
    /// Walls and details thinner than this (final millimetres) are pushed out
    /// along their normals until they reach it; 0 leaves them.
    pub thicken_mm: f32,
}

/// What thickening moved, in source units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThickenReport {
    pub moved_vertices: usize,
    /// Furthest a vertex went out, and the area of the faces that moved.
    pub max_offset: f32,
    pub area: f32,
}

/// Drain holes the panel offers at most.
pub const MAX_DRAINS: u8 = 4;

/// What hollowing did, in source units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HollowReport {
    /// Volume before and after.
    pub solid_volume: f32,
    pub volume: f32,
    /// Voxel size the wall was found on: its accuracy.
    pub cell: f32,
    pub drains: u8,
    pub drains_wanted: u8,
    /// Why it was not hollowed, if it was not.
    pub refused: Option<crate::hollow::Refusal>,
}

/// Subdivision levels the panel offers.
pub const MAX_SUBDIVIDE: u8 = 2;
/// Triangles subdividing never goes past, whatever the panel's cap says.
pub const SUBDIVIDE_LIMIT: usize = 8_000_000;

impl Default for MeshOps {
    fn default() -> Self {
        Self {
            keep_ratio: 1.0,
            flatten_mm: 0.0,
            repair: true,
            min_piece_mm: 0.0,
            orient: false,
            fill_holes: false,
            split_vertices: false,
            drop_floating: false,
            deep_clean: false,
            protect_colors: false,
            protect_detail: false,
            subdivide: 0,
            smooth: 0,
            crease_deg: 30.0,
            hollow_mm: 0.0,
            drains: 0,
            drain_mm: 3.0,
            thicken_mm: 0.0,
        }
    }
}

/// What the clean-up changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    pub welded_vertices: usize,
    pub degenerate_triangles: usize,
    pub duplicate_triangles: usize,
    pub loose_pieces: usize,
    pub loose_triangles: usize,
    pub filled_holes: usize,
    pub flipped_triangles: usize,
    pub split_vertices: usize,
    pub dropped_pieces: usize,
    pub fin_triangles: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub parts: Vec<MeshPart>,
    pub repair: RepairReport,
    /// Farthest any vertex of the mesh before simplifying lies from the simplified
    /// surface, in source units; 0 when it was not simplified.
    pub deviation: f32,
    /// What hollowing did, when it was asked for.
    pub hollow: Option<HollowReport>,
    /// What thickening moved, when it was asked for.
    pub thicken: Option<ThickenReport>,
}

/// The filament slot of each triangle of each part, for [`Mesh::build_with`]
/// to keep colour borders; empty when there is no palette yet.
pub type Labeller<'a> = &'a dyn Fn(&Mesh) -> Vec<Vec<u8>>;

impl Mesh {
    pub fn from_scene(scene: &Scene) -> Self {
        let parts = scene
            .parts
            .iter()
            .map(|p| MeshPart {
                name: p.name.clone(),
                positions: p.positions.clone(),
                tris: p.tris.clone(),
                origin: Some((0..p.tris.len() as u32).map(|i| (i, ROOT)).collect()),
            })
            .collect();
        Self { parts, repair: RepairReport::default(), deviation: 0.0, hollow: None, thicken: None }
    }

    /// Builds the working mesh. Returns the shape before flattening (used to look
    /// up colours by position) and the final shape; both have the same triangles.
    /// `scale` converts source millimetres to final millimetres.
    pub fn build(scene: &Scene, ops: MeshOps, scale: f32) -> (Self, Self) {
        Self::build_with(scene, ops, scale, &|_| vec![])
    }

    /// [`Self::build`], with `labels` giving the colour of each triangle when
    /// simplifying has to keep colour borders.
    pub fn build_with(scene: &Scene, ops: MeshOps, scale: f32, labels: Labeller) -> (Self, Self) {
        let mut shape = Self::from_scene(scene);
        // Holes, winding and fans are read from shared vertices: on an unwelded
        // mesh every seam would look like a hole. Reverting the cleanup alone
        // must not leave them working on loose triangles.
        let rounds = ops.subdivide > 0 || ops.smooth > 0 || ops.thicken_mm > 0.0;
        if ops.repair || ops.deep_clean || ops.fill_holes || ops.orient || ops.split_vertices || rounds {
            shape.repair_all();
        }
        // Before holes: a stray face's open sides are not a hole to cap.
        if ops.deep_clean {
            shape.deep_clean(CLEAN_MM / scale);
        }
        // Holes first, so their caps get oriented with the rest of the piece.
        if ops.fill_holes {
            shape.fill_holes();
        }
        if ops.orient {
            shape.orient();
        }
        if ops.split_vertices {
            // Far enough apart that the printability weld does not join them again.
            shape.split_vertices(0.02 / scale);
        }
        if ops.min_piece_mm > 0.0 {
            shape.remove_loose_pieces(ops.min_piece_mm / scale);
        }
        if ops.keep_ratio < 0.999 {
            let before = shape.clone();
            let locks = ops.protect_colors.then(|| shape.border_vertices(&labels(&shape)));
            shape.simplify_with(ops.keep_ratio, locks.as_deref(), ops.protect_detail);
            shape.deviation = before.deviation_to(&shape);
        }
        if ops.subdivide > 0 || ops.smooth > 0 {
            shape.round_off(ops, &labels(&shape));
        }
        if ops.flatten_mm > 0.0 {
            shape.split_at(shape.bounds().map_or(0.0, |(lo, _)| lo[2]) + ops.flatten_mm / scale);
        }
        let mut work = shape.clone();
        if ops.flatten_mm > 0.0 {
            work.flatten(ops.flatten_mm / scale);
        }
        // Only the final shape moves: colours are still looked up where pieces were.
        if ops.drop_floating {
            work.drop_floating(1e-4 / scale);
        }
        // Only the final shape moves here too: a thickened detail keeps the colour it had.
        if ops.thicken_mm > 0.0 {
            work.thicken = Some(work.thicken(ops.thicken_mm / scale));
        }
        // Flattening squashes the walls under the cut into lines, and holes,
        // simplifying and the cut leave slivers after the clean-up ran.
        if ops.deep_clean || ops.flatten_mm > 0.0 {
            Self::clear_slivers(&mut shape, &mut work, CLEAN_MM / scale);
        }
        // Last, on the final shape: the wall is measured from the surface that prints.
        if ops.hollow_mm > 0.0 {
            let drains = (ops.drains.min(MAX_DRAINS), ops.drain_mm.max(0.0) / scale);
            Self::hollow_out(&mut shape, &mut work, ops.hollow_mm / scale, drains, CLEAN_MM / scale);
        }
        (shape, work)
    }

    /// Hollows `work` to a wall of `wall` (source units) with `drains` holes of
    /// that diameter through the base, and gives `shape` the same triangles.
    /// Leaves both alone, saying why, when the surface is not closed or the
    /// wall leaves no room.
    /// `clean` is the printability tolerance in source units: faces thinner than it are flipped away.
    fn hollow_out(shape: &mut Self, work: &mut Self, wall: f32, (drains, drain_d): (u8, f32), clean: f32) {
        use crate::hollow;
        let solid_volume = work.volume();
        let mut report = HollowReport {
            solid_volume,
            volume: solid_volume,
            cell: 0.0,
            drains: 0,
            drains_wanted: drains,
            refused: None,
        };
        let closed = work.parts.iter().all(|p| crate::mesh::boundary_loops(p).1 == 0 && !has_crowded_edges(p));
        let Some((lo, hi)) = work.bounds().filter(|_| closed) else {
            report.refused = Some(hollow::Refusal::Open);
            work.hollow = Some(report);
            return;
        };
        let longest = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max);
        report.cell = hollow::cell_for(longest, wall);
        let all: Vec<[Vec3; 3]> =
            work.parts.iter().flat_map(|p| (0..p.tris.len()).map(move |t| p.corners(t))).collect();
        let inner = match hollow::inner_wall(&all, wall, report.cell) {
            Ok(s) => hollow::lighten(s, report.cell * 0.25),
            Err(why) => {
                report.refused = Some(why);
                work.hollow = Some(report);
                return;
            }
        };
        // Each piece of wall goes to the part whose surface is nearest it, and
        // takes the colour of the closest outer face.
        let soup = work.soup();
        let tree = crate::bvh::Bvh::build(&soup);
        let pieces = MeshPart {
            name: String::new(),
            positions: inner.positions.clone(),
            tris: inner.tris.clone(),
            origin: None,
        }
        .components();
        let mut owner_of_piece: HashMap<u32, u32> = HashMap::new();
        let mut added: Vec<Vec<usize>> = vec![vec![]; work.parts.len()];
        for (t, &piece) in pieces.iter().enumerate() {
            let owner = *owner_of_piece.entry(piece).or_insert_with(|| {
                let at = inner.positions[inner.tris[t][0] as usize];
                tree.nearest(&soup, at).map_or(0, |(f, _)| soup.owner[f as usize].0)
            });
            added[owner as usize].push(t);
        }
        // The biggest piece of wall is the cavity the drains go into.
        let mut sizes: HashMap<u32, usize> = HashMap::new();
        for &p in &pieces {
            *sizes.entry(p).or_default() += 1;
        }
        let main = sizes.iter().max_by_key(|(_, &n)| n).map(|(&p, _)| p);
        for (pi, tris) in added.iter().enumerate() {
            if tris.is_empty() {
                continue;
            }
            let part = &mut work.parts[pi];
            let outer_tris = part.tris.len();
            let mut local: HashMap<u32, u32> = HashMap::new();
            let mut main_tris = vec![];
            for &t in tris {
                let f = inner.tris[t].map(|v| {
                    *local.entry(v).or_insert_with(|| {
                        part.positions.push(inner.positions[v as usize]);
                        part.positions.len() as u32 - 1
                    })
                });
                if Some(pieces[t]) == main {
                    main_tris.push(part.tris.len());
                }
                part.tris.push(f);
                if let Some(origin) = &mut part.origin {
                    let centre = geom::at(&f.map(|v| part.positions[v as usize]), [1.0 / 3.0; 3]);
                    let spot = tree
                        .nearest(&soup, centre)
                        .filter(|(f, _)| soup.owner[*f as usize].0 == pi as u32)
                        .map(|(f, b)| {
                            let (src, corners) = origin[soup.owner[f as usize].1 as usize];
                            (src, [geom::bary_in(&corners, b); 3])
                        })
                        .unwrap_or_else(|| {
                            let (src, corners) = origin[0];
                            (src, [geom::bary_in(&corners, [1.0 / 3.0; 3]); 3])
                        });
                    origin.push(spot);
                }
            }
            if drains > 0 && drain_d > 0.0 && !main_tris.is_empty() {
                report.drains += drill(part, outer_tris, &main_tris, drains, drain_d / 2.0, report.cell);
            }
        }
        // `shape` gets the same faces; its own positions for the old vertices.
        for (s, w) in shape.parts.iter_mut().zip(&work.parts) {
            let old = s.positions.len();
            s.positions.extend_from_slice(&w.positions[old..]);
            s.tris.clone_from(&w.tris);
            s.origin.clone_from(&w.origin);
        }
        // The wall's flat stretches can leave faces with their corners in a line: flip them away.
        Self::clear_slivers(shape, work, clean);

        work.repair.degenerate_triangles = 0;
        report.volume = work.volume();
        work.hollow = Some(report);
    }

    /// Pushes out what is thinner than `min` (source units). Each face is
    /// measured as printability measures it: from its centre, inward along its
    /// normal, to the surface across. A face short of `min` moves out along its
    /// own normal by half the shortfall (the face across does the same): each
    /// corner takes the area-weighted average of what its short faces ask, so a
    /// thin plate grows through its thin sides only and a pin grows round.
    /// Neighbours of moved vertices follow by half, so no step is left. A few
    /// rounds, since moving one side changes what the other measures. Faces
    /// thick enough, and what is around them, stay where they are.
    pub fn thicken(&mut self, min: f32) -> ThickenReport {
        use rayon::prelude::*;
        let original: Vec<Vec<Vec3>> = self.parts.iter().map(|p| p.positions.clone()).collect();
        for _ in 0..6 {
            let soup = self.soup();
            if soup.is_empty() {
                break;
            }
            let tree = crate::bvh::Bvh::build(&soup);
            let mut any = false;
            for part in &mut self.parts {
                let start = min * 1e-3;
                // What each face asks: out along its normal by half its shortfall, and its area.
                let ask: Vec<(Vec3, f32)> = (0..part.tris.len())
                    .into_par_iter()
                    .map(|t| {
                        let c = part.corners(t);
                        let cross = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
                        let area = geom::len(cross) * 0.5;
                        if area == 0.0 {
                            return ([0.0; 3], 0.0);
                        }
                        let n = geom::scale(cross, 0.5 / area);
                        let inward = geom::scale(n, -1.0);
                        let at = geom::add(geom::at(&c, [1.0 / 3.0; 3]), geom::scale(inward, start));
                        match tree.raycast(&soup, at, inward) {
                            Some(h) if h.t + start < min => (geom::scale(n, (min - h.t - start) * 0.5), area),
                            _ => ([0.0; 3], 0.0),
                        }
                    })
                    .collect();
                let mut sum = vec![([0.0f32; 3], 0.0f32); part.positions.len()];
                for (t, &(d, w)) in ask.iter().enumerate() {
                    if w == 0.0 {
                        continue;
                    }
                    for &v in &part.tris[t] {
                        let s = &mut sum[v as usize];
                        *s = (geom::add(s.0, geom::scale(d, w)), s.1 + w);
                    }
                }
                let push: Vec<Vec3> =
                    sum.iter().map(|&(d, w)| if w > 0.0 { geom::scale(d, 1.0 / w) } else { [0.0; 3] }).collect();
                if push.iter().all(|d| *d == [0.0; 3]) {
                    continue;
                }
                any = true;
                // Blend: a vertex beside a pushed one goes half as far, so the change has no step.
                let mut ring: Vec<Vec<u32>> = vec![vec![]; part.positions.len()];
                for t in &part.tris {
                    for k in 0..3 {
                        ring[t[k] as usize].push(t[(k + 1) % 3]);
                    }
                }
                let moves: Vec<Vec3> = (0..push.len())
                    .map(|v| {
                        if push[v] != [0.0; 3] {
                            return push[v];
                        }
                        let strongest = ring[v]
                            .iter()
                            .map(|&w| push[w as usize])
                            .max_by(|a, b| geom::len(*a).total_cmp(&geom::len(*b)));
                        strongest.map_or([0.0; 3], |d| geom::scale(d, 0.5))
                    })
                    .collect();
                for (p, d) in part.positions.iter_mut().zip(moves) {
                    *p = geom::add(*p, d);
                }
            }
            if !any {
                break;
            }
        }
        let mut report = ThickenReport::default();
        for (part, before) in self.parts.iter().zip(&original) {
            let off: Vec<f32> = part.positions.iter().zip(before).map(|(a, b)| geom::len(geom::sub(*a, *b))).collect();
            for &d in &off {
                if d > 0.0 {
                    report.moved_vertices += 1;
                    report.max_offset = report.max_offset.max(d);
                }
            }
            for t in 0..part.tris.len() {
                if part.tris[t].iter().any(|&v| off[v as usize] > 0.0) {
                    report.area += geom::area(&part.corners(t));
                }
            }
        }
        report
    }

    /// Signed volume of every part together, in source units³.
    pub fn volume(&self) -> f32 {
        let Some((lo, _)) = self.bounds() else { return 0.0 };
        let v: f64 = self
            .parts
            .iter()
            .flat_map(|p| (0..p.tris.len()).map(move |t| p.corners(t)))
            .map(|c| {
                let [a, b, d] = c.map(|q| geom::sub(q, lo));
                geom::dot(a, geom::cross(b, d)) as f64 / 6.0
            })
            .sum();
        v as f32
    }

    /// Clears the slivers of `work` without opening it, as the printability
    /// analysis sees it (vertices within `eps` are one). A triangle with two
    /// corners together goes; a needle, its third corner on its longest edge,
    /// gives that edge to the face across it, so both end up as two proper
    /// triangles. `shape` gets the same changes, so both keep matching triangles.
    fn clear_slivers(shape: &mut Self, work: &mut Self, eps: f32) {
        let collapsed = |c: &[Vec3; 3]| (0..3).any(|k| geom::len(geom::sub(c[k], c[(k + 1) % 3])) <= eps);
        let mut fixed = 0;
        for (s, w) in shape.parts.iter_mut().zip(&mut work.parts) {
            let keep: Vec<bool> = (0..w.tris.len()).map(|i| !collapsed(&w.corners(i))).collect();
            fixed += keep.iter().filter(|&&k| !k).count();
            s.retain(&keep);
            w.retain(&keep);
            let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
            let id: Vec<u32> = w
                .positions
                .iter()
                .enumerate()
                .map(|(i, p)| *weld.entry(p.map(|c| (c / eps).round() as i64)).or_insert(i as u32))
                .collect();
            let key = |a: u32, b: u32| (id[a as usize].min(id[b as usize]), id[a as usize].max(id[b as usize]));
            // A flip can leave the next needle beside it; a few passes settle it.
            for _ in 0..4 {
                let mut edges: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
                for (ti, t) in w.tris.iter().enumerate() {
                    for k in 0..3 {
                        edges.entry(key(t[k], t[(k + 1) % 3])).or_default().push(ti);
                    }
                }
                let mut touched = vec![false; w.tris.len()];
                let mut flips = 0;
                for ti in 0..w.tris.len() {
                    let c = w.corners(ti);
                    if touched[ti] || geom::area(&c) > eps * eps || collapsed(&c) {
                        continue;
                    }
                    let edge = |k: usize| geom::len(geom::sub(c[(k + 1) % 3], c[k]));
                    let k = (0..3).max_by(|&i, &j| edge(i).total_cmp(&edge(j))).unwrap_or(0);
                    let t = w.tris[ti];
                    let apex = t[(k + 2) % 3];
                    let faces = &edges[&key(t[k], t[(k + 1) % 3])];
                    let [x, y] = faces[..] else { continue };
                    let o = if x == ti { y } else { x };
                    let ot = w.tris[o];
                    // The face across has to run the edge the other way.
                    let Some(m) = (0..3).find(|&m| {
                        id[ot[m] as usize] == id[t[(k + 1) % 3] as usize]
                            && id[ot[(m + 1) % 3] as usize] == id[t[k] as usize]
                    }) else {
                        continue;
                    };
                    if touched[o] {
                        continue;
                    }
                    let (b, a, d) = (ot[m], ot[(m + 1) % 3], ot[(m + 2) % 3]);
                    let (pa, pb) = (w.positions[a as usize], w.positions[b as usize]);
                    let ab = geom::sub(pb, pa);
                    let along = geom::dot(geom::sub(w.positions[apex as usize], pa), ab)
                        / geom::dot(ab, ab).max(f32::MIN_POSITIVE);
                    for part in [&mut *s, &mut *w] {
                        part.tris[ti] = [apex, a, d];
                        part.tris[o] = [apex, d, b];
                        if let Some(origin) = &mut part.origin {
                            // Both halves lie on the face across, so they take its colour.
                            let (src, bary) = origin[o];
                            let (ba, bb, bd) = (bary[(m + 1) % 3], bary[m], bary[(m + 2) % 3]);
                            let mid = geom::bary_in(&[ba, bb, bd], [1.0 - along, along, 0.0]);
                            origin[ti] = (src, [mid, ba, bd]);
                            origin[o] = (src, [mid, bd, bb]);
                        }
                    }
                    touched[ti] = true;
                    touched[o] = true;
                    flips += 1;
                }
                fixed += flips;
                if flips == 0 {
                    break;
                }
            }
        }
        work.repair.degenerate_triangles += fixed;
    }

    /// Welds vertices that coincide (to a millionth of the model size), then
    /// drops triangles that collapsed or repeat another one.
    pub fn repair_all(&mut self) {
        let eps = self.bounds().map_or(1e-4, |(lo, hi)| (geom::len(geom::sub(hi, lo)) * 1e-6).max(1e-6));
        self.weld(eps, 0.0);
    }

    /// What the printability analysis calls degenerate or repeated goes too:
    /// vertices closer than `tol` (source units) are welded and triangles of
    /// area up to `tol`² dropped. Then stray faces come off (see [`Self::remove_fins`]).
    pub fn deep_clean(&mut self, tol: f32) {
        self.weld(tol, tol * tol);
        self.remove_fins();
    }

    /// Welds vertices on a grid of `eps`, then drops triangles that collapsed,
    /// have an area up to `min_area`, or repeat another one.
    fn weld(&mut self, eps: f32, min_area: f32) {
        let mut report = self.repair;
        for part in &mut self.parts {
            let mut seen: HashMap<[i64; 3], u32> = HashMap::with_capacity(part.positions.len());
            let mut remap = Vec::with_capacity(part.positions.len());
            let mut positions = Vec::with_capacity(part.positions.len());
            for p in &part.positions {
                let key = p.map(|c| (c / eps).round() as i64);
                let idx = *seen.entry(key).or_insert_with(|| {
                    positions.push(*p);
                    positions.len() as u32 - 1
                });
                remap.push(idx);
            }
            report.welded_vertices += part.positions.len() - positions.len();
            part.positions = positions;
            for t in &mut part.tris {
                *t = t.map(|v| remap[v as usize]);
            }
            let mut faces: std::collections::HashSet<[u32; 3]> =
                std::collections::HashSet::with_capacity(part.tris.len());
            let keep: Vec<bool> = (0..part.tris.len())
                .map(|i| {
                    let t = part.tris[i];
                    if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || geom::area(&part.corners(i)) <= min_area {
                        report.degenerate_triangles += 1;
                        return false;
                    }
                    let mut key = t;
                    key.sort_unstable();
                    if !faces.insert(key) {
                        report.duplicate_triangles += 1;
                        return false;
                    }
                    true
                })
                .collect();
            part.retain(&keep);
        }
        self.repair = report;
    }

    /// Removes fins: small sheets of faces (joined through edges with exactly
    /// two faces) that hang off the surface by a non-manifold edge and leave the
    /// rest of their border open. They are what modelling leaves behind when a
    /// face is extruded or duplicated by mistake, and they make that edge
    /// non-manifold, its sides open and its vertices non-manifold all at once.
    /// Only sheets up to a tenth of the part's largest one count as fins.
    pub fn remove_fins(&mut self) {
        let mut report = self.repair;
        for part in &mut self.parts {
            // Taking a fin off can turn the next one into a fin; a few passes settle it.
            for _ in 0..8 {
                let mut edges: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
                for (ti, t) in part.tris.iter().enumerate() {
                    for k in 0..3 {
                        let (a, b) = (t[k], t[(k + 1) % 3]);
                        edges.entry((a.min(b), a.max(b))).or_default().push(ti);
                    }
                }
                let n = part.tris.len();
                let mut sheet = vec![usize::MAX; n];
                // Triangles, open edges and non-manifold edges of each sheet.
                let mut sheets: Vec<(usize, usize, usize)> = vec![];
                for start in 0..n {
                    if sheet[start] != usize::MAX {
                        continue;
                    }
                    let id = sheets.len();
                    let mut s = (0, 0, 0);
                    sheet[start] = id;
                    let mut stack = vec![start];
                    while let Some(ti) = stack.pop() {
                        s.0 += 1;
                        let t = part.tris[ti];
                        for k in 0..3 {
                            let (a, b) = (t[k], t[(k + 1) % 3]);
                            let faces = &edges[&(a.min(b), a.max(b))];
                            match faces.len() {
                                1 => s.1 += 1,
                                2 => {
                                    for &o in faces {
                                        if sheet[o] == usize::MAX {
                                            sheet[o] = id;
                                            stack.push(o);
                                        }
                                    }
                                }
                                _ => s.2 += 1,
                            }
                        }
                    }
                    sheets.push(s);
                }
                let largest = sheets.iter().map(|s| s.0).max().unwrap_or(0);
                let fin: Vec<bool> =
                    sheets.iter().map(|&(size, open, nm)| nm > 0 && open > 0 && size * 10 <= largest).collect();
                if !fin.contains(&true) {
                    break;
                }
                let keep: Vec<bool> = sheet.iter().map(|&s| !fin[s]).collect();
                report.fin_triangles += keep.iter().filter(|&&k| !k).count();
                part.retain(&keep);
            }
        }
        self.repair = report;
    }

    /// Removes connected pieces whose bounding box diagonal is under `min_size`
    /// (source units). The largest piece of each part always stays.
    pub fn remove_loose_pieces(&mut self, min_size: f32) {
        let mut report = self.repair;
        for part in &mut self.parts {
            let comp = part.components();
            let n = comp.iter().copied().max().map_or(0, |m| m as usize + 1);
            let mut lo = vec![[f32::MAX; 3]; n];
            let mut hi = vec![[f32::MIN; 3]; n];
            let mut count = vec![0usize; n];
            for (i, t) in part.tris.iter().enumerate() {
                let c = comp[i] as usize;
                count[c] += 1;
                for &v in t {
                    let p = part.positions[v as usize];
                    for k in 0..3 {
                        lo[c][k] = lo[c][k].min(p[k]);
                        hi[c][k] = hi[c][k].max(p[k]);
                    }
                }
            }
            let largest = (0..n).max_by_key(|&c| count[c]).unwrap_or(0);
            let small: Vec<bool> =
                (0..n).map(|c| c != largest && geom::len(geom::sub(hi[c], lo[c])) < min_size).collect();
            report.loose_pieces += small.iter().filter(|&&x| x).count();
            let keep: Vec<bool> = comp.iter().map(|&c| !small[c as usize]).collect();
            report.loose_triangles += keep.iter().filter(|&&k| !k).count();
            part.retain(&keep);
        }
        self.repair = report;
    }

    /// Caps every hole: each boundary loop gets a fan around its centroid. A cap
    /// triangle runs its boundary edge backwards, so it agrees with the face it
    /// closes, and keeps that face's colour by pointing at one spot of its source.
    /// Boundaries are found on shared vertex indices, so weld first.
    pub fn fill_holes(&mut self) {
        fn find(parent: &mut HashMap<u32, u32>, x: u32) -> u32 {
            let mut root = x;
            while let Some(&up) = parent.get(&root).filter(|&&up| up != root) {
                root = up;
            }
            parent.insert(x, root);
            root
        }
        let mut report = self.repair;
        for part in &mut self.parts {
            let mut uses: HashMap<(u32, u32), usize> = HashMap::new();
            for t in &part.tris {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    *uses.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            // Directed boundary edges as their face runs them, with that face.
            let mut boundary = vec![];
            for (ti, t) in part.tris.iter().enumerate() {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    if uses[&(a.min(b), a.max(b))] == 1 {
                        boundary.push((a, b, ti));
                    }
                }
            }
            if boundary.is_empty() {
                continue;
            }
            // A loop is the boundary vertices its edges join.
            let mut parent: HashMap<u32, u32> = HashMap::new();
            for &(a, b, _) in &boundary {
                parent.entry(a).or_insert(a);
                parent.entry(b).or_insert(b);
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent.insert(rb, ra);
                }
            }
            let mut centre: HashMap<u32, (Vec3, f32)> = HashMap::new();
            for &(a, _, _) in &boundary {
                let root = find(&mut parent, a);
                let c = centre.entry(root).or_insert(([0.0; 3], 0.0));
                c.0 = geom::add(c.0, part.positions[a as usize]);
                c.1 += 1.0;
            }
            let mut hub: HashMap<u32, u32> = HashMap::new();
            for (&root, &(sum, n)) in &centre {
                hub.insert(root, part.positions.len() as u32);
                part.positions.push(geom::scale(sum, 1.0 / n));
            }
            report.filled_holes += hub.len();
            for (a, b, ti) in boundary {
                let c = hub[&find(&mut parent, a)];
                part.tris.push([b, a, c]);
                if let Some(origin) = &mut part.origin {
                    let (src, corners) = origin[ti];
                    let spot = geom::bary_in(&corners, [1.0 / 3.0; 3]);
                    origin.push((src, [spot; 3]));
                }
            }
        }
        self.repair = report;
    }

    /// Flips faces so neighbours run each shared edge in opposite directions,
    /// then turns any piece whose volume comes out negative inside out. Only
    /// edges with exactly two faces carry the orientation across.
    pub fn orient(&mut self) {
        let mut report = self.repair;
        for part in &mut self.parts {
            let mut edges: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
            for (ti, t) in part.tris.iter().enumerate() {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    edges.entry((a.min(b), a.max(b))).or_default().push((ti, a < b));
                }
            }
            let n = part.tris.len();
            let mut flip = vec![false; n];
            let mut seen = vec![false; n];
            let mut piece = vec![0usize; n];
            let mut volume: Vec<f64> = vec![];
            for start in 0..n {
                if seen[start] {
                    continue;
                }
                let id = volume.len();
                volume.push(0.0);
                seen[start] = true;
                let mut stack = vec![start];
                while let Some(ti) = stack.pop() {
                    piece[ti] = id;
                    let t = part.tris[ti];
                    for k in 0..3 {
                        let (a, b) = (t[k], t[(k + 1) % 3]);
                        let faces = &edges[&(a.min(b), a.max(b))];
                        if faces.len() != 2 {
                            continue;
                        }
                        let mine = (a < b) ^ flip[ti];
                        for &(other, raw) in faces {
                            if other != ti && !seen[other] {
                                // The neighbour must run this edge the other way.
                                flip[other] = raw == mine;
                                seen[other] = true;
                                stack.push(other);
                            }
                        }
                    }
                }
            }
            // Translate near the piece to avoid cancellation at large offsets.
            let lo = part.positions.iter().fold([f32::MAX; 3], |m, p| std::array::from_fn(|k| m[k].min(p[k])));
            for ti in 0..n {
                let mut c = part.corners(ti);
                if flip[ti] {
                    c.swap(1, 2);
                }
                let [a, b, d] = c.map(|p| geom::sub(p, lo));
                volume[piece[ti]] += geom::dot(a, geom::cross(b, d)) as f64;
            }
            // An inside-out piece inside another piece is a cavity (a hollowed model): it stays as it is.
            let cavity: Vec<bool> =
                (0..volume.len()).map(|id| volume[id] < 0.0 && inside_other(part, &piece, &flip, id)).collect();
            for ti in 0..n {
                if volume[piece[ti]] < 0.0 && !cavity[piece[ti]] {
                    flip[ti] = !flip[ti];
                }
                if flip[ti] {
                    part.tris[ti].swap(1, 2);
                    if let Some(origin) = &mut part.origin {
                        origin[ti].1.swap(1, 2);
                    }
                    report.flipped_triangles += 1;
                }
            }
        }
        self.repair = report;
    }

    /// Unbinds non-manifold vertices: the faces around a vertex that do not
    /// reach each other across an edge through it each get their own copy,
    /// moved `nudge` towards their fan so a positional weld keeps them apart.
    pub fn split_vertices(&mut self, nudge: f32) {
        let mut report = self.repair;
        for part in &mut self.parts {
            let mut around: Vec<Vec<usize>> = vec![vec![]; part.positions.len()];
            for (ti, t) in part.tris.iter().enumerate() {
                for &v in t {
                    around[v as usize].push(ti);
                }
            }
            for (v, faces) in around.iter().enumerate() {
                if faces.len() < 2 {
                    continue;
                }
                // Fans: faces joined through an edge that has `v` at one end.
                let mut fan: Vec<usize> = (0..faces.len()).collect();
                fn root(fan: &mut [usize], mut i: usize) -> usize {
                    while fan[i] != i {
                        fan[i] = fan[fan[i]];
                        i = fan[i];
                    }
                    i
                }
                let others = |ti: usize| part.tris[ti].iter().copied().filter(|&w| w as usize != v).collect::<Vec<_>>();
                for i in 0..faces.len() {
                    for j in i + 1..faces.len() {
                        let a = others(faces[i]);
                        if others(faces[j]).iter().any(|w| a.contains(w)) {
                            let (ri, rj) = (root(&mut fan, i), root(&mut fan, j));
                            fan[rj] = ri;
                        }
                    }
                }
                let roots: Vec<usize> = (0..faces.len()).map(|i| root(&mut fan, i)).collect();
                let mut distinct = roots.clone();
                distinct.sort_unstable();
                distinct.dedup();
                if distinct.len() < 2 {
                    continue;
                }
                let p = part.positions[v];
                // The first fan keeps the vertex; each other fan gets a copy.
                for &r in &distinct[1..] {
                    let mine: Vec<usize> = (0..faces.len()).filter(|&i| roots[i] == r).map(|i| faces[i]).collect();
                    let centre = mine.iter().fold([0.0; 3], |acc, &ti| {
                        geom::add(
                            acc,
                            geom::scale(
                                geom::add(geom::add(part.corners(ti)[0], part.corners(ti)[1]), part.corners(ti)[2]),
                                1.0 / 3.0,
                            ),
                        )
                    });
                    let towards = geom::sub(geom::scale(centre, 1.0 / mine.len() as f32), p);
                    let len = geom::len(towards);
                    let copy =
                        if len > 0.0 { geom::add(p, geom::scale(towards, nudge.min(len * 0.5) / len)) } else { p };
                    let id = part.positions.len() as u32;
                    part.positions.push(copy);
                    for ti in mine {
                        for w in part.tris[ti].iter_mut() {
                            if *w as usize == v {
                                *w = id;
                            }
                        }
                    }
                    report.split_vertices += 1;
                }
            }
        }
        self.repair = report;
    }

    /// Lowers each connected piece that has nothing under it (see [`unsupported`])
    /// until it rests on the lowest point of the whole mesh. Pieces resting on or
    /// sunk into another one stay where they are.
    pub fn drop_floating(&mut self, tol: f32) {
        let Some((lo, _)) = self.bounds() else { return };
        let mut report = self.repair;
        // Pieces of every part, since one object usually sits on another.
        let mut pieces = vec![];
        let mut boxes = vec![];
        for part in &self.parts {
            let comp = part.components();
            let n = comp.iter().copied().max().map_or(0, |m| m as usize + 1);
            let mut bb = vec![([f32::MAX; 3], [f32::MIN; 3]); n];
            let mut piece_of = vec![u32::MAX; part.positions.len()];
            for (ti, t) in part.tris.iter().enumerate() {
                let c = comp[ti] as usize;
                for &v in t {
                    let p = part.positions[v as usize];
                    bb[c] =
                        (std::array::from_fn(|k| bb[c].0[k].min(p[k])), std::array::from_fn(|k| bb[c].1[k].max(p[k])));
                    piece_of[v as usize] = comp[ti];
                }
            }
            pieces.push((piece_of, boxes.len()));
            boxes.extend(bb);
        }
        let floats = unsupported(&boxes, lo[2], tol);
        report.dropped_pieces += floats.iter().filter(|&&f| f).count();
        for (part, (piece_of, first)) in self.parts.iter_mut().zip(pieces) {
            for (v, p) in part.positions.iter_mut().enumerate() {
                let Some(i) = piece_of.get(v).filter(|&&c| c != u32::MAX).map(|&c| first + c as usize) else {
                    continue;
                };
                if floats[i] {
                    p[2] -= boxes[i].0[2] - lo[2];
                }
            }
        }
        self.repair = report;
    }

    /// Subdivides and smooths every part as `ops` asks (see [`crate::smooth`]),
    /// keeping sharp edges and the borders between `labels`. Levels that would
    /// pass [`SUBDIVIDE_LIMIT`] triangles are skipped.
    pub fn round_off(&mut self, ops: MeshOps, labels: &[Vec<u8>]) {
        let mut levels = ops.subdivide.min(MAX_SUBDIVIDE);
        while levels > 0 && self.triangle_count() * 4usize.pow(levels as u32) > SUBDIVIDE_LIMIT {
            levels -= 1;
        }
        let crease = if ops.crease_deg.is_finite() { ops.crease_deg.clamp(0.0, 180.0) } else { 30.0 };
        for (pi, part) in self.parts.iter_mut().enumerate() {
            let mut l = labels.get(pi).cloned().unwrap_or_default();
            for _ in 0..levels {
                crate::smooth::subdivide(part, &mut l, crease);
            }
            crate::smooth::smooth(part, &l, crease, ops.smooth as u32);
        }
    }

    /// Farthest a vertex of `self` lies from the surface of `other` (same units),
    /// over up to 100 000 of them spread evenly.
    pub fn deviation_to(&self, other: &Mesh) -> f32 {
        use rayon::prelude::*;
        let soup = other.soup();
        if soup.is_empty() {
            return 0.0;
        }
        let bvh = crate::bvh::Bvh::build(&soup);
        let points: Vec<Vec3> = self.parts.iter().flat_map(|p| p.positions.iter().copied()).collect();
        let stride = points.len().div_ceil(100_000).max(1);
        points
            .par_iter()
            .step_by(stride)
            .map(|&q| {
                bvh.nearest(&soup, q)
                    .map_or(0.0, |(t, b)| geom::len(geom::sub(q, geom::at(&soup.corners[t as usize], b))))
            })
            .reduce(|| 0.0, f32::max)
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.tris.len()).sum()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.parts.iter().flat_map(|p| p.positions.iter().copied());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        }))
    }

    /// Quadric simplification (meshoptimizer) of every part to `keep` of its triangles.
    pub fn simplify(&mut self, keep: f32) {
        self.simplify_with(keep, None, false);
    }

    /// Vertices of each part where triangles of different `labels` meet: the colour borders.
    pub fn border_vertices(&self, labels: &[Vec<u8>]) -> Vec<Vec<bool>> {
        self.parts
            .iter()
            .enumerate()
            .map(|(pi, part)| {
                let mut first = vec![u8::MAX; part.positions.len()];
                let mut border = vec![false; part.positions.len()];
                let Some(labels) = labels.get(pi).filter(|l| l.len() == part.tris.len()) else { return border };
                for (t, &l) in part.tris.iter().zip(labels) {
                    for &v in t {
                        let f = &mut first[v as usize];
                        if *f == u8::MAX {
                            *f = l;
                        } else if *f != l {
                            border[v as usize] = true;
                        }
                    }
                }
                border
            })
            .collect()
    }

    /// [`Self::simplify`] keeping the vertices `locks` marks in place, and with
    /// `detail`, giving priority to the vertices where the surface bends (see
    /// [`MeshPart::bent_vertices`]) so details lose fewer triangles than flat areas.
    pub fn simplify_with(&mut self, keep: f32, locks: Option<&[Vec<bool>]>, detail: bool) {
        for (pi, part) in self.parts.iter_mut().enumerate() {
            let indices: Vec<u32> = part.tris.iter().flatten().copied().collect();
            let target = ((part.tris.len() as f32 * keep) as usize).max(4) * 3;
            let Ok(adapter) = meshopt::VertexDataAdapter::new(
                meshopt::typed_to_bytes(&part.positions),
                std::mem::size_of::<Vec3>(),
                0,
            ) else {
                continue;
            };
            let lock = locks.and_then(|l| l.get(pi)).filter(|l| l.len() == part.positions.len());
            let out = if lock.is_none() && !detail {
                meshopt::simplify(&indices, &adapter, target, 0.02, meshopt::SimplifyOptions::None, None)
            } else {
                use meshopt::SimplifyVertex as V;
                let bent = if detail { part.bent_vertices(DETAIL_DEG) } else { vec![] };
                let flags: Vec<V> = (0..part.positions.len())
                    .map(|v| {
                        let mut f = V::None;
                        if lock.is_some_and(|l| l[v]) {
                            f |= V::Lock;
                        }
                        if bent.get(v).copied().unwrap_or(false) {
                            f |= V::Priority;
                        }
                        f
                    })
                    .collect();
                meshopt::simplify_with_locks(
                    &indices,
                    &adapter,
                    &flags,
                    target,
                    0.02,
                    meshopt::SimplifyOptions::None,
                    None,
                )
            };
            if out.len() < 3 {
                continue;
            }
            // Compact the vertex buffer to the vertices still in use.
            let mut remap = vec![u32::MAX; part.positions.len()];
            let mut positions = Vec::new();
            let mut tris = Vec::with_capacity(out.len() / 3);
            for t in out.as_chunks::<3>().0 {
                let mut tri = [0u32; 3];
                for (k, &v) in t.iter().enumerate() {
                    if remap[v as usize] == u32::MAX {
                        remap[v as usize] = positions.len() as u32;
                        positions.push(part.positions[v as usize]);
                    }
                    tri[k] = remap[v as usize];
                }
                tris.push(tri);
            }
            part.positions = positions;
            part.tris = tris;
            part.origin = None;
        }
    }

    /// Splits every triangle that crosses the plane `z = cut`, sharing the new
    /// vertices between neighbours so the mesh stays closed.
    pub fn split_at(&mut self, cut: f32) {
        for part in &mut self.parts {
            let mut on_edge: HashMap<(u32, u32), u32> = HashMap::new();
            let mut tris = Vec::with_capacity(part.tris.len());
            let mut origin = part.origin.as_ref().map(|o| Vec::with_capacity(o.len()));
            let below = |z: f32| z < cut;
            for (ti, &t) in part.tris.iter().enumerate() {
                let side = t.map(|v| below(part.positions[v as usize][2]));
                let parent_origin = part.origin.as_ref().map(|o| o[ti]);
                if side[0] == side[1] && side[1] == side[2] {
                    tris.push(t);
                    if let (Some(o), Some(po)) = (origin.as_mut(), parent_origin) {
                        o.push(po);
                    }
                    continue;
                }
                // Rotate so corner 0 is the one alone on its side; winding is kept.
                let k = (0..3).find(|&k| side[k] != side[(k + 1) % 3] && side[k] != side[(k + 2) % 3]).unwrap_or(0);
                let (l, p, q) = (k, (k + 1) % 3, (k + 2) % 3);
                let mut edge_point = |a: usize, b: usize| -> (u32, f32) {
                    let (va, vb) = (t[a], t[b]);
                    let (za, zb) = (part.positions[va as usize][2], part.positions[vb as usize][2]);
                    let s = ((cut - za) / (zb - za)).clamp(0.0, 1.0);
                    let key = (va.min(vb), va.max(vb));
                    let idx = *on_edge.entry(key).or_insert_with(|| {
                        part.positions.push(geom::add(
                            part.positions[va as usize],
                            geom::scale(geom::sub(part.positions[vb as usize], part.positions[va as usize]), s),
                        ));
                        part.positions.len() as u32 - 1
                    });
                    (idx, s)
                };
                let (x, sx) = edge_point(l, p);
                let (y, sy) = edge_point(l, q);
                let new = [[t[l], x, y], [x, t[p], t[q]], [x, t[q], y]];
                // Corners as barycentrics of the parent triangle.
                let mut e = [[0.0f32; 3]; 3];
                e[0][l] = 1.0;
                e[1][p] = 1.0;
                e[2][q] = 1.0;
                let lerp = |a: [f32; 3], b: [f32; 3], s: f32| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s);
                let bx: [f32; 3] = lerp(e[0], e[1], sx);
                let by: [f32; 3] = lerp(e[0], e[2], sy);
                let local = [[e[0], bx, by], [bx, e[1], e[2]], [bx, e[2], by]];
                for (nt, lb) in new.iter().zip(local) {
                    if nt[0] == nt[1] || nt[1] == nt[2] || nt[0] == nt[2] {
                        continue;
                    }
                    tris.push(*nt);
                    if let (Some(o), Some((src, pb))) = (origin.as_mut(), parent_origin) {
                        o.push((src, lb.map(|b| geom::bary_in(&pb, b))));
                    }
                }
            }
            part.tris = tris;
            part.origin = origin;
        }
    }

    /// Presses everything below `depth` (source units above the lowest point) onto
    /// one plane and sets that plane on Z = 0. Run [`Mesh::split_at`] first so the
    /// edge of the flat area is exact.
    pub fn flatten(&mut self, depth: f32) {
        let Some((lo, _)) = self.bounds() else { return };
        let cut = lo[2] + depth;
        for part in &mut self.parts {
            for v in &mut part.positions {
                v[2] = v[2].max(cut) - cut;
            }
        }
    }

    /// Area (source units²) of downward-facing triangles lying within `tol` of the plate.
    pub fn contact_area(&self, tol: f32) -> f32 {
        let Some((lo, _)) = self.bounds() else { return 0.0 };
        let mut area = 0.0;
        for part in &self.parts {
            for i in 0..part.tris.len() {
                let c = part.corners(i);
                if c.iter().all(|p| p[2] <= lo[2] + tol) {
                    let n = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
                    if n[2] < 0.0 {
                        area += geom::area(&c);
                    }
                }
            }
        }
        area
    }

    pub fn soup(&self) -> TriSoup {
        let mut s = TriSoup::default();
        for (pi, part) in self.parts.iter().enumerate() {
            for i in 0..part.tris.len() {
                s.push(part.corners(i), pi as u32, i as u32);
            }
        }
        s
    }
}

/// How far a face may turn from its vertex's normal before "Proteger detalle"
/// counts the vertex as part of a detail worth keeping.
pub const DETAIL_DEG: f32 = 12.0;

/// Weld and sliver size of the deep clean, in final millimetres: the
/// printability analysis's default tolerance, so what it flags goes.
pub const CLEAN_MM: f32 = 0.001;

/// What each advanced repair finds on a mesh, read on welded vertices as the
/// repairs see it. Each count is what that repair would change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshIssues {
    /// Boundary loops, and the edges with one face that make them.
    pub holes: usize,
    pub open_edges: usize,
    /// Faces [`Mesh::orient`] would turn over.
    pub flipped_triangles: usize,
    /// Extra copies [`Mesh::split_vertices`] would make.
    pub shared_vertices: usize,
    /// Pieces [`Mesh::drop_floating`] would lower.
    pub floating_pieces: usize,
    /// Slivers, repeats and stray faces [`Mesh::deep_clean`] would take off.
    pub stray_triangles: usize,
}

impl Mesh {
    /// Counts what each advanced repair would change, without changing `self`.
    /// `clean_tol` and `float_tol` are the deep clean's and the drop's tolerances
    /// in source units, as [`Mesh::build`] passes them.
    pub fn issues(&self, clean_tol: f32, float_tol: f32) -> MeshIssues {
        let mut welded = self.clone();
        welded.repair = RepairReport::default();
        welded.repair_all();
        let mut issues = MeshIssues::default();
        for part in &welded.parts {
            let (holes, open) = boundary_loops(part);
            issues.holes += holes;
            issues.open_edges += open;
        }
        let after = |f: &dyn Fn(&mut Mesh)| {
            let mut m = welded.clone();
            m.repair = RepairReport::default();
            f(&mut m);
            m.repair
        };
        issues.flipped_triangles = after(&|m| m.orient()).flipped_triangles;
        issues.shared_vertices = after(&|m| m.split_vertices(0.0)).split_vertices;
        issues.floating_pieces = after(&|m| m.drop_floating(float_tol)).dropped_pieces;
        let clean = after(&|m| m.deep_clean(clean_tol));
        issues.stray_triangles = clean.degenerate_triangles + clean.duplicate_triangles + clean.fin_triangles;
        issues
    }
}

/// Whether piece `id` of `part` (faces as `flip` would turn them) lies inside
/// another piece: a ray from one of its corners crosses the others an odd
/// number of times.
fn inside_other(part: &MeshPart, piece: &[usize], _flip: &[bool], id: usize) -> bool {
    let Some(t0) = piece.iter().position(|&p| p == id) else { return false };
    let from = part.corners(t0)[0];
    let d = geom::normalize([1.0, 0.000_731, 0.001_37]);
    let mut crossings = 0;
    for (t, &p) in piece.iter().enumerate() {
        if p == id {
            continue;
        }
        let c = part.corners(t);
        let (e1, e2) = (geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
        let q = geom::cross(d, e2);
        let det = geom::dot(e1, q);
        if det.abs() < 1e-12 {
            continue;
        }
        let s = geom::sub(from, c[0]);
        let u = geom::dot(s, q) / det;
        let r = geom::cross(s, e1);
        let v = geom::dot(d, r) / det;
        if (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0 && geom::dot(e2, r) / det > 1e-6 {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

/// Whether some edge of a part has more than two faces.
fn has_crowded_edges(part: &MeshPart) -> bool {
    let mut edges: Vec<(u32, u32)> = part
        .tris
        .iter()
        .flat_map(|t| (0..3).map(move |k| (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]))))
        .collect();
    edges.sort_unstable();
    edges.windows(3).any(|w| w[0] == w[1] && w[1] == w[2])
}

/// Drills up to `count` holes of radius `r` from the base of `part` into the
/// cavity whose faces are `cavity`; the faces before `outer` are the outside.
/// Each one replaces the faces over its disc, below and in the cavity floor,
/// with a ring, and joins the rings with a tube. Returns how many it made.
fn drill(part: &mut MeshPart, outer: usize, cavity: &[usize], count: u8, r: f32, cell: f32) -> u8 {
    let n = part.tris.len();
    let mut is_cavity = vec![false; n];
    for &t in cavity {
        is_cavity[t] = true;
    }
    // 0 outside, 1 cavity, 2 anything else (other walls): what each face is, kept through the changes.
    let mut kind: Vec<u8> = (0..n)
        .map(|t| {
            if t < outer {
                0
            } else if is_cavity[t] {
                1
            } else {
                2
            }
        })
        .collect();
    // Where to try: under the middle of the cavity floor first, then its lowest points.
    let floor_z =
        cavity.iter().flat_map(|&t| part.tris[t]).map(|v| part.positions[v as usize][2]).fold(f32::MAX, f32::min);
    let mut floor: Vec<Vec3> = cavity
        .iter()
        .flat_map(|&t| part.tris[t])
        .map(|v| part.positions[v as usize])
        .filter(|p| p[2] <= floor_z + cell * 1.5)
        .collect();
    if floor.is_empty() {
        return 0;
    }
    let mid = floor.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
    let mid = [mid[0] / floor.len() as f32, mid[1] / floor.len() as f32];
    let reach = floor.iter().map(|p| (p[0] - mid[0]).hypot(p[1] - mid[1])).fold(0.0, f32::max) * 0.5;
    let mut spots: Vec<[f32; 2]> = if count <= 1 {
        vec![mid]
    } else {
        (0..count)
            .map(|i| {
                let a = i as f32 / count as f32 * std::f32::consts::TAU;
                [mid[0] + reach * a.cos(), mid[1] + reach * a.sin()]
            })
            .collect()
    };
    floor.sort_by(|a, b| a[2].total_cmp(&b[2]));
    let step = floor.len().div_ceil(64).max(1);
    spots.extend(floor.iter().step_by(step).map(|p| [p[0], p[1]]));
    let mut made = 0u8;
    let mut done: Vec<[f32; 2]> = vec![];
    for c in spots {
        if made >= count {
            break;
        }
        if done.iter().any(|d| (d[0] - c[0]).hypot(d[1] - c[1]) < r * 3.0) {
            continue;
        }
        if drill_one(part, &mut kind, c, r) {
            done.push(c);
            made += 1;
        }
    }
    made
}

/// One drain at `c`: rings on the outside below and on the cavity floor above,
/// joined by a tube. Changes `part` only when all of it fits.
fn drill_one(part: &mut MeshPart, kind: &mut Vec<u8>, c: [f32; 2], r: f32) -> bool {
    use crate::hollow::{caster, ring, Sheet};
    let tris = part.tris.clone();
    let n = tris.len();
    let not_outer: Vec<bool> = kind.iter().map(|&k| k != 0).collect();
    let not_cavity: Vec<bool> = kind.iter().map(|&k| k != 1).collect();
    let below = part.positions.iter().map(|p| p[2]).fold(f32::MAX, f32::min) - 1.0;
    let (out_tree, out_soup) = caster(&part.positions, &tris, &not_outer);
    let (in_tree, in_soup) = caster(&part.positions, &tris, &not_cavity);
    let up = [0.0, 0.0, 1.0];
    // The outside's first face up from below, then the cavity's first face above it.
    let out_hit = |q: [f32; 2]| out_tree.raycast(&out_soup, [q[0], q[1], below], up);
    let in_hit = |q: [f32; 2], from: f32| in_tree.raycast(&in_soup, [q[0], q[1], from], up);
    let Some(o) = out_hit(c) else { return false };
    let Some(i) = in_hit(c, o.point[2]) else { return false };
    let (start_out, start_in) = (out_soup.owner[o.tri as usize].1 as usize, in_soup.owner[i.tri as usize].1 as usize);
    let height_out = |q: [f32; 2]| out_hit(q).map(|h| h.point[2]);
    let height_in = |q: [f32; 2]| out_hit(q).and_then(|h| in_hit(q, h.point[2])).map(|h| h.point[2]);
    let mut positions = part.positions.clone();
    let mut removed = vec![false; n];
    let Some(a) = ring(
        &mut Sheet { positions: &mut positions, tris: &tris, removed: &mut removed },
        c,
        r,
        -1.0,
        start_out,
        &height_out,
    ) else {
        return false;
    };
    let Some(b) = ring(
        &mut Sheet { positions: &mut positions, tris: &tris, removed: &mut removed },
        c,
        r,
        1.0,
        start_in,
        &height_in,
    ) else {
        return false;
    };
    // The tube: each side of the circle, from the base up to the cavity floor, facing the hole's axis.
    let mut tube = vec![];
    for s in 0..a.circle.len() {
        let s1 = (s + 1) % a.circle.len();
        for mut f in [[a.circle[s], a.circle[s1], b.circle[s1]], [a.circle[s], b.circle[s1], b.circle[s]]] {
            let [p, q, w] = f.map(|v| positions[v as usize]);
            let normal = geom::cross(geom::sub(q, p), geom::sub(w, p));
            let mid = geom::scale(geom::add(geom::add(p, q), w), 1.0 / 3.0);
            if geom::dot(normal, [c[0] - mid[0], c[1] - mid[1], 0.0]) < 0.0 {
                f.swap(1, 2);
            }
            tube.push(f);
        }
    }
    let spot = |t: usize| {
        part.origin.as_ref().map(|o| {
            let (src, corners) = o[t];
            (src, [geom::bary_in(&corners, [1.0 / 3.0; 3]); 3])
        })
    };
    let mut add = vec![];
    for (faces, from, k) in [(&a.tris, start_out, 0u8), (&tube, start_out, 0), (&b.tris, start_in, 1)] {
        for &f in faces {
            add.push((f, spot(from), k));
        }
    }
    part.positions = positions;
    for (f, o, k) in add {
        part.tris.push(f);
        if let (Some(origin), Some(o)) = (&mut part.origin, o) {
            origin.push(o);
        }
        kind.push(k);
    }
    let keep: Vec<bool> = removed.iter().map(|&r| !r).chain(std::iter::repeat_n(true, part.tris.len() - n)).collect();
    let mut it = keep.iter();
    kind.retain(|_| *it.next().unwrap_or(&true));
    part.retain(&keep);
    true
}

/// Holes (loops of edges with one face, joined at their vertices) and open edges of a welded part.
pub(crate) fn boundary_loops(part: &MeshPart) -> (usize, usize) {
    let mut edges: Vec<(u32, u32)> = part
        .tris
        .iter()
        .flat_map(|t| (0..3).map(move |k| (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]))))
        .collect();
    edges.sort_unstable();
    let mut open = vec![];
    let mut i = 0;
    while i < edges.len() {
        let mut j = i + 1;
        while j < edges.len() && edges[j] == edges[i] {
            j += 1;
        }
        if j - i == 1 {
            open.push(edges[i]);
        }
        i = j;
    }
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn find(parent: &mut HashMap<u32, u32>, x: u32) -> u32 {
        let mut root = x;
        while let Some(&up) = parent.get(&root).filter(|&&up| up != root) {
            root = up;
        }
        parent.insert(x, root);
        root
    }
    for &(a, b) in &open {
        parent.entry(a).or_insert(a);
        parent.entry(b).or_insert(b);
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent.insert(rb, ra);
        }
    }
    let keys: Vec<u32> = parent.keys().copied().collect();
    let loops: std::collections::HashSet<u32> = keys.into_iter().map(|v| find(&mut parent, v)).collect();
    (loops.len(), open.len())
}

/// Which pieces (by bounding box, lowest corner first) have nothing to hold
/// them up: they do not reach `floor` and do not touch, within `tol`, any piece
/// that does, directly or through others. Boxes are coarse, so a piece above a
/// hollow of its neighbour still counts as supported; a separate object resting
/// on another (a hat, an eye) is never taken for a floating one.
pub(crate) fn unsupported(boxes: &[(Vec3, Vec3)], floor: f32, tol: f32) -> Vec<bool> {
    let touch = |a: &(Vec3, Vec3), b: &(Vec3, Vec3)| (0..3).all(|k| a.0[k] <= b.1[k] + tol && b.0[k] <= a.1[k] + tol);
    let mut floats: Vec<bool> = boxes.iter().map(|b| b.0[2] > floor + tol).collect();
    let mut stack: Vec<usize> = (0..boxes.len()).filter(|&i| !floats[i]).collect();
    while let Some(i) = stack.pop() {
        for j in 0..boxes.len() {
            if floats[j] && touch(&boxes[i], &boxes[j]) {
                floats[j] = false;
                stack.push(j);
            }
        }
    }
    floats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::adjacency;

    /// A closed octahedron standing on its lower tip.
    fn octahedron() -> Mesh {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [-1.0, 0.0, 1.0],
            [0.0, -1.0, 1.0],
            [0.0, 0.0, 2.0],
        ];
        let tris = vec![[0, 2, 1], [0, 3, 2], [0, 4, 3], [0, 1, 4], [5, 1, 2], [5, 2, 3], [5, 3, 4], [5, 4, 1]];
        let origin = Some((0..8).map(|i| (i, ROOT)).collect());
        Mesh {
            parts: vec![MeshPart { name: "o".into(), positions, tris, origin }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        }
    }

    fn closed_cube() -> Mesh {
        let positions = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ];
        let tris = vec![
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
        let origin = Some((0..tris.len() as u32).map(|i| (i, ROOT)).collect());
        Mesh {
            parts: vec![MeshPart { name: "cube".into(), positions, tris, origin }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        }
    }

    fn signed_volume(m: &Mesh) -> f32 {
        let p = &m.parts[0];
        (0..p.tris.len())
            .map(|i| {
                let [a, b, c] = p.corners(i);
                geom::dot(a, geom::cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn orient_fixes_a_flipped_face_and_an_inside_out_piece() {
        let mut m = closed_cube();
        m.parts[0].tris[3].swap(1, 2);
        m.orient();
        assert_eq!(m.repair.flipped_triangles, 1);
        assert!((signed_volume(&m) - 1.0).abs() < 1e-5);
        let mut m = closed_cube();
        for t in &mut m.parts[0].tris {
            t.swap(1, 2);
        }
        m.orient();
        assert_eq!(m.repair.flipped_triangles, 12);
        assert!((signed_volume(&m) - 1.0).abs() < 1e-5);
        // Each corner keeps its own spot of the source: the origin swaps with it.
        assert_eq!(m.parts[0].origin.as_ref().unwrap()[0].1, [ROOT[0], ROOT[2], ROOT[1]]);
    }

    #[test]
    fn split_vertices_unbinds_a_bow_tie_and_drop_floating_lands_pieces() {
        // Two triangles touching only at vertex 0.
        let positions = vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [-1., 0., 0.], [-1., -1., 0.]];
        let mut m = Mesh {
            parts: vec![MeshPart { name: "bow".into(), positions, tris: vec![[0, 1, 2], [0, 3, 4]], origin: None }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        };
        m.split_vertices(0.01);
        assert_eq!(m.repair.split_vertices, 1);
        assert_eq!(m.parts[0].positions.len(), 6);
        assert_ne!(m.parts[0].tris[0][0], m.parts[0].tris[1][0]);
        assert!(geom::len(m.parts[0].positions[5]) <= 0.0101);
        // A closed cube has no such vertex.
        let mut cube = closed_cube();
        cube.split_vertices(0.01);
        assert_eq!(cube.repair.split_vertices, 0);

        let mut m = closed_cube();
        let raised: Vec<Vec3> = m.parts[0].positions.iter().map(|p| [p[0] + 3.0, p[1], p[2] + 5.0]).collect();
        let offset = m.parts[0].positions.len() as u32;
        let tris: Vec<[u32; 3]> = m.parts[0].tris.iter().map(|t| t.map(|v| v + offset)).collect();
        m.parts[0].positions.extend(raised);
        m.parts[0].tris.extend(tris);
        m.parts[0].origin = None;
        m.drop_floating(1e-4);
        assert_eq!(m.repair.dropped_pieces, 1);
        assert!(m.parts[0].positions.iter().all(|p| p[2] <= 1.0 + 1e-6));

        // A separate object stacked on the first one (a hat, an eye) is held up and stays put.
        let mut m = closed_cube();
        let mut top = closed_cube().parts.remove(0);
        for p in &mut top.positions {
            p[2] += 1.0;
        }
        m.parts.push(top);
        let before = m.parts[1].positions.clone();
        m.drop_floating(1e-4);
        assert_eq!(m.repair.dropped_pieces, 0);
        assert_eq!(m.parts[1].positions, before);
        let report = crate::printability::analyze(&m, crate::printability::Params::for_nozzle(0.4));
        assert_eq!(report.checks.iter().find(|c| c.code == "floating").unwrap().count, 0);
    }

    #[test]
    fn deep_clean_takes_off_a_stray_face_hanging_by_one_edge() {
        let mut m = closed_cube();
        // A flap on the bottom edge 0-1: that edge gets three faces, the flap two open sides.
        m.parts[0].positions.push([0.5, -1.0, -1.0]);
        m.parts[0].tris.push([0, 1, 8]);
        m.parts[0].origin.as_mut().unwrap().push((0, ROOT));
        let before = crate::printability::analyze(&m, crate::printability::Params::for_nozzle(0.4));
        let count =
            |r: &crate::printability::Report, code: &str| r.checks.iter().find(|c| c.code == code).unwrap().count;
        assert_eq!(count(&before, "nonManifoldEdges"), 1);
        assert_eq!(count(&before, "openEdges"), 2);
        m.deep_clean(1e-3);
        assert_eq!(m.repair.fin_triangles, 1);
        assert_eq!(m.parts[0].tris.len(), 12);
        let after = crate::printability::analyze(&m, crate::printability::Params::for_nozzle(0.4));
        for code in ["nonManifoldEdges", "openEdges", "nonManifoldVertices"] {
            assert_eq!(count(&after, code), 0, "{code}");
        }
        // A real hole in the surface is not a fin: nothing hangs from a non-manifold edge.
        let mut open = closed_cube();
        open.parts[0].tris.pop();
        open.parts[0].origin.as_mut().unwrap().pop();
        open.deep_clean(1e-3);
        assert_eq!(open.parts[0].tris.len(), 11);
    }

    #[test]
    fn fill_holes_closes_the_box_with_matching_winding() {
        let mut m = closed_cube();
        m.parts[0].tris.drain(2..4);
        m.parts[0].origin.as_mut().unwrap().drain(2..4);
        m.fill_holes();
        assert_eq!(m.repair.filled_holes, 1);
        assert_eq!(m.parts[0].tris.len(), 14);
        assert_eq!(m.parts[0].origin.as_ref().unwrap().len(), 14);
        assert!((signed_volume(&m) - 1.0).abs() < 1e-5);
        // Closed and consistent now: orienting changes nothing.
        m.orient();
        assert_eq!(m.repair.flipped_triangles, 0);
    }

    #[test]
    fn issues_count_what_each_repair_would_change() {
        let mut m = closed_cube();
        // The top open, one side face turned over, the whole cube up off the plate.
        m.parts[0].tris.drain(2..4);
        m.parts[0].origin.as_mut().unwrap().drain(2..4);
        m.parts[0].tris[3].swap(1, 2);
        let clean = closed_cube().issues(1e-4, 1e-4);
        assert_eq!(clean, MeshIssues::default());
        let i = m.issues(1e-4, 1e-4);
        assert_eq!((i.holes, i.open_edges), (1, 4));
        assert_eq!(i.flipped_triangles, 1);
        assert_eq!(i.shared_vertices, 0);
        // Counting leaves the mesh as it was.
        assert_eq!(m.parts[0].tris.len(), 10);
        m.fill_holes();
        m.orient();
        let fixed = m.issues(1e-4, 1e-4);
        assert_eq!((fixed.holes, fixed.open_edges, fixed.flipped_triangles), (0, 0, 0));
    }

    /// Every edge of every part run once each way: closed and consistently wound.
    fn sealed(m: &Mesh) -> bool {
        m.parts.iter().all(|p| {
            let mut count: HashMap<(u32, u32), i32> = HashMap::new();
            for t in &p.tris {
                for k in 0..3 {
                    *count.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
                }
            }
            count.iter().all(|(&(a, b), &n)| n == 1 && count.get(&(b, a)) == Some(&1))
        })
    }

    fn big_cube(size: f32) -> Mesh {
        let mut m = closed_cube();
        for p in &mut m.parts[0].positions {
            *p = geom::scale(*p, size);
        }
        m
    }

    #[test]
    fn hollowing_a_cube_leaves_a_sealed_shell_and_a_drain_through_its_base() {
        let mut work = big_cube(20.0);
        let mut shape = work.clone();
        Mesh::hollow_out(&mut shape, &mut work, 2.0, (0, 0.0), 1e-3);
        let r = work.hollow.unwrap();
        assert_eq!(r.refused, None);
        assert!(sealed(&work));
        // A 16 mm cavity, give or take the voxels.
        let cavity = 16f32.powi(3);
        assert!(((r.solid_volume - r.volume) - cavity).abs() / cavity < 0.12, "{r:?}");
        assert_eq!(shape.parts[0].tris.len(), work.parts[0].tris.len());
        assert_eq!(work.parts[0].origin.as_ref().unwrap().len(), work.parts[0].tris.len());

        let mut work = big_cube(20.0);
        let mut shape = work.clone();
        Mesh::hollow_out(&mut shape, &mut work, 2.0, (1, 4.0), 1e-3);
        let r = work.hollow.unwrap();
        assert_eq!((r.drains, r.drains_wanted), (1, 1));
        assert!(sealed(&work), "the drain keeps the shell closed");
        // One piece now: the cavity opens to the outside through the base.
        assert_eq!(work.parts[0].components().iter().max(), Some(&0));
        // The model is still a positive solid, a little lighter than with no drain.
        assert!(r.volume > 0.0 && r.volume < r.solid_volume - cavity * 0.88, "{r:?}");
        // The hole is in the base: some base vertices sit on its circle.
        let base: Vec<Vec3> = work.parts[0].positions.iter().filter(|p| p[2].abs() < 1e-4).copied().collect();
        let centre = base
            .iter()
            .filter(|p| p[0] > 0.0 && p[0] < 20.0 && p[1] > 0.0 && p[1] < 20.0)
            .fold(([0.0; 2], 0.0), |(s, n), p| ([s[0] + p[0], s[1] + p[1]], n + 1.0));
        let centre = [centre.0[0] / centre.1, centre.0[1] / centre.1];
        let on_circle = base.iter().filter(|p| ((p[0] - centre[0]).hypot(p[1] - centre[1]) - 2.0).abs() < 0.01).count();
        assert_eq!(on_circle, 24, "{centre:?}");
        // Near the middle of the base, below the middle of the cavity.
        assert!((centre[0] - 10.0).abs() < 1.0 && (centre[1] - 10.0).abs() < 1.0, "{centre:?}");
    }

    #[test]
    fn a_hollow_cylinder_stays_sealed_with_and_without_drains() {
        for (wall, drains) in [(2.0, 0), (2.0, 2), (0.8, 0), (0.8, 2)] {
            let mut part = crate::smooth::tests::cylinder(48);
            for p in &mut part.positions {
                *p = [p[0] * 22.0 + 22.0, p[1] * 22.0 + 22.0, p[2] * 22.5];
            }
            let mut work = Mesh { parts: vec![part], ..Default::default() };
            let mut shape = work.clone();
            Mesh::hollow_out(&mut shape, &mut work, wall, (drains, 3.0), 1e-3);
            let r = work.hollow.unwrap();
            assert_eq!((r.refused, r.drains), (None, drains), "{wall} {drains}");
            let i = work.issues(1e-3, 1e-4);
            assert_eq!((i.open_edges, i.flipped_triangles, i.stray_triangles), (0, 0, 0), "{wall} {drains}: {i:?}");
            // As printability sees it: closed, a cavity inside, and every wall measured.
            let p = crate::printability::analyze(&work, crate::printability::Params::for_nozzle(0.4));
            let walls = p.checks.iter().find(|c| c.code == "thinWalls").unwrap();
            assert_eq!(
                walls.evidence,
                "estimated",
                "{wall} {drains}: {:?}",
                p.checks
                    .iter()
                    .filter(|c| c.count > 0 || c.evidence == "notEvaluated")
                    .map(|c| (&c.code, c.count))
                    .collect::<Vec<_>>()
            );
            let measured = p.thickness.iter().filter(|t| t.mm.is_some()).count();
            assert!(measured * 100 >= p.thickness.len() * 95, "{wall} {drains}: {measured} of {}", p.thickness.len());
            // The thinnest wall is the one asked for, give or take a voxel.
            let cell = work.hollow.unwrap().cell;
            assert!(
                (walls.measurement.unwrap() - wall).abs() <= cell + 0.05,
                "{wall} {drains}: {:?} cell {cell}",
                walls.measurement
            );
            assert_eq!(walls.count > 0, wall < 0.88, "{wall} {drains}: {} thin", walls.count);
            assert!(sealed(&work), "{wall} {drains}");
        }
    }

    /// A cylinder whose caps are fans from one rim vertex, as a triangulated n-gon comes out.
    fn fan_capped_cylinder(sides: u32, r: f32, h: f32) -> MeshPart {
        let mut positions = vec![];
        for z in [0.0, h] {
            for i in 0..sides {
                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                positions.push([r * a.cos() + r, r * a.sin() + r, z]);
            }
        }
        let mut tris = vec![];
        for i in 0..sides {
            let j = (i + 1) % sides;
            tris.push([i, j, sides + j]);
            tris.push([i, sides + j, sides + i]);
        }
        for i in 1..sides - 1 {
            tris.push([0, i + 1, i]);
            tris.push([sides, sides + i, sides + i + 1]);
        }
        let origin = Some((0..tris.len() as u32).map(|t| (t, ROOT)).collect());
        MeshPart { name: "fan".into(), positions, tris, origin }
    }

    #[test]
    fn drains_through_a_fan_capped_base_keep_the_winding() {
        for drains in [1, 2, 4] {
            let mut work = Mesh { parts: vec![fan_capped_cylinder(48, 22.0, 45.0)], ..Default::default() };
            let mut shape = work.clone();
            Mesh::hollow_out(&mut shape, &mut work, 2.0, (drains, 3.0), 1e-3);
            let r = work.hollow.unwrap();
            assert_eq!(r.drains, drains);
            let p = crate::printability::analyze(&work, crate::printability::Params::for_nozzle(0.4));
            let bad: Vec<_> = p
                .checks
                .iter()
                .filter(|c| c.count > 0 && c.code != "overhang" && c.code != "components")
                .map(|c| (&c.code, c.count))
                .collect();
            assert!(bad.is_empty(), "{drains}: {bad:?}");
            assert!(sealed(&work), "{drains}");
        }
    }

    /// A box of `size` at `at`, as triangles of its own in `part`.
    fn add_box(part: &mut MeshPart, size: Vec3, at: Vec3) {
        let base = part.positions.len() as u32;
        let cube = closed_cube();
        part.positions.extend(cube.parts[0].positions.iter().map(|p| std::array::from_fn(|k| p[k] * size[k] + at[k])));
        part.tris.extend(cube.parts[0].tris.iter().map(|t| t.map(|v| v + base)));
    }

    #[test]
    fn thickening_pushes_out_a_thin_plate_and_leaves_the_block_alone() {
        let mut part = MeshPart { name: "t".into(), ..Default::default() };
        add_box(&mut part, [10.0; 3], [0.0; 3]);
        add_box(&mut part, [10.0, 10.0, 0.5], [20.0, 0.0, 0.0]);
        let mut m = Mesh { parts: vec![part], ..Default::default() };
        let before = m.parts[0].positions.clone();
        let params = crate::printability::Params::for_nozzle(0.4);
        let need = params.wall_mm.max(params.extrusion_mm * params.perimeters as f32);
        let thin = |m: &Mesh| {
            crate::printability::analyze(m, params).checks.into_iter().find(|c| c.code == "thinWalls").unwrap()
        };
        assert!(thin(&m).count > 0);
        let r = m.thicken(need * 1.05);
        // The plate's eight corners went out, through its thin sides only; the block did not move at all.
        assert_eq!(r.moved_vertices, 8);
        assert!(m.parts[0].positions[..8] == before[..8]);
        assert!(r.max_offset > 0.1 && r.max_offset < need, "{r:?}");
        for (a, b) in m.parts[0].positions[8..].iter().zip(&before[8..]) {
            assert!(a[0] == b[0] && a[1] == b[1], "only along Z: {b:?} -> {a:?}");
        }
        // No wall under the minimum any more, and still closed.
        let after = thin(&m);
        assert_eq!(after.count, 0, "{:?}", after.measurement);
        assert!(sealed(&m));

        // A 0.6 mm pin standing on its own: thick enough after, and no fatter than it needs.
        let mut pin = crate::smooth::tests::cylinder(16);
        for p in &mut pin.positions {
            *p = [p[0] * 0.3, p[1] * 0.3, p[2] * 5.0];
        }
        let mut m = Mesh { parts: vec![pin], ..Default::default() };
        assert!(thin(&m).count > 0);
        let r = m.thicken(need * 1.05);
        assert_eq!(thin(&m).count, 0, "{r:?}");
        let radius = m.parts[0].positions.iter().map(|p| p[0].hypot(p[1])).fold(0.0, f32::max);
        assert!(radius * 2.0 < need * 1.6, "diameter {}", radius * 2.0);
        assert!(sealed(&m));
    }

    #[test]
    fn an_open_mesh_is_not_hollowed() {
        let mut work = big_cube(20.0);
        work.parts[0].tris.pop();
        let mut shape = work.clone();
        Mesh::hollow_out(&mut shape, &mut work, 2.0, (0, 0.0), 1e-3);
        assert_eq!(work.hollow.unwrap().refused, Some(crate::hollow::Refusal::Open));
        assert_eq!(work.parts[0].tris.len(), 11);
    }

    #[test]
    fn repair_welds_and_drops_bad_triangles() {
        let mut m = octahedron();
        let part = &mut m.parts[0];
        // A duplicate vertex, a triangle using it twice, and a repeated face.
        part.positions.push([0.0, 0.0, 0.0]);
        part.tris.push([6, 0, 1]);
        part.tris.push([1, 2, 0]);
        part.origin.as_mut().unwrap().extend([(0, ROOT), (0, ROOT)]);
        m.repair_all();
        assert_eq!(m.repair.welded_vertices, 1);
        assert_eq!(m.repair.degenerate_triangles, 1);
        assert_eq!(m.repair.duplicate_triangles, 1);
        assert_eq!(m.triangle_count(), 8);
        assert_eq!(m.parts[0].origin.as_ref().unwrap().len(), 8);
    }

    #[test]
    fn loose_pieces_go_and_the_body_stays() {
        let mut m = octahedron();
        let part = &mut m.parts[0];
        let base = part.positions.len() as u32;
        part.positions.extend([[5.0, 5.0, 0.0], [5.1, 5.0, 0.0], [5.0, 5.1, 0.0]]);
        part.tris.push([base, base + 1, base + 2]);
        part.origin.as_mut().unwrap().push((0, ROOT));
        m.remove_loose_pieces(0.5);
        assert_eq!(m.repair.loose_pieces, 1);
        assert_eq!(m.triangle_count(), 8);
        assert_eq!(m.parts[0].positions.len(), 6);
    }

    #[test]
    fn flatten_cuts_a_closed_flat_base() {
        let mut m = octahedron();
        assert!(m.contact_area(1e-4) < 1e-6);
        m.split_at(0.5);
        m.flatten(0.5);
        let (lo, hi) = m.bounds().unwrap();
        assert!(lo[2].abs() < 1e-6 && (hi[2] - 1.5).abs() < 1e-6);
        // The cross-section at the cut is a square of half-diagonal 0.5: area 0.5.
        let a = m.contact_area(1e-4);
        assert!((a - 0.5).abs() < 1e-3, "{a}");
        // Still closed: every edge has exactly two triangles.
        let adj = adjacency(&m.parts[0]);
        assert!(adj.iter().all(|n| n.len() == 3));
        // Every new triangle still knows where it came from.
        assert_eq!(m.parts[0].origin.as_ref().unwrap().len(), m.parts[0].tris.len());
    }

    #[test]
    fn origin_points_back_into_the_source_triangle() {
        let src = octahedron();
        let mut m = src.clone();
        m.split_at(0.5);
        let part = &m.parts[0];
        for (i, (s, corners)) in part.origin.as_ref().unwrap().iter().enumerate() {
            let sc = src.parts[0].corners(*s as usize);
            for (b, q) in corners.iter().zip(part.corners(i)) {
                assert!(geom::len(geom::sub(geom::at(&sc, *b), q)) < 1e-5);
            }
        }
    }

    #[test]
    fn simplify_reduces_a_dense_grid() {
        let n = 40;
        let mut positions = vec![];
        let mut tris = vec![];
        for y in 0..=n {
            for x in 0..=n {
                positions.push([x as f32, y as f32, 0.0]);
            }
        }
        let id = |x: usize, y: usize| (y * (n + 1) + x) as u32;
        for y in 0..n {
            for x in 0..n {
                tris.push([id(x, y), id(x + 1, y), id(x + 1, y + 1)]);
                tris.push([id(x, y), id(x + 1, y + 1), id(x, y + 1)]);
            }
        }
        let mut m = Mesh {
            parts: vec![MeshPart { name: "g".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        };
        let before = m.triangle_count();
        m.simplify(0.25);
        assert!(m.triangle_count() < before / 2, "{} -> {}", before, m.triangle_count());
    }

    /// A bumpy 41 × 41 grid, its left half one colour and its right half another.
    fn two_colour_grid() -> (Mesh, Vec<Vec<u8>>) {
        let n = 40;
        let id = |x: u32, y: u32| y * (n + 1) + x;
        let mut positions = vec![];
        for y in 0..=n {
            for x in 0..=n {
                let (fx, fy) = (x as f32, y as f32);
                positions.push([fx, fy, (fx * 0.3).sin() * (fy * 0.2).cos()]);
            }
        }
        let (mut tris, mut labels) = (vec![], vec![]);
        for y in 0..n {
            for x in 0..n {
                let l = u8::from(x >= n / 2);
                tris.push([id(x, y), id(x + 1, y), id(x + 1, y + 1)]);
                tris.push([id(x, y), id(x + 1, y + 1), id(x, y + 1)]);
                labels.extend([l, l]);
            }
        }
        let m = Mesh {
            parts: vec![MeshPart { name: "g".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        };
        (m, vec![labels])
    }

    #[test]
    fn protected_simplify_keeps_the_colour_border_and_reports_its_deviation() {
        let (m, labels) = two_colour_grid();
        let border = m.border_vertices(&labels);
        let on_border: Vec<Vec3> =
            m.parts[0].positions.iter().zip(&border[0]).filter(|(_, &b)| b).map(|(p, _)| *p).collect();
        // The column where the colours meet, x = 20.
        assert_eq!(on_border.len(), 41);
        assert!(on_border.iter().all(|p| p[0] == 20.0));
        let mut free = m.clone();
        free.simplify(0.3);
        let mut kept = m.clone();
        kept.simplify_with(0.3, Some(&border), false);
        assert!(kept.triangle_count() < m.triangle_count() / 2);
        let has = |mesh: &Mesh, q: Vec3| mesh.parts[0].positions.contains(&q);
        assert!(on_border.iter().all(|&q| has(&kept, q)), "every border vertex stays");
        assert!(!on_border.iter().all(|&q| has(&free, q)), "unprotected, some go");
        // A bumpy surface simplified strays a little; the same mesh does not stray at all.
        let d = m.deviation_to(&free);
        assert!(d > 0.0 && d < 1.0, "{d}");
        assert_eq!(m.deviation_to(&m), 0.0);
    }

    #[test]
    fn protecting_detail_keeps_the_peaks_of_a_relief() {
        // Flat on the left half, a sharp relief on the right half.
        let n = 40u32;
        let id = |x: u32, y: u32| y * (n + 1) + x;
        let mut positions = vec![];
        for y in 0..=n {
            for x in 0..=n {
                let z = if x > n / 2 && (x + y) % 4 == 0 { 0.6 } else { 0.0 };
                positions.push([x as f32, y as f32, z]);
            }
        }
        let mut tris = vec![];
        for y in 0..n {
            for x in 0..n {
                tris.push([id(x, y), id(x + 1, y), id(x + 1, y + 1)]);
                tris.push([id(x, y), id(x + 1, y + 1), id(x, y + 1)]);
            }
        }
        let m = Mesh {
            parts: vec![MeshPart { name: "r".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
            deviation: 0.0,
            hollow: None,
            thicken: None,
        };
        // Hard pressed, the plain quadric flattens peaks; with priority more of them stay.
        let peaks = |mesh: &Mesh| mesh.parts[0].positions.iter().filter(|p| p[2] > 0.5).count();
        let mut plain = m.clone();
        plain.simplify_with(0.1, None, false);
        let mut detail = m.clone();
        detail.simplify_with(0.1, None, true);
        assert!(peaks(&detail) > peaks(&plain), "{} vs {}", peaks(&detail), peaks(&plain));
    }
}
