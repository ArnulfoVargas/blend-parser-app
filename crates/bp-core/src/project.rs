//! The open document and its use cases: palette, size, mesh operations, colour
//! clean-up, hand painting with undo, preview data and export data.

use crate::bvh::{Bvh, RayHit, TriSoup};
use crate::color::{to_u8, Classifier, Rgb};
use crate::cut;
use crate::encode;
use crate::estimate::{self, SwapEstimate};
use crate::field::ColorField;
use crate::geom::{self, Vec3, ROOT};
use crate::mesh::{Mesh, MeshOps, MeshPart, RepairReport};
use crate::optimize;
use crate::paint::{self, Node, PaintParams};
use crate::palette;
use crate::scene::{Part, Scene};
use crate::separate;
use crate::split::{self, ColorPiece};
use std::collections::HashMap;

/// Colour settings, in final millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorOps {
    /// Subdivision depth at colour borders (0 to 5).
    pub border_detail: u8,
    /// Islands smaller than this many mm² take their neighbour's colour; 0 disables it.
    pub min_island_mm2: f32,
}

impl Default for ColorOps {
    fn default() -> Self {
        Self { border_detail: 5, min_island_mm2: 0.0 }
    }
}

/// What the printer can draw, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintOps {
    pub nozzle_mm: f32,
    pub layer_mm: f32,
}

impl Default for PrintOps {
    fn default() -> Self {
        Self { nozzle_mm: 0.4, layer_mm: 0.2 }
    }
}

/// What the preview needs, flattened for transfer to the UI.
pub struct Preview {
    /// xyz of each leaf triangle's three corners.
    pub positions: Vec<f32>,
    /// Filament slot of each leaf.
    pub labels: Vec<u8>,
    /// Original model colour at each leaf's centre (sRGB bytes).
    pub source_rgb: Vec<u8>,
    /// Leaves of global triangle `i` are `offsets[i]..offsets[i + 1]`.
    pub offsets: Vec<u32>,
}

pub struct ExportPart {
    pub name: String,
    /// The printed object it belongs to (see [`Part::object`]).
    pub object: u32,
    /// Final millimetres, base on Z = 0, centred on XY.
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub paint: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub source_triangles: usize,
    pub triangles: usize,
    pub split_triangles: usize,
    /// Triangles per filament slot, by dominant colour.
    pub per_slot: Vec<usize>,
    pub size_mm: Vec3,
    pub contact_mm2: f32,
    pub swaps: SwapEstimate,
    /// For each slot, the closest other slot and their ΔE.
    pub nearest: Vec<Option<(usize, f32)>>,
    pub repair: RepairReport,
}

/// A disconnected piece of the model (see [`Project::detect_pieces`]).
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedPiece {
    /// The scene part it is in.
    pub part: usize,
    pub triangles: usize,
    pub size_mm: Vec3,
    /// Its box's centre, in preview coordinates.
    pub centre: Vec3,
    /// Its surface is closed and consistently wound.
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeparateError {
    /// The pieces asked for are not the ones the model has now.
    Stale,
    /// Every piece would stay in the object it is in.
    NothingToSeparate,
}

/// A plane worth reviewing as a cut (see [`Project::propose_cuts`]).
#[derive(Clone, Debug, PartialEq)]
pub struct CutProposal {
    pub part: usize,
    pub axis: usize,
    /// Preview coordinates, as the cut tool takes them.
    pub point: Vec3,
    pub normal: Vec3,
    /// Areas in final square millimetres.
    pub signal: separate::Signal,
}

#[derive(Clone)]
struct Change {
    part: u32,
    tri: u32,
    node: Node,
    /// Hand-painted label before the change (NO_LABEL when none).
    painted: u8,
}

/// Everything a slot merge changes, to undo it.
#[derive(Clone)]
struct SlotSnapshot {
    palette: Vec<Rgb>,
    targets: Vec<(Rgb, u8)>,
    trees: Vec<Vec<Node>>,
    painted: Vec<Vec<u8>>,
    cut_painted: Vec<Vec<Vec<u8>>>,
}

#[derive(Clone)]
enum UndoEntry {
    Stroke(Vec<Change>),
    Slots(Box<SlotSnapshot>),
    Geometry(Box<Project>),
    Cut,
}

const NO_LABEL: u8 = u8::MAX;

/// Rows of a 3×3 rotation.
pub type Mat3 = [[f32; 3]; 3];
pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn mat_vec(m: Mat3, v: Vec3) -> Vec3 {
    m.map(|row| geom::dot(row, v))
}

fn mat_mul(a: Mat3, b: Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// The inverse of a rotation.
fn transpose(m: &Mat3) -> Mat3 {
    std::array::from_fn(|r| std::array::from_fn(|c| m[c][r]))
}

/// Shortest rotation taking one unit normal to another, including opposite normals.
fn rotation_between(from: Vec3, to: Vec3) -> Mat3 {
    let c = geom::dot(from, to).clamp(-1.0, 1.0);
    if c > 1.0 - 1e-6 {
        return IDENTITY;
    }
    if c < -1.0 + 1e-6 {
        let seed = if from[0].abs() < 0.8 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let axis = geom::normalize(geom::cross(from, seed));
        return std::array::from_fn(|i| std::array::from_fn(|j| 2.0 * axis[i] * axis[j] - IDENTITY[i][j]));
    }
    let v = geom::cross(from, to);
    let k = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let k2 = mat_mul(k, k);
    std::array::from_fn(|i| std::array::from_fn(|j| IDENTITY[i][j] + k[i][j] + k2[i][j] / (1.0 + c)))
}

/// How a turn moved the preview's positions, `p' = matrix · p + shift`, when
/// it could be applied to them as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidMove {
    pub matrix: Mat3,
    pub shift: Vec3,
}

/// What a cut replaced, to undo it.
#[derive(Clone)]
struct CutSnapshot {
    parts: Vec<Part>,
    unrotated: Vec<Vec<Vec3>>,
    painted: Vec<Vec<u8>>,
    undo: Vec<UndoEntry>,
    /// What the undo button calls it: "cut", "separate"…
    kind: &'static str,
}

/// Transfers brush labels within one part. Exact source ancestry takes priority;
/// simplified geometry uses the same closest-surface rule as source colours.
struct PaintTransfer {
    soup: TriSoup,
    bvh: Bvh,
    labels: Vec<u8>,
    by_source: Vec<Option<u8>>,
}

impl PaintTransfer {
    fn new(part: &MeshPart, labels: &[u8], source_count: usize) -> Self {
        let mut by_source = vec![None; source_count];
        if let Some(origins) = &part.origin {
            // A removed source face has no brush mark to transfer.
            by_source.fill(Some(NO_LABEL));
            let mut seen = vec![false; source_count];
            for ((src, _), &label) in origins.iter().zip(labels) {
                let i = *src as usize;
                if !seen[i] {
                    by_source[i] = Some(label);
                    seen[i] = true;
                } else if by_source[i] != Some(label) {
                    by_source[i] = None;
                }
            }
        }
        let mut soup = TriSoup::default();
        if part.origin.is_none() || by_source.iter().any(Option::is_none) {
            for ti in 0..part.tris.len() {
                soup.push(part.corners(ti), 0, ti as u32);
            }
        }
        Self { bvh: Bvh::build(&soup), soup, labels: labels.to_vec(), by_source }
    }

    fn label(&self, source: Option<u32>, point: Vec3) -> u8 {
        if let Some(label) = source.and_then(|s| self.by_source.get(s as usize).copied().flatten()) {
            return label;
        }
        self.bvh.nearest(&self.soup, point).map_or(NO_LABEL, |(t, _)| self.labels[t as usize])
    }
}

/// A cut piece's surface before it is moved, turned or rebuilt. The working
/// mesh's barycentric origins can still locate its brush marks in this frame.
struct CutPaint {
    part: usize,
    origins: Vec<Option<u32>>,
    corners: Vec<[Vec3; 3]>,
}

/// Turn of `degrees` about X, Y or Z, counter-clockwise looking down the axis.
/// Quarter turns are exact, so turning back leaves no drift.
fn axis_turn(axis: usize, degrees: f32) -> Mat3 {
    let quarter = degrees / 90.0;
    let (s, c) = if (quarter - quarter.round()).abs() < 1e-6 {
        match (quarter.round() as i64).rem_euclid(4) {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        }
    } else {
        degrees.to_radians().sin_cos()
    };
    match axis {
        0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
        1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
        _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// The rotation taking unit `d` to straight down (-Z), turning about `d × -Z`.
fn onto_down(d: Vec3) -> Mat3 {
    let down = [0.0, 0.0, -1.0];
    let c = geom::dot(d, down);
    if c > 1.0 - 1e-6 {
        return IDENTITY;
    }
    if c < -1.0 + 1e-6 {
        return [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
    }
    let v = geom::cross(d, down);
    let k = 1.0 / (1.0 + c);
    let x = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let xx = mat_mul(x, x);
    std::array::from_fn(|i| std::array::from_fn(|j| IDENTITY[i][j] + x[i][j] + xx[i][j] * k))
}

/// Centre of the enclosed volume, or of the surface when it encloses none
/// (open or inside-out meshes), as a stand-in for the centre of mass.
fn mass_centre(tris: &[[Vec3; 3]], faces: &[(Vec3, f32)], origin: Vec3) -> Vec3 {
    let (mut volume, mut sum) = (0.0f64, [0.0f64; 3]);
    for c in tris {
        let [a, b, d] = c.map(|p| geom::sub(p, origin));
        let v = geom::dot(a, geom::cross(b, d)) as f64 / 6.0;
        volume += v;
        for k in 0..3 {
            sum[k] += v * (a[k] + b[k] + d[k]) as f64 / 4.0;
        }
    }
    if volume.abs() > 1e-12 {
        return std::array::from_fn(|k| (sum[k] / volume) as f32 + origin[k]);
    }
    let area: f32 = faces.iter().map(|f| f.1).sum();
    let mut c = [0.0; 3];
    for (t, f) in tris.iter().zip(faces) {
        c = geom::add(c, geom::scale(geom::add(geom::add(t[0], t[1]), t[2]), f.1 / 3.0));
    }
    geom::scale(c, 1.0 / area.max(f32::MIN_POSITIVE))
}

/// Whether `centre` falls inside the convex footprint of `feet` (the points
/// touching the plate) seen from `up`, at least `margin` from its border.
fn stands(feet: &[Vec3], centre: Vec3, up: Vec3, margin: f32) -> bool {
    let u = geom::normalize(geom::cross(up, if up[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }));
    let v = geom::cross(up, u);
    let flat = |p: Vec3| [geom::dot(p, u), geom::dot(p, v)];
    let mut pts: Vec<[f32; 2]> = feet.iter().map(|&p| flat(p)).collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return false;
    }
    // Monotone chain, counter-clockwise.
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f32; 2]> = vec![];
    for pass in 0..2 {
        let start = hull.len();
        let points: Box<dyn Iterator<Item = &[f32; 2]>> =
            if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in points {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    if hull.len() < 3 {
        return false;
    }
    let c = flat(centre);
    (0..hull.len()).all(|i| {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        len > 0.0 && cross(a, b, c) / len >= margin
    })
}

#[derive(Clone)]
pub struct Project {
    scene: Scene,
    /// Scene positions as the file has them, placed on the plate; `scene` holds
    /// them stretched by `stretch`.
    source_positions: Vec<Vec<Vec3>>,
    /// Width, depth and height of the scene before any resizing.
    source_size: Vec3,
    source_height: f32,
    scale: f32,
    /// Extra scale per axis on top of `scale`, relative to Z (always 1 on Z).
    stretch: Vec3,
    /// Size the user asked for (before any base is flattened away). Kept as
    /// typed so no rounding builds up from one change to the next.
    size_mm: Vec3,
    /// Sizes before each resize, for undo.
    size_history: Vec<Vec3>,
    /// Scene positions as loaded and placed, before `rotation`.
    unrotated: Vec<Vec<Vec3>>,
    /// Rows of the rotation applied to the model as loaded.
    rotation: Mat3,
    /// Where the rotated positions were moved to sit on the plate.
    placement: Vec3,
    /// Parts before each cut, for undo.
    cut_history: Vec<CutSnapshot>,
    mesh_ops: MeshOps,
    color_ops: ColorOps,
    print_ops: PrintOps,
    palette: Vec<Rgb>,
    /// Colours that classify into each slot: the slot's own colour plus the
    /// colours of any slot merged into it.
    targets: Vec<(Rgb, u8)>,
    shape: Mesh,
    work: Mesh,
    trees: Vec<Vec<Node>>,
    /// Triangle neighbours of each part of `work`.
    adjacency: Vec<Vec<Vec<(u32, f32)>>>,
    /// Hand-painted label per triangle, NO_LABEL where the model's colour applies.
    painted: Vec<Vec<u8>>,
    picker: (Bvh, TriSoup),
    tri_offsets: Vec<u32>,
    stroke: Vec<Change>,
    undo: Vec<UndoEntry>,
}

impl Project {
    pub fn new(mut scene: Scene, filaments: usize) -> Self {
        scene.place_on_plate();
        let source_size: Vec3 =
            scene.bounds().map_or([1.0; 3], |(lo, hi)| std::array::from_fn(|i| (hi[i] - lo[i]).max(1e-6)));
        let source_positions: Vec<Vec<Vec3>> = scene.parts.iter().map(|p| p.positions.clone()).collect();
        let mut p = Self {
            unrotated: source_positions.clone(),
            rotation: IDENTITY,
            placement: [0.0; 3],
            cut_history: vec![],
            scene,
            source_positions,
            source_size,
            source_height: source_size[2],
            scale: 1.0,
            stretch: [1.0; 3],
            size_mm: source_size,
            size_history: vec![],
            mesh_ops: MeshOps::default(),
            color_ops: ColorOps::default(),
            print_ops: PrintOps::default(),
            palette: vec![],
            targets: vec![],
            shape: Mesh::default(),
            work: Mesh::default(),
            trees: vec![],
            adjacency: vec![],
            painted: vec![],
            picker: (Bvh::default(), TriSoup::default()),
            tri_offsets: vec![],
            stroke: vec![],
            undo: vec![],
        };
        p.rebuild_geometry();
        let pal = p.suggest_palette(filaments);
        p.targets = pal.iter().enumerate().map(|(i, &c)| (c, i as u8)).collect();
        p.palette = pal;
        p.rebuild_paint();
        p
    }

    /// Current working geometry in final millimetres, before colour subdivision.
    pub fn printability(&self, params: crate::printability::Params) -> crate::printability::Report {
        let mut mesh = self.work.clone();
        for part in &mut mesh.parts {
            for p in &mut part.positions {
                *p = geom::scale(*p, self.scale);
            }
        }
        let mut report = crate::printability::analyze(&mesh, params);
        report.nozzle_mm = Some(self.print_ops.nozzle_mm);
        report.layer_mm = Some(self.print_ops.layer_mm);
        report
    }

    /// Applies mesh operations as one undoable step that restores the whole
    /// document: geometry, colours and hand painting. The printability panel's
    /// fixes and their reverts both come through here.
    pub fn fix_mesh(&mut self, ops: MeshOps) {
        if ops == self.mesh_ops {
            return;
        }
        self.end_stroke();
        let history = std::mem::take(&mut self.undo);
        let mut previous = self.clone();
        previous.undo = history;
        self.set_mesh_ops(ops);
        self.undo.push(UndoEntry::Geometry(Box::new(previous)));
    }

    pub fn rotation(&self) -> Mat3 {
        self.rotation
    }

    pub fn is_rotated(&self) -> bool {
        self.rotation != IDENTITY
    }

    /// Turns the model as loaded by `r` (absolute, not added to the current
    /// turn) and places it back on the plate at the same scale, as one undoable
    /// step. Hand painting survives: the triangles stay the same.
    pub fn set_rotation(&mut self, r: Mat3) {
        if r == self.rotation {
            return;
        }
        self.end_stroke();
        let history = std::mem::take(&mut self.undo);
        let mut previous = self.clone();
        previous.undo = history;
        // A stretch was along the old axes; the turned model keeps its scale only.
        let stretched = self.stretch != [1.0; 3];
        self.stretch = [1.0; 3];
        self.turn_to(r, stretched);
        self.undo.push(UndoEntry::Geometry(Box::new(previous)));
    }

    // ---- orientation ------------------------------------------------------

    /// Turns the model `degrees` about the X, Y or Z axis through its centre;
    /// it lands back on the plate, centred, its lowest point on Z = 0.
    pub fn rotate(&mut self, axis: usize, degrees: f32) -> Option<RigidMove> {
        if axis > 2 || !degrees.is_finite() || degrees.rem_euclid(360.0).abs() < 1e-6 {
            return None;
        }
        self.turn_to(mat_mul(axis_turn(axis, degrees), self.rotation), false)
    }

    /// Back to the orientation the file has.
    pub fn reset_rotation(&mut self) -> Option<RigidMove> {
        self.turn_to(IDENTITY, false)
    }

    /// The scale and per-axis stretch stay as they are, so the model keeps its
    /// size along each plate axis as far as the new bounds allow. Earlier sizes
    /// belong to the old orientation, so they can no longer be undone.
    ///
    /// Unstretched and with no flattened base, the model turns as a whole: its
    /// triangles, colours and hand painting stay and only positions move, and
    /// the move comes back so the preview can follow without being sent again.
    /// `rebuild` when the scene's positions no longer match the current
    /// stretch, so they cannot simply be moved.
    fn turn_to(&mut self, rotation: Mat3, rebuild: bool) -> Option<RigidMove> {
        if rotation == self.rotation {
            return None;
        }
        // Pieces dropped to the plate fell along the old down, and a flattened base was cut along it.
        if !rebuild && self.stretch == [1.0; 3] && self.mesh_ops.flatten_mm <= 0.0 && !self.mesh_ops.drop_floating {
            let turn = mat_mul(rotation, transpose(&self.rotation));
            for part in &mut self.scene.parts {
                for v in &mut part.positions {
                    *v = mat_vec(turn, *v);
                }
            }
            let shift = self.scene.place_on_plate();
            for mesh in [&mut self.shape, &mut self.work] {
                for v in mesh.parts.iter_mut().flat_map(|p| p.positions.iter_mut()) {
                    *v = geom::add(mat_vec(turn, *v), shift);
                }
            }
            self.rotation = rotation;
            self.placement = geom::add(mat_vec(turn, self.placement), shift);
            self.source_positions = self.scene.parts.iter().map(|p| p.positions.clone()).collect();
            self.measure_source();
            let soup = self.work.soup();
            self.picker = (Bvh::build(&soup), soup);
            return Some(RigidMove { matrix: turn, shift });
        }
        self.rotation = rotation;
        self.place_from_file();
        let before: Vec<usize> = self.work.parts.iter().map(|p| p.tris.len()).collect();
        self.rebuild_geometry_keep_paint();
        if self.work.parts.iter().map(|p| p.tris.len()).ne(before) {
            self.painted = self.work.parts.iter().map(|p| vec![NO_LABEL; p.tris.len()]).collect();
        }
        self.rebuild_paint();
        None
    }

    /// Scene positions from the file's, turned by the rotation and set on the plate, then stretched.
    fn place_from_file(&mut self) {
        let rotation = self.rotation;
        for (part, file) in self.scene.parts.iter_mut().zip(&self.unrotated) {
            part.positions = file.iter().map(|&v| mat_vec(rotation, v)).collect();
        }
        self.placement = self.scene.place_on_plate();
        self.source_positions = self.scene.parts.iter().map(|p| p.positions.clone()).collect();
        self.measure_source();
        self.apply_stretch();
    }

    /// Size before resizing from the placed positions; the size asked for keeps the scale and stretch.
    fn measure_source(&mut self) {
        let mut it = self.source_positions.iter().flatten().copied();
        let bounds = it.next().map(|first| {
            it.fold((first, first), |(lo, hi), p| {
                (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
            })
        });
        self.source_size =
            bounds.map_or([1.0; 3], |(lo, hi): (Vec3, Vec3)| std::array::from_fn(|i| (hi[i] - lo[i]).max(1e-6)));
        self.source_height = self.source_size[2];
        self.size_mm = std::array::from_fn(|i| self.source_size[i] * self.scale * self.stretch[i]);
        self.size_history.clear();
    }

    fn apply_stretch(&mut self) {
        let stretch = self.stretch;
        for (part, src) in self.scene.parts.iter_mut().zip(&self.source_positions) {
            part.positions = src.iter().map(|s| std::array::from_fn(|i| s[i] * stretch[i])).collect();
        }
    }

    // ---- cutting ----------------------------------------------------------

    /// Cuts the model where `t` of the way along `axis` falls on the preview (0
    /// at its low end) and keeps the low side, or the high one. With
    /// `keep_other` the other side stays too, as an object of its own resting
    /// on its cut face beside the model. Both openings are capped. Surviving
    /// surfaces retain their brush labels; caps use source colours.
    /// False when the plane misses the model.
    pub fn cut(&mut self, axis: usize, t: f32, keep_low: bool, keep_other: bool) -> bool {
        let (Some((lo, hi)), Some((_, shape_hi))) = (self.work.bounds(), self.shape.bounds()) else { return false };
        if axis > 2 || !t.is_finite() {
            return false;
        }
        // The preview is the final shape: a flattened base moved it down from where the scene is.
        let at = lo[axis] + (hi[axis] - lo[axis]) * t.clamp(0.0, 1.0) + (shape_hi[axis] - hi[axis]);
        let mut point = [0.0; 3];
        let mut normal = [0.0; 3];
        point[axis] = at;
        normal[axis] = 1.0;
        // Preserve the original axis-cut placement for callers of the old interface.
        self.cut_impl(cut::Plane { point, normal }, keep_low, keep_other, false, &[], false).is_ok()
    }

    /// Cuts in preview coordinates. An empty selection means all parts.
    /// All geometry is prepared and validated before changing the document.
    pub fn cut_plane(
        &mut self,
        point: Vec3,
        normal: Vec3,
        keep_low: bool,
        keep_other: bool,
        lay_on_cut: bool,
        selected: &[usize],
    ) -> Result<(), cut::CutError> {
        let mut plane = cut::Plane::new(point, normal)?;
        if selected.iter().any(|&i| i >= self.scene.parts.len()) {
            return Err(cut::CutError::InvalidPlane);
        }
        if let (Some((_, hi)), Some((_, shape_hi))) = (self.work.bounds(), self.shape.bounds()) {
            plane.point = geom::add(plane.point, geom::sub(shape_hi, hi));
        }
        self.cut_impl(plane, keep_low, keep_other, lay_on_cut, selected, true)
    }

    fn cut_impl(
        &mut self,
        preview_plane: cut::Plane,
        keep_low: bool,
        keep_other: bool,
        lay_on_cut: bool,
        selected: &[usize],
        checked: bool,
    ) -> Result<(), cut::CutError> {
        // Positions are stored before per-axis stretch. The plane is a covector:
        // its normal transforms by S^T, not S^-1.
        let plane = cut::Plane::new(
            std::array::from_fn(|i| preview_plane.point[i] / self.stretch[i]),
            std::array::from_fn(|i| preview_plane.normal[i] * self.stretch[i]),
        )?;
        let eps = geom::len(self.source_size) * 1e-6;
        let first_new_object = self.scene.parts.iter().map(|p| p.object).max().unwrap_or(0) + 1;
        let (mut kept, mut other) = (vec![], vec![]);
        let (mut kept_paint, mut other_paint) = (vec![], vec![]);
        let (mut untouched, mut untouched_paint) = (vec![], vec![]);
        let mut intersects = false;
        let transfers: Vec<_> = self
            .scene
            .parts
            .iter()
            .enumerate()
            .map(|(pi, part)| {
                self.painted[pi]
                    .iter()
                    .any(|&l| l != NO_LABEL)
                    .then(|| PaintTransfer::new(&self.shape.parts[pi], &self.painted[pi], part.tris.len()))
            })
            .collect();
        for (pi, (part, src)) in self.scene.parts.iter().zip(&self.source_positions).enumerate() {
            let whole = Part { positions: src.clone(), ..part.clone() };
            if !selected.is_empty() && !selected.contains(&pi) {
                let painted = transfers[pi].as_ref().map(|_| CutPaint {
                    part: pi,
                    origins: (0..whole.tris.len()).map(|i| Some(i as u32)).collect(),
                    corners: (0..whole.tris.len())
                        .map(|ti| whole.corners(ti).map(|v| std::array::from_fn(|i| v[i] * self.stretch[i])))
                        .collect(),
                });
                untouched.push(whole);
                untouched_paint.push(painted);
                continue;
            }
            let crosses = whole.positions.iter().any(|&v| plane.distance(v) < -eps)
                && whole.positions.iter().any(|&v| plane.distance(v) > eps);
            intersects |= crosses;
            let sides = cut::split_plane_with_origins(&whole, plane, eps);
            if checked && sides.iter().any(|(p, _)| !p.tris.is_empty() && !cut::valid_solid(p, eps)) {
                return Err(cut::CutError::InvalidGeometry);
            }
            let [low, high] = sides.map(|(part, origins)| {
                let painted = transfers[pi].as_ref().map(|_| CutPaint {
                    part: pi,
                    origins,
                    corners: (0..part.tris.len())
                        .map(|ti| part.corners(ti).map(|v| std::array::from_fn(|i| v[i] * self.stretch[i])))
                        .collect(),
                });
                (part, painted)
            });
            let (k, o) = if keep_low { (low, high) } else { (high, low) };
            if !k.0.tris.is_empty() {
                kept.push(k.0);
                kept_paint.push(k.1);
            }
            if !o.0.tris.is_empty() {
                other.push(o.0);
                other_paint.push(o.1);
            }
        }
        if kept.is_empty() || other.is_empty() || (checked && !intersects) {
            return Err(cut::CutError::Misses);
        }
        self.end_stroke();
        let kept_normal = geom::scale(preview_plane.normal, if keep_low { 1.0 } else { -1.0 });
        let rotate_parts = |parts: &mut Vec<Part>, normal: Vec3| {
            let turn = rotation_between(normal, [0.0, 0.0, -1.0]);
            for v in parts.iter_mut().flat_map(|p| &mut p.positions) {
                let stretched = std::array::from_fn(|i| v[i] * self.stretch[i]);
                let rotated = mat_vec(turn, stretched);
                *v = std::array::from_fn(|i| rotated[i] / self.stretch[i]);
            }
        };
        if lay_on_cut {
            rotate_parts(&mut kept, kept_normal);
        }
        if checked {
            // A selected upper half must land too, without lifting untouched objects.
            let bed_z = self.source_positions.iter().flatten().map(|v| v[2]).fold(f32::INFINITY, f32::min);
            let low_z = kept.iter().flat_map(|p| &p.positions).map(|v| v[2]).fold(f32::INFINITY, f32::min);
            for v in kept.iter_mut().flat_map(|p| &mut p.positions) {
                v[2] += bed_z - low_z;
            }
        }
        if keep_other {
            // Its cut face looks back at the kept side; turned to face the plate.
            let sign = if keep_low { -1.0 } else { 1.0 };
            let turn = rotation_between(geom::scale(plane.normal, sign), [0.0, 0.0, -1.0]);
            if lay_on_cut {
                rotate_parts(&mut other, geom::scale(kept_normal, -1.0));
            }
            for p in &mut other {
                p.object += first_new_object;
                p.name = format!("{} ({})", p.name, p.object + 1);
                if !checked {
                    for v in &mut p.positions {
                        *v = mat_vec(turn, *v);
                    }
                }
            }
            let bounds = |ps: &[Part]| {
                let mut it = ps.iter().flat_map(|p| p.positions.iter().copied());
                let first = it.next().unwrap_or([0.0; 3]);
                it.fold((first, first), |(lo, hi), p| {
                    (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn::<f32, 3, _>(|i| hi[i].max(p[i])))
                })
            };
            let ((klo, mut khi), (olo, ohi)) = (bounds(&kept), bounds(&other));
            if checked {
                for p in &untouched {
                    for v in &p.positions {
                        khi[0] = khi[0].max(v[0]);
                    }
                }
            }
            // Beside the model along X, 5 mm apart, centred with it on Y and on the plate.
            let gap = 5.0 / self.scale / self.stretch[0];
            let shift = [khi[0] + gap - olo[0], (klo[1] + khi[1] - olo[1] - ohi[1]) / 2.0, klo[2] - olo[2]];
            for v in other.iter_mut().flat_map(|p| p.positions.iter_mut()) {
                *v = geom::add(*v, shift);
            }
            kept.append(&mut other);
            kept_paint.append(&mut other_paint);
        }
        kept.append(&mut untouched);
        kept_paint.append(&mut untouched_paint);
        self.commit_parts(kept, kept_paint, &transfers, "cut");
        Ok(())
    }

    /// Replaces the scene's parts as one undoable step (`kind` names it for the
    /// undo button). Each new part says where its triangles came from, so brush
    /// labels follow them; triangles with no parent take the model's colours.
    fn commit_parts(
        &mut self,
        parts: Vec<Part>,
        paint: Vec<Option<CutPaint>>,
        transfers: &[Option<PaintTransfer>],
        kind: &'static str,
    ) {
        self.end_stroke();
        let previous = std::mem::replace(&mut self.scene.parts, parts);
        self.cut_history.push(CutSnapshot {
            parts: previous,
            unrotated: self.unrotated.clone(),
            painted: self.painted.clone(),
            undo: std::mem::take(&mut self.undo),
            kind,
        });
        let kept_paint = paint;
        let shift = self.scene.place_on_plate();
        self.placement = geom::add(self.placement, shift);
        self.source_positions = self.scene.parts.iter().map(|p| p.positions.clone()).collect();
        let back = transpose(&self.rotation);
        let placement = self.placement;
        self.unrotated = self
            .source_positions
            .iter()
            .map(|ps| ps.iter().map(|&v| mat_vec(back, geom::sub(v, placement))).collect())
            .collect();
        self.measure_source();
        self.apply_stretch();
        self.rebuild_geometry();
        for (pi, saved) in kept_paint.iter().enumerate() {
            let Some(saved) = saved else { continue };
            let transfer = transfers[saved.part].as_ref().unwrap();
            let shape = &self.shape.parts[pi];
            // Simplification breaks ancestry, so locate the source face as the
            // colour field does; keep caps in the lookup to avoid paint bleeding.
            let surface = shape.origin.is_none().then(|| {
                let source = &self.scene.parts[pi];
                let mut soup = TriSoup::default();
                for ti in 0..source.tris.len() {
                    soup.push(source.corners(ti), 0, ti as u32);
                }
                (Bvh::build(&soup), soup)
            });
            for ti in 0..shape.tris.len() {
                let source_point = if let Some(origin) = &shape.origin {
                    let (src, bary) = &origin[ti];
                    Some((*src, geom::bary_in(bary, [1.0 / 3.0; 3])))
                } else {
                    let (bvh, soup) = surface.as_ref().unwrap();
                    bvh.nearest(soup, geom::at(&shape.corners(ti), [1.0 / 3.0; 3]))
                };
                if let Some((src, bary)) = source_point {
                    if let Some(parent) = saved.origins[src as usize] {
                        let point = geom::at(&saved.corners[src as usize], bary);
                        self.painted[pi][ti] = transfer.label(Some(parent), point);
                    }
                }
            }
        }
        self.rebuild_paint();
        self.undo.push(UndoEntry::Cut);
    }

    pub fn can_undo_cut(&self) -> bool {
        !self.cut_history.is_empty()
    }

    /// Printed objects: 1 for the model, more once pieces cut off it are kept.
    pub fn object_count(&self) -> usize {
        self.scene.parts.iter().map(|p| p.object).collect::<std::collections::HashSet<_>>().len().max(1)
    }

    // ---- separating pieces -------------------------------------------------

    /// Disconnected pieces of every part, triangles as the scene has them.
    /// Corners within `weld_mm` (final millimetres) count as one.
    fn source_pieces(&self, weld_mm: f32) -> Vec<(usize, Vec<u32>)> {
        let weld = weld_mm.max(0.0) / self.scale;
        self.scene
            .parts
            .iter()
            .enumerate()
            .flat_map(|(pi, p)| separate::pieces(&p.positions, &p.tris, weld).into_iter().map(move |t| (pi, t)))
            .collect()
    }

    /// The disconnected pieces of the model, part by part and largest first,
    /// and for each triangle of the preview mesh the piece it belongs to.
    pub fn detect_pieces(&self, weld_mm: f32) -> (Vec<DetectedPiece>, Vec<u32>) {
        let found = self.source_pieces(weld_mm);
        let weld = weld_mm.max(0.0) / self.scale;
        let mut by_tri: Vec<Vec<u32>> = self.scene.parts.iter().map(|p| vec![u32::MAX; p.tris.len()]).collect();
        let pieces = found
            .iter()
            .enumerate()
            .map(|(k, (pi, tris))| {
                let part = &self.scene.parts[*pi];
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for &t in tris {
                    by_tri[*pi][t as usize] = k as u32;
                    for v in part.tris[t as usize] {
                        let p = part.positions[v as usize];
                        lo = std::array::from_fn(|i| lo[i].min(p[i]));
                        hi = std::array::from_fn(|i| hi[i].max(p[i]));
                    }
                }
                DetectedPiece {
                    part: *pi,
                    triangles: tris.len(),
                    size_mm: geom::scale(geom::sub(hi, lo), self.scale),
                    centre: geom::scale(geom::add(lo, hi), 0.5),
                    closed: separate::closed(&part.positions, &part.tris, tris, weld),
                }
            })
            .collect();
        // Simplified triangles lost their source face: the nearest one stands in.
        let mut work = Vec::with_capacity(self.work.triangle_count());
        for (pi, shape) in self.shape.parts.iter().enumerate() {
            let source = &self.scene.parts[pi];
            let lookup = shape.origin.is_none().then(|| {
                let mut soup = TriSoup::default();
                for ti in 0..source.tris.len() {
                    soup.push(source.corners(ti), 0, ti as u32);
                }
                (Bvh::build(&soup), soup)
            });
            for ti in 0..shape.tris.len() {
                let src = match (&shape.origin, &lookup) {
                    (Some(origin), _) => Some(origin[ti].0),
                    (None, Some((bvh, soup))) => {
                        bvh.nearest(soup, geom::at(&shape.corners(ti), [1.0 / 3.0; 3])).map(|h| h.0)
                    }
                    _ => None,
                };
                work.push(src.and_then(|s| by_tri[pi].get(s as usize).copied()).unwrap_or(u32::MAX));
            }
        }
        (pieces, work)
    }

    /// Makes the pieces [`Self::detect_pieces`] found with the same `weld_mm`
    /// objects of their own: pieces given the same group form one object, and
    /// `None` leaves a piece in the object it is in. The pieces' triangles do
    /// not change, so colours and hand painting stay. With `lay_flat` each new
    /// object rests on the plate in a row beside the model; otherwise the
    /// pieces keep their assembly position. One undoable step; returns how
    /// many objects there are now.
    pub fn separate_pieces(
        &mut self,
        weld_mm: f32,
        groups: &[Option<u32>],
        lay_flat: bool,
    ) -> Result<usize, SeparateError> {
        let found = self.source_pieces(weld_mm);
        if found.len() != groups.len() {
            return Err(SeparateError::Stale);
        }
        // Nothing changes when the pieces end up grouped exactly as they were.
        let mut forward: HashMap<u32, (bool, u32)> = HashMap::new();
        let mut backward: HashMap<(bool, u32), u32> = HashMap::new();
        let unchanged = found.iter().zip(groups).all(|((pi, _), g)| {
            let before = self.scene.parts[*pi].object;
            let after = g.map_or((false, before), |g| (true, g));
            *forward.entry(before).or_insert(after) == after && *backward.entry(after).or_insert(before) == before
        });
        if unchanged || groups.iter().all(Option::is_none) {
            return Err(SeparateError::NothingToSeparate);
        }
        let transfers: Vec<_> = self
            .scene
            .parts
            .iter()
            .enumerate()
            .map(|(pi, part)| {
                self.painted[pi]
                    .iter()
                    .any(|&l| l != NO_LABEL)
                    .then(|| PaintTransfer::new(&self.shape.parts[pi], &self.painted[pi], part.tris.len()))
            })
            .collect();
        let stretch = self.stretch;
        let first_new_object = self.scene.parts.iter().map(|p| p.object).max().unwrap_or(0) + 1;
        let mut order: Vec<u32> = groups.iter().flatten().copied().collect();
        order.sort_unstable();
        order.dedup();
        let (mut kept, mut kept_paint) = (vec![], vec![]);
        // Per new object, its parts (one per source part it takes pieces from).
        let mut moved: Vec<Vec<(Part, Option<CutPaint>)>> = (0..order.len()).map(|_| vec![]).collect();
        for (pi, (part, src)) in self.scene.parts.iter().zip(&self.source_positions).enumerate() {
            let whole = Part { positions: src.clone(), ..part.clone() };
            let piece = |tris: &[u32]| {
                let p = separate::sub_part(&whole, tris);
                let paint = transfers[pi].as_ref().map(|_| CutPaint {
                    part: pi,
                    origins: tris.iter().map(|&t| Some(t)).collect(),
                    corners: tris
                        .iter()
                        .map(|&t| whole.corners(t as usize).map(|v| std::array::from_fn(|i| v[i] * stretch[i])))
                        .collect(),
                });
                (p, paint)
            };
            let mine: Vec<(&Vec<u32>, Option<u32>)> =
                found.iter().zip(groups).filter(|((p, _), _)| *p == pi).map(|((_, t), g)| (t, *g)).collect();
            let mut stay: Vec<u32> =
                mine.iter().filter(|(_, g)| g.is_none()).flat_map(|(t, _)| t.iter().copied()).collect();
            if !stay.is_empty() {
                stay.sort_unstable();
                let (p, paint) = piece(&stay);
                kept.push(p);
                kept_paint.push(paint);
            }
            for (rank, g) in order.iter().enumerate() {
                let mut tris: Vec<u32> =
                    mine.iter().filter(|(_, x)| *x == Some(*g)).flat_map(|(t, _)| t.iter().copied()).collect();
                if tris.is_empty() {
                    continue;
                }
                tris.sort_unstable();
                let (mut p, paint) = piece(&tris);
                p.object = first_new_object + rank as u32;
                p.name = format!("{} ({})", p.name, p.object + 1);
                moved[rank].push((p, paint));
            }
        }
        if lay_flat {
            // Each object down on the plate, then side by side along X past the rest, 5 mm apart.
            let bed_z = self.source_positions.iter().flatten().map(|v| v[2]).fold(f32::INFINITY, f32::min);
            let bounds = |parts: &mut dyn Iterator<Item = &Part>| {
                parts.flat_map(|p| p.positions.iter()).fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                    (std::array::from_fn(|i| lo[i].min(v[i])), std::array::from_fn(|i| hi[i].max(v[i])))
                })
            };
            let (klo, khi) = bounds(&mut kept.iter());
            let mut x = if kept.is_empty() { None } else { Some(khi[0]) };
            let centre_y = if kept.is_empty() { 0.0 } else { (klo[1] + khi[1]) / 2.0 };
            let gap = 5.0 / self.scale / stretch[0];
            for object in &mut moved {
                let (lo, hi) = bounds(&mut object.iter().map(|(p, _)| p));
                let to_x = x.map_or(lo[0], |x| x + gap);
                let shift = [to_x - lo[0], centre_y - (lo[1] + hi[1]) / 2.0, bed_z - lo[2]];
                for v in object.iter_mut().flat_map(|(p, _)| p.positions.iter_mut()) {
                    *v = geom::add(*v, shift);
                }
                x = Some(hi[0] + shift[0]);
            }
        }
        for (p, paint) in moved.into_iter().flatten() {
            kept.push(p);
            kept_paint.push(paint);
        }
        self.commit_parts(kept, kept_paint, &transfers, "separate");
        Ok(self.object_count())
    }

    /// Planes worth reviewing to split a continuous mesh, in preview
    /// coordinates: where it narrows and where its colour changes. Only
    /// suggestions; nothing is cut until the user confirms a cut.
    pub fn propose_cuts(&self, selected: &[usize]) -> Vec<CutProposal> {
        let labels: Vec<Vec<u8>> = self.trees.iter().map(|n| n.iter().map(Node::dominant).collect()).collect();
        let closed: Vec<bool> = self
            .work
            .parts
            .iter()
            .map(|p| separate::closed(&p.positions, &p.tris, &(0..p.tris.len() as u32).collect::<Vec<_>>(), 0.0))
            .collect();
        let mm2 = self.scale * self.scale;
        separate::proposals(&self.work.parts, &labels, selected, &closed)
            .into_iter()
            .map(|p| {
                let (lo, hi) =
                    self.work.parts[p.part].positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                        (std::array::from_fn(|i| lo[i].min(v[i])), std::array::from_fn(|i| hi[i].max(v[i])))
                    });
                let mut point = geom::scale(geom::add(lo, hi), 0.5);
                point[p.axis] = p.at;
                let mut normal = [0.0; 3];
                normal[p.axis] = 1.0;
                let signal = match p.signal {
                    separate::Signal::Neck { area, before, after } => {
                        separate::Signal::Neck { area: area * mm2, before: before * mm2, after: after * mm2 }
                    }
                    s => s,
                };
                CutProposal { part: p.part, axis: p.axis, point, normal, signal }
            })
            .collect()
    }

    /// Puts back what the last cut took, in the orientation the model has now.
    pub fn undo_cut(&mut self) -> bool {
        let Some(s) = self.cut_history.pop() else { return false };
        self.scene.parts = s.parts;
        self.unrotated = s.unrotated;
        self.place_from_file();
        self.rebuild_geometry();
        for (restored, saved) in self.painted.iter_mut().zip(s.painted) {
            if restored.len() == saved.len() {
                *restored = saved;
                for label in restored {
                    if *label as usize >= self.palette.len() {
                        *label = NO_LABEL;
                    }
                }
            }
        }
        self.rebuild_paint();
        self.undo = s.undo;
        true
    }

    /// The rotation (absolute, like [`Self::set_rotation`]) that leaves the least
    /// downward-facing area steeper than `overhang_deg` off the plate. It tries
    /// resting on each axis, edge and corner direction and on the largest faces,
    /// but only where the model stands by itself: the centre of mass has to fall
    /// inside the footprint it rests on, so it never balances on a corner. Near
    /// ties go to more plate contact. Returns the current rotation when nothing
    /// stable is clearly better.
    pub fn best_rotation(&self, overhang_deg: f32) -> Mat3 {
        let tris: Vec<[Vec3; 3]> =
            self.scene.parts.iter().flat_map(|p| (0..p.tris.len()).map(move |t| p.corners(t))).collect();
        let Some((lo, hi)) = self.scene.bounds() else { return self.rotation };
        let diagonal = geom::len(geom::sub(hi, lo));
        let tol = diagonal * 1e-3;
        let faces: Vec<(Vec3, f32)> = tris
            .iter()
            .map(|c| {
                let n = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
                (geom::normalize(n), geom::len(n) * 0.5)
            })
            .collect();
        let total: f32 = faces.iter().map(|f| f.1).sum();
        let centre = mass_centre(&tris, &faces, lo);
        let limit = overhang_deg.to_radians().sin();
        // (overhang, contact) with `down` pointing at the plate; None when it would tip over.
        let score = |down: Vec3| -> Option<(f32, f32)> {
            let up = geom::scale(down, -1.0);
            let base = tris.iter().flatten().map(|&p| geom::dot(p, up)).fold(f32::MAX, f32::min);
            let (mut overhang, mut contact) = (0.0, 0.0);
            let mut feet = vec![];
            for (c, &(n, area)) in tris.iter().zip(&faces) {
                let facing = geom::dot(n, up);
                if c.iter().all(|&p| geom::dot(p, up) <= base + tol) {
                    if facing < 0.0 {
                        contact += area;
                    }
                } else if -facing > limit {
                    overhang += area;
                }
                feet.extend(c.iter().copied().filter(|&p| geom::dot(p, up) <= base + tol));
            }
            stands(&feet, centre, up, diagonal * 0.01).then_some((overhang, contact))
        };
        let mut candidates: Vec<Vec3> = vec![];
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if (x, y, z) != (0, 0, 0) {
                        candidates.push(geom::normalize([x as f32, y as f32, z as f32]));
                    }
                }
            }
        }
        let mut largest: Vec<&(Vec3, f32)> = faces.iter().filter(|f| f.1 > 0.0).collect();
        largest.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.extend(largest.iter().take(24).map(|f| f.0));
        // The model as it is stays a valid answer even if it would not stand.
        let current = score([0.0, 0.0, -1.0]).unwrap_or_else(|| {
            let up = [0.0, 0.0, 1.0];
            let overhang = faces.iter().filter(|f| -geom::dot(f.0, up) > limit).map(|f| f.1).sum();
            (overhang, 0.0)
        });
        let mut best = ([0.0, 0.0, -1.0], current);
        for d in candidates {
            let Some(s) = score(d) else { continue };
            let near = (s.0 - best.1 .0).abs() <= total * 0.01;
            if s.0 < best.1 .0 - total * 0.01 || (near && s.1 > best.1 .1 * 1.05) {
                best = (d, s);
            }
        }
        // Not worth turning for less than 1% of the surface.
        if best.1 .0 >= current.0 - total * 0.01 && best.1 .1 <= current.1 * 1.05 {
            return self.rotation;
        }
        mat_mul(onto_down(best.0), self.rotation)
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    pub fn palette(&self) -> &[Rgb] {
        &self.palette
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }
    /// Height the user asked for, before any base is flattened away.
    pub fn height_mm(&self) -> f32 {
        self.source_height * self.scale
    }
    /// Width, depth and height as the file has them, in millimetres.
    pub fn original_size_mm(&self) -> Vec3 {
        self.source_size
    }
    /// Width, depth and height the user asked for, before flattening.
    pub fn size_mm(&self) -> Vec3 {
        self.size_mm
    }
    /// Scale of each axis against the original size.
    pub fn axis_scale(&self) -> Vec3 {
        std::array::from_fn(|i| self.scale * self.stretch[i])
    }
    /// Per-axis stretch the preview positions carry on top of [`Self::scale`].
    pub fn stretch(&self) -> Vec3 {
        self.stretch
    }
    pub fn can_undo_size(&self) -> bool {
        !self.size_history.is_empty()
    }
    pub fn mesh_ops(&self) -> MeshOps {
        self.mesh_ops
    }
    pub fn color_ops(&self) -> ColorOps {
        self.color_ops
    }
    pub fn print_ops(&self) -> PrintOps {
        self.print_ops
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || !self.stroke.is_empty()
    }

    /// What the next undo would revert, as a key the UI translates: "stroke" or "merge".
    pub fn undo_label(&self) -> Option<&'static str> {
        if !self.stroke.is_empty() {
            return Some("stroke");
        }
        self.undo.last().map(|e| match e {
            UndoEntry::Stroke(_) => "stroke",
            UndoEntry::Slots(_) => "merge",
            UndoEntry::Geometry(_) => "fix",
            UndoEntry::Cut => self.cut_history.last().map_or("cut", |c| c.kind),
        })
    }

    /// Triangles painted by hand; a full repaint with new colours discards them.
    pub fn painted_count(&self) -> usize {
        self.painted.iter().flatten().filter(|&&l| l != NO_LABEL).count()
    }

    // ---- palette ----------------------------------------------------------

    pub fn suggest_palette(&self, n: usize) -> Vec<Rgb> {
        let field = ColorField::new(&self.scene, &self.shape);
        let samples = palette::surface_samples(&field, &self.shape, 300_000, 1);
        palette::auto_palette(&samples, n.clamp(1, 16))
    }

    /// Replaces the target colours and repaints. With the same number of slots,
    /// each slot keeps the colours merged into it; otherwise merges are reset.
    /// Hand painting on slots that no longer exist is dropped.
    pub fn set_palette(&mut self, colors: Vec<Rgb>) {
        let n = colors.len().clamp(1, 16);
        let colors: Vec<Rgb> = colors.into_iter().take(n).collect();
        if n == self.palette.len() {
            for t in &mut self.targets {
                // The slot's own target is the one equal to its old colour.
                if t.0 == self.palette[t.1 as usize] {
                    t.0 = colors[t.1 as usize];
                }
            }
        } else {
            self.targets = colors.iter().enumerate().map(|(i, &c)| (c, i as u8)).collect();
        }
        self.palette = colors;
        for l in self.painted.iter_mut().chain(self.cut_history.iter_mut().flat_map(|s| &mut s.painted)).flatten() {
            if *l != NO_LABEL && *l as usize >= n {
                *l = NO_LABEL;
            }
        }
        self.rebuild_paint();
    }

    /// Merges the closest slots (ΔE) until `target` remain. Painting is
    /// relabelled, not recomputed, so hand painting survives.
    pub fn reduce_palette(&mut self, target: usize) {
        let weights = self.stats_per_slot();
        let (palette, map) = palette::reduce(&self.palette, &weights, target.clamp(1, 16));
        self.apply_slot_map(palette, &map);
    }

    /// Merges slot `from` into slot `into`.
    pub fn merge_slots(&mut self, from: usize, into: usize) {
        if from == into || from >= self.palette.len() || into >= self.palette.len() || self.palette.len() < 2 {
            return;
        }
        let (palette, map) = palette::merge(&self.palette, from, into);
        self.apply_slot_map(palette, &map);
    }

    fn apply_slot_map(&mut self, palette: Vec<Rgb>, map: &[u8]) {
        self.end_stroke();
        let snapshot = SlotSnapshot {
            palette: self.palette.clone(),
            targets: self.targets.clone(),
            trees: self.trees.clone(),
            painted: self.painted.clone(),
            cut_painted: self.cut_history.iter().map(|s| s.painted.clone()).collect(),
        };
        for t in &mut self.targets {
            t.1 = map[t.1 as usize];
        }
        for nodes in &mut self.trees {
            for n in nodes.iter_mut() {
                *n = n.relabel(map);
            }
        }
        for l in self.painted.iter_mut().chain(self.cut_history.iter_mut().flat_map(|s| &mut s.painted)).flatten() {
            if *l != NO_LABEL {
                *l = map[*l as usize];
            }
        }
        self.palette = palette;
        self.undo.push(UndoEntry::Slots(Box::new(snapshot)));
    }

    fn stats_per_slot(&self) -> Vec<usize> {
        let mut per = vec![0usize; self.palette.len()];
        for n in self.trees.iter().flatten() {
            if let Some(c) = per.get_mut(n.dominant() as usize) {
                *c += 1;
            }
        }
        per
    }

    // ---- size and mesh ----------------------------------------------------

    /// Sets the final height; width and depth keep their proportions.
    pub fn set_height_mm(&mut self, mm: f32) {
        let k = mm / self.size_mm[2];
        self.set_size_mm(self.size_mm.map(|v| v * k));
    }

    /// Sets width, depth and height (before flattening). Unequal factors
    /// stretch the model; the previous size can be undone.
    pub fn set_size_mm(&mut self, size: Vec3) {
        let size = size.map(|v| v.max(1e-6));
        if size == self.size_mm {
            return;
        }
        self.size_history.push(self.size_mm);
        self.apply_size(size);
    }

    /// Goes back to the size before the last resize.
    pub fn undo_size(&mut self) -> bool {
        let Some(size) = self.size_history.pop() else { return false };
        self.apply_size(size);
        true
    }

    /// Back to the size the file has; it can be undone like any resize.
    pub fn reset_size(&mut self) {
        self.set_size_mm(self.source_size);
    }

    fn apply_size(&mut self, size: Vec3) {
        self.size_mm = size;
        self.scale = (size[2] / self.source_height).max(1e-6);
        let stretch: Vec3 = std::array::from_fn(|i| size[i] / (self.source_size[i] * self.scale));
        let stretch_changes = stretch.iter().zip(&self.stretch).any(|(a, b)| (a - b).abs() > 1e-6);
        if stretch_changes {
            self.stretch = stretch;
            for (part, src) in self.scene.parts.iter_mut().zip(&self.source_positions) {
                for (v, s) in part.positions.iter_mut().zip(src) {
                    *v = std::array::from_fn(|i| s[i] * stretch[i]);
                }
            }
        }
        if stretch_changes || self.mesh_ops.flatten_mm > 0.0 || self.mesh_ops.min_piece_mm > 0.0 {
            let before: Vec<usize> = self.work.parts.iter().map(|p| p.tris.len()).collect();
            self.rebuild_geometry_keep_paint();
            // Hand painting is per triangle: it only survives when the triangles do.
            if self.work.parts.iter().map(|p| p.tris.len()).ne(before) {
                self.painted = self.work.parts.iter().map(|p| vec![NO_LABEL; p.tris.len()]).collect();
            }
        }
        self.rebuild_paint();
    }

    pub fn set_mesh_ops(&mut self, ops: MeshOps) {
        let topology_changes = (ops.keep_ratio - self.mesh_ops.keep_ratio).abs() > 1e-4
            || ops.repair != self.mesh_ops.repair
            || (ops.min_piece_mm - self.mesh_ops.min_piece_mm).abs() > 1e-4
            || ops.orient != self.mesh_ops.orient
            || ops.fill_holes != self.mesh_ops.fill_holes
            || ops.split_vertices != self.mesh_ops.split_vertices
            || ops.deep_clean != self.mesh_ops.deep_clean;
        self.mesh_ops = ops;
        if topology_changes {
            self.rebuild_geometry();
        } else {
            self.rebuild_geometry_keep_paint();
            // Flattening can add or remove triangles even without another mesh
            // operation changing. Brush storage must match the rebuilt mesh.
            for (labels, part) in self.painted.iter_mut().zip(&self.work.parts) {
                if labels.len() != part.tris.len() {
                    *labels = vec![NO_LABEL; part.tris.len()];
                }
            }
        }
        self.rebuild_paint();
    }

    pub fn set_color_ops(&mut self, ops: ColorOps) {
        self.color_ops = ColorOps { border_detail: ops.border_detail.min(6), ..ops };
        self.rebuild_paint();
    }

    /// A new nozzle changes how fine colour borders are cut; a new layer
    /// height only changes the estimate.
    pub fn set_print_ops(&mut self, ops: PrintOps) {
        let nozzle_changes = (ops.nozzle_mm - self.print_ops.nozzle_mm).abs() > 1e-4;
        self.print_ops = ops;
        if nozzle_changes {
            self.rebuild_paint();
        }
    }

    fn rebuild_geometry(&mut self) {
        self.rebuild_geometry_keep_paint();
        self.painted = self.work.parts.iter().map(|p| vec![NO_LABEL; p.tris.len()]).collect();
    }

    fn rebuild_geometry_keep_paint(&mut self) {
        let (shape, work) = Mesh::build(&self.scene, self.mesh_ops, self.scale);
        self.shape = shape;
        self.work = work;
        self.adjacency = self.work.parts.iter().map(optimize::adjacency).collect();
        let soup = self.work.soup();
        self.picker = (Bvh::build(&soup), soup);
        let mut acc = 0u32;
        self.tri_offsets = self
            .work
            .parts
            .iter()
            .map(|p| {
                let o = acc;
                acc += p.tris.len() as u32;
                o
            })
            .collect();
    }

    // ---- painting from the model -----------------------------------------

    fn rebuild_paint(&mut self) {
        let field = ColorField::new(&self.scene, &self.shape);
        let cls = Classifier::with_slots(&self.targets);
        // Borders are not cut finer than half a nozzle: the printer cannot draw them.
        let min_edge = self.print_ops.nozzle_mm / 2.0 / self.scale;
        let params = PaintParams { max_depth: self.color_ops.border_detail, min_edge };
        self.trees = paint::build(&field, &self.shape, &cls, params);
        for (nodes, painted) in self.trees.iter_mut().zip(&self.painted) {
            for (n, &l) in nodes.iter_mut().zip(painted) {
                if l != NO_LABEL {
                    *n = Node::Leaf(l);
                }
            }
        }
        if self.color_ops.min_island_mm2 > 0.0 {
            let min_area = self.color_ops.min_island_mm2 / (self.scale * self.scale);
            let parts = self.work.parts.iter().zip(&self.adjacency);
            for ((part, adj), (nodes, painted)) in parts.zip(self.trees.iter_mut().zip(&self.painted)) {
                let locked: Vec<bool> = painted.iter().map(|&l| l != NO_LABEL).collect();
                optimize::despeckle(part, adj, nodes, &locked, min_area);
            }
        }
        self.stroke.clear();
        self.undo.clear();
    }

    // ---- hand painting ----------------------------------------------------

    /// Ray in model space (millimetres before scaling, as the preview shows it).
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<RayHit> {
        self.picker.0.raycast(&self.picker.1, origin, geom::normalize(dir))
    }

    /// Paints every triangle within `radius_mm` (final size) of where the ray
    /// lands. Returns the hit and the global ids of the triangles that changed.
    pub fn paint(&mut self, origin: Vec3, dir: Vec3, radius_mm: f32, slot: u8) -> Option<(RayHit, Vec<u32>)> {
        if slot as usize >= self.palette.len() {
            return None;
        }
        let hit = self.pick(origin, dir)?;
        let mut found = vec![hit.tri];
        self.picker.0.sphere(&self.picker.1, hit.point, radius_mm / self.scale, &mut found);
        found.sort_unstable();
        found.dedup();
        let mut changed = Vec::new();
        for t in found {
            let (pi, ti) = self.picker.1.owner[t as usize];
            let (p, i) = (pi as usize, ti as usize);
            let node = &mut self.trees[p][i];
            if *node == Node::Leaf(slot) && self.painted[p][i] == slot {
                continue;
            }
            self.stroke.push(Change { part: pi, tri: ti, node: node.clone(), painted: self.painted[p][i] });
            *node = Node::Leaf(slot);
            self.painted[p][i] = slot;
            changed.push(self.tri_offsets[p] + ti);
        }
        Some((hit, changed))
    }

    pub fn end_stroke(&mut self) {
        if !self.stroke.is_empty() {
            self.undo.push(UndoEntry::Stroke(std::mem::take(&mut self.stroke)));
        }
    }

    /// Undoes the last stroke or slot merge. The preview has to be rebuilt afterwards.
    pub fn undo(&mut self) -> bool {
        self.end_stroke();
        match self.undo.pop() {
            None => false,
            Some(UndoEntry::Cut) => self.undo_cut(),
            Some(UndoEntry::Stroke(stroke)) => {
                for c in stroke.into_iter().rev() {
                    let (p, i) = (c.part as usize, c.tri as usize);
                    self.painted[p][i] = c.painted;
                    self.trees[p][i] = c.node;
                }
                true
            }
            Some(UndoEntry::Geometry(previous)) => {
                let print_ops = self.print_ops;
                *self = *previous;
                self.set_print_ops(print_ops);
                true
            }
            Some(UndoEntry::Slots(s)) => {
                let s = *s;
                self.palette = s.palette;
                self.targets = s.targets;
                self.trees = s.trees;
                self.painted = s.painted;
                for (cut, painted) in self.cut_history.iter_mut().zip(s.cut_painted) {
                    cut.painted = painted;
                }
                true
            }
        }
    }

    // ---- read models ------------------------------------------------------

    pub fn preview(&self) -> Preview {
        let field = ColorField::new(&self.scene, &self.shape);
        let leaves: usize = self.trees.iter().flatten().map(Node::leaf_count).sum();
        let mut pv = Preview {
            positions: Vec::with_capacity(leaves * 9),
            labels: Vec::with_capacity(leaves),
            source_rgb: Vec::with_capacity(leaves * 3),
            offsets: Vec::with_capacity(self.work.triangle_count() + 1),
        };
        for (pi, (part, nodes)) in self.work.parts.iter().zip(&self.trees).enumerate() {
            for (ti, node) in nodes.iter().enumerate() {
                pv.offsets.push(pv.labels.len() as u32);
                let c = part.corners(ti);
                node.for_each_leaf(&ROOT, &mut |t, label| {
                    for &b in t {
                        pv.positions.extend(geom::at(&c, b));
                    }
                    pv.labels.push(label);
                    let centre = geom::bary_in(t, [1.0 / 3.0; 3]);
                    pv.source_rgb.extend(to_u8(field.sample(&self.shape, pi, ti, centre).rgb()));
                });
            }
        }
        pv.offsets.push(pv.labels.len() as u32);
        pv
    }

    /// Leaf ranges in the binary preview, one for each scene part.
    pub fn preview_part_ranges(&self) -> Vec<(usize, usize)> {
        let mut start = 0;
        self.trees
            .iter()
            .map(|nodes| {
                let end = start + nodes.iter().map(Node::leaf_count).sum::<usize>();
                let range = (start, end);
                start = end;
                range
            })
            .collect()
    }

    pub fn stats(&self) -> Stats {
        let mut per_slot = vec![0usize; self.palette.len()];
        let mut split = 0;
        for n in self.trees.iter().flatten() {
            if let Some(c) = per_slot.get_mut(n.dominant() as usize) {
                *c += 1;
            }
            split += n.is_split() as usize;
        }
        let size = self.work.bounds().map_or([0.0; 3], |(lo, hi)| geom::scale(geom::sub(hi, lo), self.scale));
        Stats {
            source_triangles: self.scene.triangle_count(),
            triangles: self.work.triangle_count(),
            split_triangles: split,
            per_slot,
            size_mm: size,
            contact_mm2: self.work.contact_area(0.05 / self.scale) * self.scale * self.scale,
            // A colour narrower than two lines on a layer is not printed as its own.
            swaps: estimate::estimate(
                &self.work,
                &self.trees,
                self.scale,
                self.print_ops.layer_mm,
                2.0 * self.print_ops.nozzle_mm * self.print_ops.layer_mm,
            ),
            nearest: palette::nearest_others(&self.palette),
            repair: self.work.repair,
        }
    }

    /// Colour islands smaller than `min_mm2`, the ones the nozzle cannot draw cleanly.
    pub fn small_islands(&self, min_mm2: f32) -> usize {
        let min_area = min_mm2 / (self.scale * self.scale);
        self.work
            .parts
            .iter()
            .zip(&self.adjacency)
            .zip(&self.trees)
            .map(|((part, adj), nodes)| optimize::small_islands(part, adj, nodes, min_area))
            .sum()
    }

    /// One closed piece per colour in final millimetres: shells `thickness_mm`
    /// deep, or with `main_solid` the colour with most triangles as the whole part.
    pub fn color_pieces(&self, thickness_mm: f32, main_solid: bool) -> Vec<ColorPiece> {
        let labels: Vec<Vec<u8>> = self.trees.iter().map(|n| n.iter().map(Node::dominant).collect()).collect();
        let per = self.stats_per_slot();
        let main = main_solid.then(|| (0..per.len()).max_by_key(|&i| per[i]).unwrap_or(0) as u8);
        let mut pieces = split::color_pieces(&self.work, &labels, self.palette.len(), thickness_mm / self.scale, main);
        for p in &mut pieces {
            for v in &mut p.positions {
                *v = geom::scale(*v, self.scale);
            }
        }
        pieces
    }

    pub fn export_parts(&self) -> Vec<ExportPart> {
        self.work
            .parts
            .iter()
            .zip(&self.trees)
            .zip(&self.scene.parts)
            .map(|((part, nodes), src)| ExportPart {
                name: part.name.clone(),
                object: src.object,
                positions: part.positions.iter().map(|&v| geom::scale(v, self.scale)).collect(),
                tris: part.tris.clone(),
                paint: nodes.iter().map(encode::encode).collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ColorSource, Part};

    /// Two stacked quads, the lower one red and the upper one blue.
    fn scene() -> Scene {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
            [10.0, 0.0, 20.0],
            [0.0, 0.0, 20.0],
        ];
        let tris = vec![[0, 1, 2], [0, 2, 3], [3, 2, 4], [3, 4, 5]];
        let part = Part {
            name: "q".into(),
            positions,
            tris,
            tri_loops: vec![[0, 1, 2], [3, 4, 5], [6, 7, 8], [9, 10, 11]],
            tri_source: vec![0, 0, 1, 1],
            sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
            uv_layers: vec![],
            color_layers: vec![],
            object: 0,
        };
        Scene { parts: vec![part], ..Default::default() }
    }

    #[test]
    fn palette_paint_and_export() {
        let p = Project::new(scene(), 4);
        assert_eq!(p.palette().len(), 2);
        let s = p.stats();
        assert_eq!(s.per_slot.iter().sum::<usize>(), 4);
        let parts = p.export_parts();
        assert_eq!(parts[0].paint.len(), 4);
        assert!(parts[0].paint.iter().all(|c| c == "4" || c == "8"));
    }

    #[test]
    fn height_scales_export() {
        let mut p = Project::new(scene(), 2);
        p.set_height_mm(40.0);
        let s = p.stats();
        assert!((s.size_mm[2] - 40.0).abs() < 1e-3);
    }

    #[test]
    fn size_per_axis_undo_and_reset() {
        let mut p = Project::new(scene(), 2);
        assert_eq!(p.original_size_mm(), [10.0, 1e-6, 20.0]);
        p.set_height_mm(40.0);
        assert_eq!(p.size_mm()[2], 40.0);
        assert!((p.stats().size_mm[0] - 20.0).abs() < 1e-3);

        // Only the width changes: the model stretches, the height stays.
        p.set_size_mm([35.0, p.size_mm()[1], 40.0]);
        let s = p.stats().size_mm;
        assert!((s[0] - 35.0).abs() < 1e-3 && (s[2] - 40.0).abs() < 1e-3, "{s:?}");
        assert!((p.axis_scale()[0] - 3.5).abs() < 1e-5);
        assert!((p.stretch()[0] - 1.75).abs() < 1e-5);

        // The typed size is kept exactly, with no rounding from the scale.
        p.set_size_mm([12.34, p.size_mm()[1], 56.78]);
        assert_eq!(p.size_mm()[0], 12.34);
        assert!((p.stats().size_mm[0] - 12.34).abs() < 1e-3);

        assert!(p.undo_size());
        assert_eq!(p.size_mm()[0], 35.0);
        p.reset_size();
        assert_eq!(p.size_mm(), p.original_size_mm());
        assert!((p.stats().size_mm[0] - 10.0).abs() < 1e-3);
        assert!(p.undo_size());
        assert_eq!(p.size_mm()[0], 35.0);
        while p.undo_size() {}
        assert_eq!(p.size_mm(), p.original_size_mm());
    }

    #[test]
    fn stretching_keeps_hand_painting() {
        let mut p = Project::new(scene(), 2);
        p.paint([2.0, -5.0, 3.0], [0.0, 1.0, 0.0], 0.5, 1).unwrap();
        p.end_stroke();
        let painted = p.painted_count();
        p.set_size_mm([30.0, 1.0, 20.0]);
        assert_eq!(p.painted_count(), painted);
    }

    #[test]
    fn brush_paints_and_undo_restores() {
        let mut p = Project::new(scene(), 2);
        let before = p.export_parts()[0].paint.clone();
        let target = if before[0] == "4" { 1 } else { 0 };
        let (_, changed) = p.paint([2.0, -5.0, 3.0], [0.0, 1.0, 0.0], 0.5, target).unwrap();
        assert!(!changed.is_empty());
        p.end_stroke();
        assert_ne!(p.export_parts()[0].paint, before);
        assert!(p.undo());
        assert_eq!(p.export_parts()[0].paint, before);
    }

    #[test]
    fn painting_over_painting_undoes_to_the_first_colour() {
        let mut p = Project::new(scene(), 2);
        let ray = ([2.0, -5.0, 3.0], [0.0, 1.0, 0.0]);
        p.paint(ray.0, ray.1, 0.5, 1).unwrap();
        p.end_stroke();
        let after_first = p.export_parts()[0].paint.clone();
        p.paint(ray.0, ray.1, 0.5, 0).unwrap();
        p.end_stroke();
        assert!(p.undo());
        assert_eq!(p.export_parts()[0].paint, after_first);
        // A repaint keeps the first stroke, not the undone one.
        p.set_height_mm(25.0);
        assert_eq!(p.export_parts()[0].paint, after_first);
        assert!(p.painted_count() > 0);
    }

    #[test]
    fn merge_can_be_undone() {
        let mut p = Project::new(scene(), 2);
        let before = p.export_parts()[0].paint.clone();
        p.merge_slots(1, 0);
        assert_eq!(p.undo_label(), Some("merge"));
        assert!(p.undo());
        assert_eq!(p.palette().len(), 2);
        assert_eq!(p.export_parts()[0].paint, before);
    }

    #[test]
    fn manual_merge_survives_a_repaint() {
        let mut p = Project::new(scene(), 2);
        assert_eq!(p.palette().len(), 2);
        p.merge_slots(1, 0);
        assert_eq!(p.palette().len(), 1);
        p.set_height_mm(30.0);
        assert!(p.export_parts()[0].paint.iter().all(|c| c == "4"));
    }

    #[test]
    fn colour_pieces_cover_every_slot() {
        let p = Project::new(scene(), 2);
        let pieces = p.color_pieces(0.5, false);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|x| !x.solid && !x.tris.is_empty()));
    }

    #[test]
    fn rotation_turns_the_size_and_lands_on_the_plate() {
        let mut p = Project::new(scene(), 2);
        p.set_height_mm(40.0);
        let painted_before = p.export_parts()[0].paint.clone();
        // A quarter turn about Y lays the 20 mm tall quad on its side.
        p.rotate(1, 90.0);
        assert!(p.is_rotated());
        let s = p.stats().size_mm;
        assert!((s[0] - 40.0).abs() < 1e-3 && (s[2] - 20.0).abs() < 1e-3, "{s:?}");
        assert!(p.size_mm().iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-3));
        assert!(!p.can_undo_size());
        let lowest = p.export_parts()[0].positions.iter().map(|v| v[2]).fold(f32::MAX, f32::min);
        assert!(lowest.abs() < 1e-4);
        // The painting follows the triangles.
        assert_eq!(p.export_parts()[0].paint, painted_before);

        p.rotate(1, -90.0);
        assert_eq!(p.rotation(), IDENTITY);
        p.rotate(2, 30.0);
        p.reset_rotation();
        assert!(!p.is_rotated());
        assert!((p.stats().size_mm[2] - 40.0).abs() < 1e-3);
    }

    /// A closed 10 × 10 × 20 box, its bottom half red and its top half blue.
    fn tower() -> Scene {
        let c = |i: usize| [(i & 1) as f32 * 10.0, ((i >> 1) & 1) as f32 * 10.0, (i >> 2) as f32 * 10.0];
        let positions: Vec<Vec3> = (0..12).map(c).collect();
        // Two stacked cubes sharing the middle ring of vertices (4..8).
        let mut tris = vec![];
        for (lo, hi) in [(0u32, 4u32), (4, 8)] {
            let q = |a: u32, b: u32, cc: u32, d: u32| [[a, b, cc], [a, cc, d]];
            let side = [
                q(lo, lo + 1, hi + 1, hi),
                q(lo + 2, hi + 2, hi + 3, lo + 3),
                q(lo, hi, hi + 2, lo + 2),
                q(lo + 1, lo + 3, hi + 3, hi + 1),
            ];
            tris.extend(side.into_iter().flatten());
        }
        tris.extend([[0, 2, 3], [0, 3, 1], [8, 9, 11], [8, 11, 10]]);
        let n = tris.len() as u32;
        let tri_source = tris.iter().map(|t: &[u32; 3]| u16::from(t.iter().any(|&v| v >= 8))).collect();
        let part = Part {
            name: "t".into(),
            positions,
            tri_loops: (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tris,
            tri_source,
            sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
            ..Default::default()
        };
        Scene { parts: vec![part], ..Default::default() }
    }

    /// Two towers side by side in one part, the second `lift` above the plate.
    fn twins(lift: f32) -> Scene {
        let mut scene = tower();
        let one = scene.parts[0].clone();
        let part = &mut scene.parts[0];
        let (base, loops) = (part.positions.len() as u32, part.tri_loops.len() as u32 * 3);
        part.positions.extend(one.positions.iter().map(|v| [v[0] + 25.0, v[1], v[2] + lift]));
        part.tris.extend(one.tris.iter().map(|t| t.map(|v| v + base)));
        part.tri_loops.extend(one.tri_loops.iter().map(|l| l.map(|v| v + loops)));
        part.tri_source.extend(&one.tri_source);
        scene
    }

    #[test]
    fn separating_pieces_keeps_geometry_and_paint_and_undoes() {
        let mut p = Project::new(twins(0.0), 2);
        let (pieces, map) = p.detect_pieces(0.01);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|x| x.closed && x.triangles == 20 && x.part == 0));
        assert!((pieces[0].size_mm[2] - 20.0).abs() < 1e-3);
        assert_eq!(map.len(), p.work.triangle_count());
        assert!(map.iter().all(|&k| k < 2));
        // A brush mark on the left tower.
        let target = 1 - p.trees[0][0].dominant();
        p.paint([-15.0, -50.0, 3.0], [0.0, 1.0, 0.0], 0.01, target).unwrap();
        p.end_stroke();
        let area = brush_area(&p, target);
        assert!(area > 0.0);
        let before = p.preview();
        let bounds = p.work.bounds().unwrap();
        let left = usize::from(pieces[0].centre[0] > pieces[1].centre[0]);
        let groups: Vec<Option<u32>> = (0..2).map(|k| (k != left).then_some(0)).collect();
        assert_eq!(p.separate_pieces(0.01, &groups, false), Ok(2));
        assert_eq!(p.undo_label(), Some("separate"));
        assert_eq!(p.scene.parts.len(), 2);
        assert_eq!(p.scene.parts.iter().map(|x| x.object).collect::<Vec<_>>(), [0, 1]);
        // The assembly position is kept: same box, same triangles, same painting.
        assert_eq!(p.work.bounds().unwrap(), bounds);
        assert_eq!(p.work.triangle_count(), 40);
        assert_eq!(p.painted_count(), 1);
        assert!((brush_area(&p, target) - area).abs() < 1e-4);
        for part in &p.scene.parts {
            assert!(cut::valid_solid(part, 1e-5));
        }
        assert_eq!(p.export_parts().iter().map(|x| x.object).collect::<Vec<_>>(), [0, 1]);
        assert!(p.undo());
        assert_eq!(p.object_count(), 1);
        assert_eq!(p.preview().positions, before.positions);
        assert_eq!(p.preview().labels, before.labels);
    }

    #[test]
    fn separated_pieces_can_rest_on_the_plate_beside_the_model() {
        let mut p = Project::new(twins(5.0), 2);
        let (pieces, _) = p.detect_pieces(0.01);
        let high = usize::from(pieces[1].centre[2] > pieces[0].centre[2]);
        let groups: Vec<Option<u32>> = (0..2).map(|k| (k == high).then_some(7)).collect();
        assert_eq!(p.separate_pieces(0.01, &groups, true), Ok(2));
        let parts = p.export_parts();
        let span = |o: u32, axis: usize| {
            let vs = parts.iter().filter(|x| x.object == o).flat_map(|x| x.positions.iter().map(move |v| v[axis]));
            vs.fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        };
        let (moved, stay) = (span(1, 2), span(0, 2));
        assert!(moved.0.abs() < 1e-3 && stay.0.abs() < 1e-3, "{moved:?} {stay:?}");
        assert!((span(1, 0).0 - span(0, 0).1 - 5.0).abs() < 1e-3);
    }

    #[test]
    fn separating_refuses_stale_or_empty_choices() {
        let mut p = Project::new(twins(0.0), 2);
        assert_eq!(p.separate_pieces(0.01, &[Some(0)], false), Err(SeparateError::Stale));
        assert_eq!(p.separate_pieces(0.01, &[None, None], false), Err(SeparateError::NothingToSeparate));
        // Both pieces into one new object: still one object, nothing to do.
        assert_eq!(p.separate_pieces(0.01, &[Some(3), Some(3)], false), Err(SeparateError::NothingToSeparate));
        assert!(!p.can_undo_cut());
        // A tolerance as wide as the gap between the towers makes them one piece.
        assert_eq!(p.detect_pieces(20.0).0.len(), 1);
    }

    #[test]
    fn a_continuous_tower_gets_a_colour_proposal_where_red_meets_blue() {
        let p = Project::new(tower(), 2);
        let found = p.propose_cuts(&[]);
        let z: Vec<_> = found.iter().filter(|x| x.axis == 2).collect();
        assert_eq!(z.len(), 1, "{found:?}");
        assert!((z[0].point[2] - 10.0).abs() < 0.5 && z[0].normal == [0.0, 0.0, 1.0]);
        assert!(matches!(z[0].signal, separate::Signal::Colour { .. }));
        // It is a proposal only: the model is still one piece.
        assert_eq!(p.detect_pieces(0.01).0.len(), 1);
    }

    #[test]
    fn an_unstretched_turn_moves_positions_and_keeps_the_painting() {
        let mut p = Project::new(tower(), 2);
        p.paint([0.0, -10.0, 3.0], [0.0, 1.0, 0.0], 0.5, 1).unwrap();
        p.end_stroke();
        let painted = p.painted_count();
        let before = p.preview().positions;
        let m = p.rotate(0, 90.0).expect("a whole-model turn");
        assert_eq!(p.painted_count(), painted);
        assert!(p.can_undo(), "the stroke can still be undone");
        // The preview it had, moved as reported, is the preview it has.
        let after = p.preview().positions;
        for (b, a) in before.chunks(3).zip(after.chunks(3)) {
            let moved = geom::add(mat_vec(m.matrix, [b[0], b[1], b[2]]), m.shift);
            assert!((0..3).all(|i| (moved[i] - a[i]).abs() < 1e-4), "{moved:?} vs {a:?}");
        }
        let s = p.stats().size_mm;
        assert!((s[1] - 20.0).abs() < 1e-3 && (s[2] - 10.0).abs() < 1e-3, "{s:?}");
        // A stretched model is rebuilt instead.
        p.set_size_mm([20.0, 20.0, 10.0]);
        assert!(p.rotate(2, 90.0).is_none());
    }

    #[test]
    fn a_cut_keeps_one_side_or_both_as_two_objects_and_can_be_undone() {
        let mut p = Project::new(tower(), 2);
        assert!(!p.cut(2, 1.5, true, false), "a plane past the model cuts nothing");
        assert!(p.cut(2, 0.25, true, false));
        let s = p.stats().size_mm;
        assert!((s[2] - 5.0).abs() < 1e-3, "{s:?}");
        assert_eq!(p.object_count(), 1);
        assert!(p.undo_cut());
        assert!((p.stats().size_mm[2] - 20.0).abs() < 1e-3);

        // Keeping the top too: it rests on its cut face beside the bottom.
        assert!(p.cut(2, 0.25, true, true));
        assert_eq!(p.object_count(), 2);
        let parts = p.export_parts();
        let top = parts.iter().find(|x| x.object == 1).unwrap();
        let low = top.positions.iter().map(|v| v[2]).fold(f32::MAX, f32::min);
        let high = top.positions.iter().map(|v| v[2]).fold(f32::MIN, f32::max);
        assert!(low.abs() < 1e-3 && (high - 15.0).abs() < 1e-3, "{low} {high}");
        let s = p.stats().size_mm;
        assert!((s[0] - 25.0).abs() < 1e-3 && (s[2] - 15.0).abs() < 1e-3, "{s:?}");
        // The cut face of the kept piece is closed: the bottom keeps its red, the top its blue.
        assert!(parts.iter().all(|x| x.tris.len() == x.paint.len()));
        assert!(p.undo_cut() && !p.can_undo_cut());
        assert_eq!(p.object_count(), 1);
    }

    fn brush_front(p: &mut Project) -> u8 {
        let target = 1 - p.trees[0][0].dominant();
        p.paint([2.0, -20.0, 3.0], [0.0, 1.0, 0.0], 0.01, target).unwrap();
        p.end_stroke();
        assert_eq!(p.painted_count(), 1);
        target
    }

    #[test]
    fn free_cut_matches_the_preview_after_turn_scale_and_stretch_and_undo_restores_paint() {
        for stretch in [false, true] {
            for normal in [[1.0, 0.0, 1.0], [0.31, -0.47, 0.8]] {
                let mut p = Project::new(tower(), 2);
                brush_front(&mut p);
                p.rotate(0, 27.0);
                if stretch {
                    p.set_size_mm([19.0, 11.0, 37.0]);
                } else {
                    p.set_height_mm(35.0);
                }
                let before = p.preview();
                let placement = p.placement;
                let (lo, hi) = p.work.bounds().unwrap();
                let point = geom::scale(geom::add(lo, hi), 0.5);
                let plane = cut::Plane::new(point, normal).unwrap();
                p.cut_plane(point, normal, true, false, false, &[]).unwrap();
                // Cutting recentres XY and drops the result to the bed; compare in the pre-cut frame.
                let shift = std::array::from_fn(|i| (p.placement[i] - placement[i]) * p.stretch[i]);
                assert!(p
                    .work
                    .parts
                    .iter()
                    .flat_map(|part| &part.positions)
                    .all(|&v| plane.distance(geom::sub(v, shift)) < 1e-4));
                assert_eq!(p.undo_label(), Some("cut"));
                assert!(p.painted_count() > 0);
                for part in &p.scene.parts {
                    assert!(cut::valid_solid(part, 1e-5));
                }
                let cap_normal = plane.normal;
                assert!(p.work.parts.iter().any(|part| (0..part.tris.len()).any(|ti| {
                    let [a, b, c] = part.corners(ti);
                    let n = geom::normalize(geom::cross(geom::sub(b, a), geom::sub(c, a)));
                    geom::dot(n, cap_normal) > 1.0 - 1e-5
                })));
                assert!(p.undo());
                assert_eq!(p.preview().positions, before.positions);
                assert_eq!(p.preview().labels, before.labels);
            }
        }
    }

    #[test]
    fn free_cut_selection_placement_and_failures_are_atomic() {
        let mut scene = tower();
        let mut second = scene.parts[0].clone();
        second.object = 1;
        for v in &mut second.positions {
            v[0] += 25.0;
        }
        scene.parts.push(second);
        for lay in [false, true] {
            let mut p = Project::new(scene.clone(), 2);
            let original = p.preview();
            let (lo, hi) = p.work.bounds().unwrap();
            let point = [lo[0] + 5.0, 0.0, (lo[2] + hi[2]) / 2.0];
            let second_tris = p.scene.parts[1].tris.len();
            p.cut_plane(point, [0.3, 0.0, 1.0], true, true, lay, &[0]).unwrap();
            assert_eq!(p.scene.parts.last().unwrap().tris.len(), second_tris);
            assert_eq!(p.object_count(), 3);
            assert!(p.export_parts().iter().all(|part| part.paint.len() == part.tris.len()));
            assert!(p.undo());
            assert_eq!(p.preview().positions, original.positions);
            for (point, normal, selected, error) in [
                ([0.0; 3], [0.0; 3], vec![], cut::CutError::InvalidPlane),
                ([0.0; 3], [0.0, 0.0, 1.0], vec![99], cut::CutError::InvalidPlane),
                ([0.0, 0.0, 999.0], [0.0, 0.0, 1.0], vec![], cut::CutError::Misses),
            ] {
                assert_eq!(p.cut_plane(point, normal, true, true, lay, &selected), Err(error));
                assert_eq!(p.preview().positions, original.positions);
                assert!(!p.can_undo_cut());
            }
        }
        let mut p = Project::new(self::scene(), 2); // open surface
        let before = p.preview().positions;
        assert_eq!(
            p.cut_plane([0.0, 0.0, 8.0], [0.0, 0.0, 1.0], true, false, false, &[]),
            Err(cut::CutError::InvalidGeometry)
        );
        assert_eq!(p.preview().positions, before);
    }

    fn brush_area(p: &Project, target: u8) -> f32 {
        p.work
            .parts
            .iter()
            .zip(&p.painted)
            .flat_map(|(part, labels)| {
                labels
                    .iter()
                    .enumerate()
                    .filter(|(_, label)| **label == target)
                    .map(move |(ti, _)| geom::area(&part.corners(ti)) * p.scale * p.scale)
            })
            .sum()
    }

    #[test]
    fn cut_keeps_brush_area_on_both_sides_and_exports_it() {
        for axis in 0..3 {
            for keep_low in [true, false] {
                for keep_other in [true, false] {
                    let mut p = Project::new(tower(), 2);
                    let target = brush_front(&mut p);
                    let before = p.export_parts()[0].paint.clone();
                    let t = if axis == 2 { 0.25 } else { 0.5 };
                    assert!(p.cut(axis, t, keep_low, keep_other));
                    // The brush covers one 50 mm² triangle on the front wall.
                    // X=0 leaves 12.5 on the left; Z=5 leaves 37.5 below.
                    let below = [12.5, 50.0, 37.5][axis];
                    let expected = if keep_other {
                        50.0
                    } else if keep_low {
                        below
                    } else {
                        50.0 - below
                    };
                    assert!(
                        (brush_area(&p, target) - expected).abs() < 1e-3,
                        "axis={axis}, low={keep_low}, both={keep_other}"
                    );
                    for (pi, part) in p.export_parts().iter().enumerate() {
                        for (ti, &label) in p.painted[pi].iter().enumerate() {
                            if label != NO_LABEL {
                                assert_eq!(part.paint[ti], encode::encode(&Node::Leaf(target)));
                            }
                        }
                    }
                    assert!(p.undo_cut());
                    assert_eq!(p.export_parts()[0].paint, before);
                    assert_eq!(p.painted_count(), 1);
                }
            }
        }
    }

    #[test]
    fn cut_paint_survives_repeated_cuts_rotation_resize_and_stroke_undo() {
        let mut p = Project::new(tower(), 2);
        let target = brush_front(&mut p);
        assert!(p.cut(2, 0.25, true, true));
        let after_first: Vec<_> = p.export_parts().iter().map(|part| part.paint.clone()).collect();
        assert!(p.cut(0, 0.5, true, true));
        assert!((brush_area(&p, target) - 50.0).abs() < 1e-3);
        assert!(p.undo_cut());
        assert_eq!(p.export_parts().iter().map(|part| part.paint.clone()).collect::<Vec<_>>(), after_first);
        p.rotate(2, 90.0);
        p.set_height_mm(30.0);
        assert_eq!(p.export_parts().iter().map(|part| part.paint.clone()).collect::<Vec<_>>(), after_first);
        let old_labels = p.painted.clone();
        let c = p.work.parts[0].corners(0);
        let centre = geom::at(&c, [1.0 / 3.0; 3]);
        let n = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
        p.paint(geom::add(centre, geom::scale(n, 5.0)), geom::scale(n, -1.0), 0.01, 1 - target).unwrap();
        p.end_stroke();
        assert!(p.undo());
        assert_eq!(p.painted, old_labels);
        assert!(p.undo_cut());
        assert_eq!(p.painted_count(), 1);
    }

    #[test]
    fn cut_caps_do_not_inherit_brush_paint_and_missed_cuts_leave_it_intact() {
        let mut p = Project::new(tower(), 2);
        let target = brush_front(&mut p);
        let before = p.export_parts()[0].paint.clone();
        for (axis, t) in [(3, 0.5), (2, f32::NAN), (2, 1.5)] {
            assert!(!p.cut(axis, t, true, false));
            assert_eq!(p.export_parts()[0].paint, before);
            assert!(!p.can_undo_cut());
        }
        assert!(p.cut(2, 0.25, true, false));
        assert!((brush_area(&p, target) - 37.5).abs() < 1e-3);
        let part = &p.work.parts[0];
        let caps: Vec<_> =
            (0..part.tris.len()).filter(|&ti| part.corners(ti).iter().all(|v| (v[2] - 5.0).abs() < 1e-5)).collect();
        assert!(!caps.is_empty());
        assert!(caps.iter().all(|&ti| p.painted[0][ti] == NO_LABEL));
    }

    #[test]
    fn cut_paint_undo_tracks_palette_merges_and_their_undo() {
        for undo_merge in [false, true] {
            let mut p = Project::new(tower(), 2);
            let target = brush_front(&mut p);
            assert!(p.cut(2, 0.25, true, true));
            p.merge_slots(target as usize, (1 - target) as usize);
            if undo_merge {
                assert!(p.undo());
            }
            assert!(p.undo_cut());
            assert_eq!(p.painted_count(), 1);
            assert_eq!(p.painted[0][0], if undo_merge { target } else { 0 });
            assert_eq!(p.export_parts()[0].paint[0], encode::encode(&Node::Leaf(p.painted[0][0])));
        }
    }

    #[test]
    fn cut_keeps_brush_paint_with_prior_stretch_or_simplification() {
        for simplified in [false, true] {
            let mut p = Project::new(tower(), 2);
            if simplified {
                p.set_mesh_ops(MeshOps { keep_ratio: 0.8, ..p.mesh_ops() });
            } else {
                p.set_size_mm([20.0, 10.0, 20.0]);
            }
            // Paint the entire surface, so any unpainted surviving surface
            // after rebuilding is a transfer failure, independent of retriangulation.
            p.paint([0.0, -20.0, 3.0], [0.0, 1.0, 0.0], 100.0, 1).unwrap();
            p.end_stroke();
            let before = p.export_parts()[0].paint.clone();
            assert!(p.cut(2, 0.25, true, true));
            let mut surfaces = 0;
            for (part, labels) in p.work.parts.iter().zip(&p.painted) {
                for (ti, &label) in labels.iter().enumerate() {
                    let c = part.corners(ti);
                    // Both caps are horizontal, at the top of the low piece
                    // or the bottom of the high piece.
                    if (c[0][2] - c[1][2]).abs() > 1e-4 || (c[0][2] - c[2][2]).abs() > 1e-4 {
                        assert_eq!(label, 1, "simplified={simplified}");
                        surfaces += 1;
                    }
                }
            }
            assert!(surfaces > 0);
            assert!(p.undo_cut());
            assert_eq!(p.export_parts()[0].paint, before);
        }
    }

    #[test]
    fn cut_keeps_different_brush_labels_on_children_of_a_flattened_source_face() {
        let mut p = Project::new(tower(), 2);
        p.set_mesh_ops(MeshOps { flatten_mm: 2.0, ..p.mesh_ops() });
        let origins = p.shape.parts[0].origin.as_ref().unwrap();
        let children: Vec<_> = origins.iter().enumerate().filter(|(_, (src, _))| *src == 1).map(|(ti, _)| ti).collect();
        assert!(children.len() > 1);
        let ti = *children.last().unwrap();
        let c = p.work.parts[0].corners(ti);
        let centre = geom::at(&c, [1.0 / 3.0; 3]);
        let n = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
        let target = 1 - p.trees[0][ti].dominant();
        p.paint(geom::add(centre, geom::scale(n, 5.0)), geom::scale(n, -1.0), 0.01, target).unwrap();
        p.end_stroke();
        assert_eq!(p.painted_count(), 1);
        let area = brush_area(&p, target);
        let before = p.export_parts()[0].paint.clone();
        // Cut above this face: its distinct painted and unpainted children stay.
        assert!(p.cut(2, 0.5, true, false));
        assert_eq!(p.painted_count(), 1);
        assert!((brush_area(&p, target) - area).abs() < 1e-3);
        assert!(p.undo_cut());
        assert_eq!(p.export_parts()[0].paint, before);
    }

    #[test]
    fn preview_offsets_cover_all_leaves() {
        let p = Project::new(scene(), 2);
        let pv = p.preview();
        assert_eq!(pv.offsets.len(), 5);
        assert_eq!(*pv.offsets.last().unwrap() as usize, pv.labels.len());
        assert_eq!(pv.positions.len(), pv.labels.len() * 9);
    }
    /// A wedge resting on its sloped side would overhang; on its wide flat side it does not.
    #[test]
    fn best_rotation_rests_the_model_on_its_flat_side_and_keeps_paint() {
        let positions = vec![[0., 0., 0.], [20., 0., 0.], [20., 10., 0.], [0., 10., 0.], [0., 0., 10.], [0., 10., 10.]];
        // Right triangular prism: the sloped face joins the far bottom edge to the top edge.
        let tris = vec![[0, 2, 1], [0, 3, 2], [0, 1, 4], [3, 5, 2], [0, 4, 5], [0, 5, 3], [1, 2, 5], [1, 5, 4]];
        let part = Part {
            name: "wedge".into(),
            tri_loops: tris.clone(),
            tri_source: vec![0; tris.len()],
            sources: vec![ColorSource::Flat([0.8; 3])],
            positions,
            tris,
            ..Default::default()
        };
        let mut p = Project::new(Scene { parts: vec![part], ..Default::default() }, 1);
        // Upside down: the wide base becomes the top and the slope hangs over the plate.
        let flipped = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
        p.set_rotation(flipped);
        let labels = p.preview().labels;
        let r = p.best_rotation(45.0);
        assert_ne!(r, flipped);
        p.set_rotation(r);
        assert_eq!(p.preview().labels, labels, "turning keeps the triangles and their paint");
        // Resting on a flat side nothing overhangs, so another search changes nothing.
        assert_eq!(p.best_rotation(45.0), p.rotation());
        let overhang = p.printability(crate::printability::Params::for_nozzle(0.4));
        assert_eq!(overhang.checks.iter().find(|c| c.code == "overhang").unwrap().count, 0);
        assert_eq!(p.undo_label(), Some("fix"));
        assert!(p.undo());
        assert_eq!(p.rotation(), flipped);
    }

    /// Flattening squashes the walls under the cut into lines; none may be left behind.
    #[test]
    fn a_flattened_base_leaves_no_degenerate_faces() {
        let positions = vec![
            [0., 0., 0.],
            [10., 0., 0.],
            [10., 10., 0.],
            [0., 10., 0.],
            [0., 0., 10.],
            [10., 0., 10.],
            [10., 10., 10.],
            [0., 10., 10.],
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
        let part = Part {
            name: "box".into(),
            tri_loops: tris.clone(),
            tri_source: vec![0; tris.len()],
            sources: vec![ColorSource::Flat([0.8; 3])],
            positions,
            tris,
            ..Default::default()
        };
        let mut p = Project::new(Scene { parts: vec![part], ..Default::default() }, 1);
        for deep_clean in [false, true] {
            p.set_mesh_ops(MeshOps { flatten_mm: 0.8, deep_clean, ..p.mesh_ops() });
            let r = p.printability(crate::printability::Params::for_nozzle(0.4));
            for code in ["degenerate", "openEdges", "nonManifoldEdges", "winding"] {
                assert_eq!(
                    r.checks.iter().find(|c| c.code == code).unwrap().count,
                    0,
                    "{code}, deep clean {deep_clean}"
                );
            }
            assert_eq!(p.preview().labels.len() * 9, p.preview().positions.len());
        }
    }

    #[test]
    fn a_model_only_stands_where_its_centre_is_over_its_footprint() {
        let square = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]];
        let up = [0.0, 0.0, 1.0];
        assert!(stands(&square, [5.0, 5.0, 4.0], up, 0.1));
        // Over the edge, or on a single corner, it tips over.
        assert!(!stands(&square, [12.0, 5.0, 4.0], up, 0.1));
        assert!(!stands(&square[..1], [0.0, 0.0, 4.0], up, 0.1));
    }

    #[test]
    fn fix_undo_restores_paint_and_geometry() {
        let mut p = Project::new(scene(), 2);
        p.set_mesh_ops(MeshOps { repair: false, ..p.mesh_ops() });
        p.paint([1.0, 1.0, 30.0], [0.0, 0.0, -1.0], 100.0, 1);
        p.end_stroke();
        let before = p.preview();
        let palette = p.palette().to_vec();
        p.fix_mesh(MeshOps { repair: true, orient: true, ..p.mesh_ops() });
        assert_eq!(p.undo_label(), Some("fix"));
        assert!(p.mesh_ops().orient);
        assert!(p.undo());
        assert!(!p.mesh_ops().repair && !p.mesh_ops().orient);
        assert_eq!(p.preview().positions, before.positions);
        assert_eq!(p.preview().labels, before.labels);
        assert_eq!(p.preview().source_rgb, before.source_rgb);
        assert_eq!(p.palette(), palette);
    }
    #[test]
    fn fix_undo_keeps_the_active_layer_profile_and_bounded_history() {
        let mut p = Project::new(scene(), 2);
        p.set_mesh_ops(MeshOps { repair: false, ..p.mesh_ops() });
        p.paint([1., 1., 30.], [0., 0., -1.], 100., 1);
        p.end_stroke();
        let before = p.preview().labels;
        p.fix_mesh(MeshOps { repair: true, ..p.mesh_ops() });
        p.set_print_ops(PrintOps { nozzle_mm: 0.4, layer_mm: 0.12 });
        assert!(p.undo());
        assert_eq!(p.print_ops().layer_mm, 0.12);
        assert_eq!(p.preview().labels, before);
        // A fix and its revert are two steps; neither duplicates older snapshots.
        for _ in 0..20 {
            p.fix_mesh(MeshOps { fill_holes: true, ..p.mesh_ops() });
            p.fix_mesh(MeshOps { fill_holes: false, ..p.mesh_ops() });
            assert!(p.undo.len() <= 2);
            p.set_mesh_ops(MeshOps { repair: false, ..p.mesh_ops() });
            assert!(p.undo.is_empty());
        }
        // Asking for what is already there changes nothing and records nothing.
        p.fix_mesh(p.mesh_ops());
        assert!(!p.can_undo());
        p.fix_mesh(MeshOps { repair: true, ..p.mesh_ops() });
        p.set_print_ops(PrintOps { nozzle_mm: 0.8, layer_mm: 0.3 });
        // A nozzle change rebuilds paint, intentionally invalidating earlier topology undo.
        assert!(!p.can_undo());
        assert_eq!(p.printability(crate::printability::Params::for_nozzle(0.8)).nozzle_mm, Some(0.8));
    }
}
