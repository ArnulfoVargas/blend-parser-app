//! Local FDM screening in final millimetres. A report is evidence, never a print guarantee.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::bvh::Bvh;
use crate::geom::{self, Vec3};
use crate::mesh::Mesh;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Params {
    pub extrusion_mm: f32,
    pub perimeters: u32,
    pub wall_mm: f32,
    pub detail_mm: f32,
    pub component_mm: f32,
    pub overhang_deg: f32,
    pub contact_mm2: f32,
    pub tolerance_mm: f32,
    pub samples: usize,
}

impl Params {
    /// Editable starting points tied to extrusion width, not universal material limits.
    pub fn for_nozzle(nozzle: f32) -> Self {
        Self {
            extrusion_mm: nozzle * 1.1,
            perimeters: 2,
            wall_mm: nozzle * 2.2,
            detail_mm: nozzle * 1.1,
            component_mm: 2.0,
            overhang_deg: 45.0,
            contact_mm2: 4.0,
            tolerance_mm: 0.001,
            samples: 512,
        }
    }
    pub fn valid(&self) -> bool {
        [
            (self.extrusion_mm, 0.05, 3.0),
            (self.wall_mm, 0.05, 20.0),
            (self.detail_mm, 0.05, 20.0),
            (self.component_mm, 0.0, 100.0),
            (self.overhang_deg, 0.0, 90.0),
            (self.contact_mm2, 0.0, 10000.0),
            (self.tolerance_mm, 0.00001, 0.1),
        ]
        .iter()
        .all(|&(v, lo, hi)| v.is_finite() && v >= lo && v <= hi)
            && (1..=20).contains(&self.perimeters)
            && (16..=4096).contains(&self.samples)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub code: String,
    /// checked, estimated, or notEvaluated
    pub evidence: String,
    /// pass, warning, error, or incomplete
    pub severity: String,
    pub count: usize,
    pub measurement: Option<f32>,
    pub threshold: Option<f32>,
    /// Final-mm triangle centroids, bounded for UI responsiveness.
    pub locations: Vec<Vec3>,
}
impl Check {
    pub fn new(
        code: &str,
        evidence: &str,
        count: usize,
        severity: &str,
        measurement: Option<f32>,
        threshold: Option<f32>,
        locations: Vec<Vec3>,
    ) -> Self {
        Self {
            code: code.into(),
            evidence: evidence.into(),
            severity: if count == 0 { "pass" } else { severity }.into(),
            count,
            measurement,
            threshold,
            locations,
        }
    }
    fn unavailable(code: &str) -> Self {
        Self {
            code: code.into(),
            evidence: "notEvaluated".into(),
            severity: "incomplete".into(),
            count: 0,
            measurement: None,
            threshold: None,
            locations: vec![],
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThicknessSample {
    pub point: Vec3,
    pub mm: Option<f32>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub params: Params,
    pub checks: Vec<Check>,
    pub thickness: Vec<ThicknessSample>,
    pub triangles: usize,
    pub printer: Option<String>,
    pub nozzle_mm: Option<f32>,
    pub layer_mm: Option<f32>,
    pub size_mm: Vec3,
}

fn center(c: &[Vec3; 3]) -> Vec3 {
    geom::at(c, [1.0 / 3.0; 3])
}
fn collect(points: impl Iterator<Item = Vec3>) -> Vec<Vec3> {
    points.take(32).collect()
}

/// Mesh coordinates must already be final millimetres and placed at the plate.
/// Topology is checked per part after a quantized positional weld at `tolerance_mm`.
pub fn analyze(mesh: &Mesh, params: Params) -> Report {
    assert!(params.valid());
    let mut checks = vec![];
    let mut soup = mesh.soup();
    let bounds = mesh.bounds();
    let (lo, hi) = bounds.unwrap_or(([0.0; 3], [0.0; 3]));
    let size_mm = geom::sub(hi, lo);
    let mut open = vec![];
    let mut nonmanifold = vec![];
    let mut winding = vec![];
    let mut nonmanifold_vertices = vec![];
    let mut degenerate = vec![];
    let mut duplicate = vec![];
    let mut components = vec![];
    let mut small = vec![];
    let mut detail = vec![];
    let mut floating = vec![];
    let mut invalid_volume = vec![];
    let mut all_closed = !soup.is_empty();
    let mut positive_shells = 0;
    let mut negative_shells = 0;
    for part in &mesh.parts {
        let mut vertices = HashMap::<[i64; 3], u32>::new();
        let remap: Vec<u32> = part
            .positions
            .iter()
            .map(|p| {
                let next = vertices.len() as u32;
                *vertices.entry(p.map(|v| (v / params.tolerance_mm).round() as i64)).or_insert(next)
            })
            .collect();
        let mut edges = HashMap::<(u32, u32), Vec<(usize, bool)>>::new();
        let mut faces = HashSet::new();
        let mut welded = part.clone();
        // Components use welded positions so exported seams do not look disconnected.
        welded.positions = vec![[0.0; 3]; vertices.len()];
        for (i, &v) in remap.iter().enumerate() {
            welded.positions[v as usize] = part.positions[i];
        }
        welded.tris = part.tris.iter().map(|t| t.map(|v| remap[v as usize])).collect();
        for (ti, t) in welded.tris.iter().enumerate() {
            let c = part.corners(ti);
            if geom::area(&c) <= params.tolerance_mm * params.tolerance_mm
                || t[0] == t[1]
                || t[1] == t[2]
                || t[2] == t[0]
            {
                degenerate.push(center(&c));
            }
            let mut key = *t;
            key.sort_unstable();
            if !faces.insert(key) {
                duplicate.push(center(&c));
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edges.entry((a.min(b), a.max(b))).or_default().push((ti, a < b));
            }
        }
        // The link of a manifold vertex is a single cycle (interior) or path (boundary).
        let mut links = vec![vec![]; welded.positions.len()];
        for t in &welded.tris {
            for k in 0..3 {
                links[t[k] as usize].push((t[(k + 1) % 3], t[(k + 2) % 3]));
            }
        }
        for (vi, link) in links.iter().enumerate() {
            if link.is_empty() {
                continue;
            }
            let mut graph = HashMap::<u32, Vec<u32>>::new();
            for &(a, b) in link {
                graph.entry(a).or_default().push(b);
                graph.entry(b).or_default().push(a);
            }
            let first = *graph.keys().next().unwrap();
            let mut visited = HashSet::new();
            let mut stack = vec![first];
            while let Some(v) = stack.pop() {
                if visited.insert(v) {
                    stack.extend(&graph[&v]);
                }
            }
            let ends = graph.values().filter(|n| n.len() == 1).count();
            if visited.len() != graph.len() || graph.values().any(|n| n.len() > 2) || (ends != 0 && ends != 2) {
                nonmanifold_vertices.push(welded.positions[vi]);
            }
        }
        let mut closed = true;
        for entries in edges.values() {
            let at = center(&part.corners(entries[0].0));
            if entries.len() == 1 {
                open.push(at);
                closed = false;
            }
            if entries.len() > 2 {
                nonmanifold.push(at);
                closed = false;
            }
            if entries.len() == 2 && entries[0].1 == entries[1].1 {
                winding.push(at);
                closed = false;
            }
        }
        let ids = welded.components();
        let n = ids.iter().copied().max().map_or(0, |v| v as usize + 1);
        let mut bb = vec![([f32::MAX; 3], [f32::MIN; 3]); n];
        let mut volume = vec![0.0f64; n];
        for (ti, &id) in ids.iter().enumerate() {
            let c = part.corners(ti);
            let id = id as usize;
            for p in c {
                for (k, &v) in p.iter().enumerate() {
                    bb[id].0[k] = bb[id].0[k].min(v);
                    bb[id].1[k] = bb[id].1[k].max(v);
                }
            }
            // Translate near the component to avoid cancellation at large world offsets.
            let a = geom::sub(c[0], lo);
            let b = geom::sub(c[1], lo);
            let d = geom::sub(c[2], lo);
            volume[id] += geom::dot(a, geom::cross(b, d)) as f64 / 6.0;
        }
        for (i, &(a, b)) in bb.iter().enumerate() {
            let at = geom::scale(geom::add(a, b), 0.5);
            components.push(at);
            let diagonal = geom::len(geom::sub(b, a));
            if diagonal < params.component_mm {
                small.push(at);
            }
            if (0..3).map(|k| b[k] - a[k]).fold(f32::MAX, f32::min) < params.detail_mm.max(params.extrusion_mm) {
                detail.push(at);
            }
            if a[2] > lo[2] + params.tolerance_mm {
                floating.push(at);
            }
            if closed && volume[i] > 0.0 {
                positive_shells += 1;
            }
            if closed && volume[i] < 0.0 {
                negative_shells += 1;
            }
            if closed && volume[i] <= (params.tolerance_mm as f64).powi(3) {
                invalid_volume.push(at);
            }
        }
        all_closed &= closed;
    }
    for (code, points) in [
        ("openEdges", open),
        ("nonManifoldEdges", nonmanifold),
        ("winding", winding),
        ("degenerate", degenerate),
        ("duplicate", duplicate),
    ] {
        checks.push(Check::new(
            code,
            "checked",
            points.len(),
            "error",
            Some(points.len() as f32),
            Some(0.0),
            collect(points.into_iter()),
        ));
    }
    checks.push(Check::new(
        "nonManifoldVertices",
        "checked",
        nonmanifold_vertices.len(),
        "error",
        None,
        Some(0.0),
        collect(nonmanifold_vertices.into_iter()),
    ));
    // Oppositely oriented shells can be valid cavity boundaries. Without robust
    // nesting/intersection classification, their orientation expectation is unknown.
    let mixed_shells = positive_shells > 0 && negative_shells > 0;
    if all_closed && !mixed_shells {
        checks.push(Check::new(
            "volume",
            "checked",
            invalid_volume.len(),
            "error",
            None,
            None,
            collect(invalid_volume.into_iter()),
        ));
    } else {
        checks.push(Check::unavailable("volume"));
    }
    checks.push(Check::new(
        "components",
        "checked",
        components.len().saturating_sub(1),
        "warning",
        Some(components.len() as f32),
        Some(1.0),
        collect(components.into_iter()),
    ));
    checks.push(Check::new(
        "smallComponents",
        "estimated",
        small.len(),
        "warning",
        None,
        Some(params.component_mm),
        collect(small.into_iter()),
    ));
    checks.push(Check::new(
        "smallDetails",
        "estimated",
        detail.len(),
        "warning",
        None,
        Some(params.detail_mm.max(params.extrusion_mm)),
        collect(detail.into_iter()),
    ));
    checks.push(Check::new(
        "floating",
        "estimated",
        floating.len(),
        "warning",
        None,
        Some(params.tolerance_mm),
        collect(floating.into_iter()),
    ));
    let mut contact = 0.0;
    let mut resting = vec![];
    let mut overhang = vec![];
    for c in &soup.corners {
        let normal = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
        if c.iter().all(|p| p[2] <= lo[2] + params.tolerance_mm) {
            if normal[2] < 0.0 {
                contact += geom::area(c);
                resting.push(center(c));
            }
        } else if normal[2] < 0.0 && (-normal[2]).asin().to_degrees() > params.overhang_deg {
            overhang.push(center(c));
        }
    }
    checks.push(Check::new(
        "contact",
        "estimated",
        usize::from(contact < params.contact_mm2),
        "warning",
        Some(contact),
        Some(params.contact_mm2),
        // Where the model does touch; with no contact there is nothing to point at.
        collect(resting.into_iter()),
    ));
    checks.push(Check::new(
        "overhang",
        "estimated",
        overhang.len(),
        "warning",
        None,
        Some(params.overhang_deg),
        collect(overhang.into_iter()),
    ));
    let mut thickness = vec![];
    let reliable = !mixed_shells
        && all_closed
        && checks
            .iter()
            .filter(|c| ["degenerate", "duplicate", "volume", "nonManifoldVertices"].contains(&c.code.as_str()))
            .all(|c| c.count == 0);
    if reliable {
        let bvh = Bvh::build(&soup);
        let stride = soup.len().div_ceil(params.samples).max(1);
        let mut thin = vec![];
        let mut minimum = f32::MAX;
        let threshold = params.wall_mm.max(params.extrusion_mm * params.perimeters as f32);
        for c in soup.corners.iter().step_by(stride) {
            let at = center(c);
            let inward = geom::normalize(geom::cross(geom::sub(c[2], c[0]), geom::sub(c[1], c[0])));
            let offset = params.tolerance_mm * 2.0;
            let hit = bvh.raycast(&soup, geom::add(at, geom::scale(inward, offset)), inward);
            let mm = hit.map(|h| h.t + offset).filter(|v| v.is_finite() && *v > offset);
            if let Some(mm) = mm {
                minimum = minimum.min(mm);
                if mm < threshold {
                    thin.push(at);
                }
            }
            thickness.push(ThicknessSample { point: at, mm });
        }
        checks.push(Check::new(
            "thinWalls",
            "estimated",
            thin.len(),
            "warning",
            (minimum < f32::MAX).then_some(minimum),
            Some(threshold),
            collect(thin.into_iter()),
        ));
        if thickness.iter().any(|s| s.mm.is_none()) {
            checks.push(Check::unavailable("thicknessCoverage"));
        }
    } else {
        checks.push(Check::unavailable("thinWalls"));
    }
    for code in ["selfIntersections", "bridges", "layerSupport", "fragileConnections", "partSpacing"] {
        checks.push(Check::unavailable(code));
    }
    // Keep the soup allocation scoped here; sampling costs O(S log T), not all pairs.
    soup.corners.clear();
    Report {
        params,
        checks,
        thickness,
        triangles: mesh.triangle_count(),
        printer: None,
        nozzle_mm: None,
        layer_mm: None,
        size_mm,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::MeshPart;

    // A reference cuboid, outward winding, with known volume/plate area/wall dimensions.
    fn cuboid(size: Vec3, offset: Vec3) -> MeshPart {
        let positions = [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ]
        .map(|p| std::array::from_fn(|k| p[k] * size[k] + offset[k]))
        .to_vec();
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
        MeshPart { name: "reference cuboid".into(), positions, tris, origin: None }
    }
    fn mesh(part: MeshPart) -> Mesh {
        Mesh { parts: vec![part], ..Default::default() }
    }
    fn check<'a>(r: &'a Report, code: &str) -> &'a Check {
        r.checks.iter().find(|c| c.code == code).unwrap()
    }
    fn params() -> Params {
        Params::for_nozzle(0.4)
    }

    #[test]
    fn known_closed_box_has_contact_and_samples_in_mm() {
        let r = analyze(&mesh(cuboid([10., 20., 30.], [0.; 3])), params());
        for code in
            ["openEdges", "nonManifoldEdges", "winding", "degenerate", "duplicate", "volume", "nonManifoldVertices"]
        {
            assert_eq!(check(&r, code).severity, "pass", "{code}");
        }
        assert_eq!(check(&r, "contact").measurement, Some(200.));
        assert!(check(&r, "contact").locations.iter().all(|p| p[2] == 0.));
        assert_eq!(r.size_mm, [10., 20., 30.]);
        assert_eq!(r.thickness.len(), 12);
        assert!(r.thickness.iter().all(|s| s.mm.is_some_and(|mm| (10.0..=30.001).contains(&mm))));
    }

    #[test]
    fn open_box_detects_boundary_and_does_not_certify_volume_or_thickness() {
        let mut b = cuboid([10.; 3], [0.; 3]);
        b.tris.drain(2..4);
        let r = analyze(&mesh(b), params());
        assert_eq!(check(&r, "openEdges").count, 4);
        assert_eq!(check(&r, "volume").evidence, "notEvaluated");
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
    }

    #[test]
    fn duplicate_degenerate_and_nonmanifold_are_independent_defects() {
        let mut b = cuboid([10.; 3], [0.; 3]);
        b.tris.push(b.tris[0]);
        b.tris.push([0, 0, 1]);
        let r = analyze(&mesh(b), params());
        assert_eq!(check(&r, "duplicate").count, 1);
        assert_eq!(check(&r, "degenerate").count, 1);
        assert!(check(&r, "nonManifoldEdges").count >= 3);
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
    }

    #[test]
    fn flipped_face_and_inverted_shell_are_reported() {
        let mut b = cuboid([10.; 3], [0.; 3]);
        b.tris[0].swap(1, 2);
        let r = analyze(&mesh(b), params());
        assert_eq!(check(&r, "winding").count, 3);
        let mut b = cuboid([10.; 3], [0.; 3]);
        for t in &mut b.tris {
            t.swap(1, 2);
        }
        let r = analyze(&mesh(b), params());
        assert_eq!(check(&r, "volume").count, 1);
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
    }

    #[test]
    fn final_scale_and_nozzle_change_wall_findings_coherently() {
        let r = analyze(&mesh(cuboid([0.6, 10., 10.], [0.; 3])), params());
        assert!(check(&r, "thinWalls").count > 0);
        assert_eq!(check(&r, "thinWalls").measurement.map(|m| (m * 10.).round()), Some(6.));
        let r = analyze(&mesh(cuboid([1.2, 20., 20.], [0.; 3])), params());
        assert_eq!(check(&r, "thinWalls").count, 0);
        let r = analyze(&mesh(cuboid([0.6, 10., 10.], [0.; 3])), Params::for_nozzle(0.2));
        assert_eq!(check(&r, "thinWalls").count, 0);
        let p = Params { wall_mm: 1.5, ..params() };
        let r = analyze(&mesh(cuboid([1.2, 20., 20.], [0.; 3])), p);
        assert!(check(&r, "thinWalls").count > 0);
    }

    #[test]
    fn isolated_small_component_and_unsupported_underside_are_locatable() {
        let m = Mesh { parts: vec![cuboid([10.; 3], [0.; 3]), cuboid([0.1; 3], [20., 0., 15.])], ..Default::default() };
        let r = analyze(&m, params());
        for code in ["components", "smallComponents", "smallDetails", "floating", "overhang"] {
            assert!(check(&r, code).count > 0, "{code}");
            assert!(!check(&r, code).locations.is_empty());
        }
        assert_eq!(check(&r, "bridges").evidence, "notEvaluated");
        assert_eq!(check(&r, "layerSupport").evidence, "notEvaluated");
    }

    #[test]
    fn intersecting_closed_boxes_do_not_receive_an_intersection_pass() {
        let m = Mesh { parts: vec![cuboid([10.; 3], [0.; 3]), cuboid([10.; 3], [5.; 3])], ..Default::default() };
        let r = analyze(&m, params());
        assert_eq!(check(&r, "selfIntersections").severity, "incomplete");
        assert_eq!(check(&r, "selfIntersections").evidence, "notEvaluated");
    }

    #[test]
    fn empty_mesh_is_not_a_valid_volume() {
        let r = analyze(&Mesh::default(), params());
        assert_eq!(check(&r, "volume").evidence, "notEvaluated");
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
    }

    #[test]
    fn large_fixture_keeps_sampling_and_locations_bounded() {
        // 24,000 triangles, intentionally separate solids. This catches unbounded ray-pair loops.
        let m =
            Mesh { parts: (0..2000).map(|i| cuboid([1.; 3], [i as f32 * 2., 0., 0.])).collect(), ..Default::default() };
        let r = analyze(&m, Params { samples: 64, ..params() });
        assert_eq!(r.triangles, 24_000);
        assert!(r.thickness.len() <= 64);
        assert!(r.checks.iter().all(|c| c.locations.len() <= 32));
        assert_eq!(check(&r, "components").count, 1999);
    }

    #[test]
    fn parameters_reject_nonfinite_and_unbounded_work() {
        assert!(!Params { wall_mm: f32::NAN, ..params() }.valid());
        assert!(!Params { samples: 1_000_000, ..params() }.valid());
        assert!(!Params { tolerance_mm: 0., ..params() }.valid());
    }
    #[test]
    fn bow_tie_vertex_is_not_misreported_as_manifold() {
        let mut a = cuboid([1.; 3], [0.; 3]);
        let b = cuboid([1.; 3], [1.; 3]);
        let offset = a.positions.len() as u32;
        a.positions.extend(b.positions);
        a.tris.extend(b.tris.iter().map(|t| t.map(|v| v + offset)));
        let r = analyze(&mesh(a), params());
        assert_eq!(check(&r, "nonManifoldEdges").count, 0);
        assert_eq!(check(&r, "nonManifoldVertices").count, 1);
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
    }
    #[test]
    fn hollow_solid_with_reversed_cavity_is_incomplete_rather_than_invalid() {
        let mut outer = cuboid([10.; 3], [0.; 3]);
        let mut inner = cuboid([8.; 3], [1.; 3]);
        for t in &mut inner.tris {
            t.swap(1, 2);
        }
        let offset = outer.positions.len() as u32;
        outer.positions.extend(inner.positions);
        outer.tris.extend(inner.tris.iter().map(|t| t.map(|v| v + offset)));
        let r = analyze(&mesh(outer), params());
        assert_eq!(check(&r, "volume").evidence, "notEvaluated");
        assert_eq!(check(&r, "volume").severity, "incomplete");
        assert_eq!(check(&r, "thinWalls").evidence, "notEvaluated");
        assert_eq!(check(&r, "winding").severity, "pass");
    }
}
