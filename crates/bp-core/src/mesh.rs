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
}

impl Default for MeshOps {
    fn default() -> Self {
        Self { keep_ratio: 1.0, flatten_mm: 0.0, repair: true, min_piece_mm: 0.0 }
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
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub parts: Vec<MeshPart>,
    pub repair: RepairReport,
}

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
        Self { parts, repair: RepairReport::default() }
    }

    /// Builds the working mesh. Returns the shape before flattening (used to look
    /// up colours by position) and the final shape; both have the same triangles.
    /// `scale` converts source millimetres to final millimetres.
    pub fn build(scene: &Scene, ops: MeshOps, scale: f32) -> (Self, Self) {
        let mut shape = Self::from_scene(scene);
        if ops.repair {
            shape.repair_all();
        }
        if ops.min_piece_mm > 0.0 {
            shape.remove_loose_pieces(ops.min_piece_mm / scale);
        }
        if ops.keep_ratio < 0.999 {
            shape.simplify(ops.keep_ratio);
        }
        if ops.flatten_mm > 0.0 {
            shape.split_at(shape.bounds().map_or(0.0, |(lo, _)| lo[2]) + ops.flatten_mm / scale);
        }
        let mut work = shape.clone();
        if ops.flatten_mm > 0.0 {
            work.flatten(ops.flatten_mm / scale);
        }
        (shape, work)
    }

    /// Welds vertices that coincide (to a millionth of the model size), then
    /// drops triangles that collapsed or repeat another one.
    pub fn repair_all(&mut self) {
        let eps = self.bounds().map_or(1e-4, |(lo, hi)| (geom::len(geom::sub(hi, lo)) * 1e-6).max(1e-6));
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
                    if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || geom::area(&part.corners(i)) <= 0.0 {
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
        for part in &mut self.parts {
            let indices: Vec<u32> = part.tris.iter().flatten().copied().collect();
            let target = ((part.tris.len() as f32 * keep) as usize).max(4) * 3;
            let Ok(adapter) = meshopt::VertexDataAdapter::new(
                meshopt::typed_to_bytes(&part.positions),
                std::mem::size_of::<Vec3>(),
                0,
            ) else {
                continue;
            };
            let out = meshopt::simplify(&indices, &adapter, target, 0.02, meshopt::SimplifyOptions::None, None);
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
        Mesh { parts: vec![MeshPart { name: "o".into(), positions, tris, origin }], repair: RepairReport::default() }
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
        };
        let before = m.triangle_count();
        m.simplify(0.25);
        assert!(m.triangle_count() < before / 2, "{} -> {}", before, m.triangle_count());
    }
}
