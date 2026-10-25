//! The open document and its use cases: palette, size, mesh operations, colour
//! clean-up, hand painting with undo, preview data and export data.

use crate::bvh::{Bvh, RayHit, TriSoup};
use crate::color::{to_u8, Classifier, Rgb};
use crate::cut;
use crate::encode;
use crate::estimate::{self, SwapEstimate};
use crate::field::ColorField;
use crate::geom::{self, Vec3, ROOT};
use crate::mesh::{Mesh, MeshOps, RepairReport};
use crate::optimize;
use crate::paint::{self, Node, PaintParams};
use crate::palette;
use crate::scene::{Part, Scene};
use crate::split::{self, ColorPiece};

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
}

#[derive(Clone)]
enum UndoEntry {
    Stroke(Vec<Change>),
    Slots(Box<SlotSnapshot>),
    Geometry(Box<Project>),
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
    /// on its cut face beside the model. Both openings are capped. Hand painting
    /// goes, as the triangles change. False when the plane misses the model.
    pub fn cut(&mut self, axis: usize, t: f32, keep_low: bool, keep_other: bool) -> bool {
        let (Some((lo, hi)), Some((_, shape_hi))) = (self.work.bounds(), self.shape.bounds()) else { return false };
        if axis > 2 || !t.is_finite() {
            return false;
        }
        // The preview is the final shape: a flattened base moved it down from where the scene is.
        let at = lo[axis] + (hi[axis] - lo[axis]) * t.clamp(0.0, 1.0) + (shape_hi[axis] - hi[axis]);
        let value = at / self.stretch[axis];
        let eps = geom::len(self.source_size) * 1e-6;
        let (mut kept, mut other) = (vec![], vec![]);
        for (part, src) in self.scene.parts.iter().zip(&self.source_positions) {
            let whole = Part { positions: src.clone(), ..part.clone() };
            let [low, high] = cut::split(&whole, axis, value, eps);
            let (k, o) = if keep_low { (low, high) } else { (high, low) };
            kept.extend((!k.tris.is_empty()).then_some(k));
            other.extend((!o.tris.is_empty()).then_some(o));
        }
        if kept.is_empty() || other.is_empty() {
            return false;
        }
        let parts = std::mem::take(&mut self.scene.parts);
        self.cut_history.push(CutSnapshot { parts, unrotated: self.unrotated.clone() });
        if keep_other {
            // Its cut face looks back at the kept side; turned to face the plate.
            let sign = if keep_low { -1.0 } else { 1.0 };
            let turn = match axis {
                0 => axis_turn(1, 90.0 * sign),
                1 => axis_turn(0, -90.0 * sign),
                _ if keep_low => IDENTITY,
                _ => axis_turn(0, 180.0),
            };
            let first_new = kept.iter().chain(&other).map(|p| p.object).max().unwrap_or(0) + 1;
            for p in &mut other {
                p.object += first_new;
                p.name = format!("{} ({})", p.name, p.object + 1);
                for v in &mut p.positions {
                    *v = mat_vec(turn, *v);
                }
            }
            let bounds = |ps: &[Part]| {
                let mut it = ps.iter().flat_map(|p| p.positions.iter().copied());
                let first = it.next().unwrap_or([0.0; 3]);
                it.fold((first, first), |(lo, hi), p| {
                    (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn::<f32, 3, _>(|i| hi[i].max(p[i])))
                })
            };
            let ((klo, khi), (olo, ohi)) = (bounds(&kept), bounds(&other));
            // Beside the model along X, 5 mm apart, centred with it on Y and on the plate.
            let gap = 5.0 / self.scale;
            let shift = [khi[0] + gap - olo[0], (klo[1] + khi[1] - olo[1] - ohi[1]) / 2.0, klo[2] - olo[2]];
            for v in other.iter_mut().flat_map(|p| p.positions.iter_mut()) {
                *v = geom::add(*v, shift);
            }
            kept.append(&mut other);
        }
        self.scene.parts = kept;
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
        self.rebuild_paint();
        true
    }

    pub fn can_undo_cut(&self) -> bool {
        !self.cut_history.is_empty()
    }

    /// Printed objects: 1 for the model, more once pieces cut off it are kept.
    pub fn object_count(&self) -> usize {
        self.scene.parts.iter().map(|p| p.object as usize + 1).max().unwrap_or(1)
    }

    /// Puts back what the last cut took, in the orientation the model has now.
    pub fn undo_cut(&mut self) -> bool {
        let Some(s) = self.cut_history.pop() else { return false };
        self.scene.parts = s.parts;
        self.unrotated = s.unrotated;
        self.place_from_file();
        self.rebuild_geometry();
        self.rebuild_paint();
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
        for l in self.painted.iter_mut().flatten() {
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
        };
        for t in &mut self.targets {
            t.1 = map[t.1 as usize];
        }
        for nodes in &mut self.trees {
            for n in nodes.iter_mut() {
                *n = n.relabel(map);
            }
        }
        for l in self.painted.iter_mut().flatten() {
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
