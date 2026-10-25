//! The open document and its use cases: palette, size, mesh operations, colour
//! clean-up, hand painting with undo, preview data and export data.

use crate::bvh::{Bvh, RayHit, TriSoup};
use crate::color::{to_u8, Classifier, Rgb};
use crate::encode;
use crate::estimate::{self, SwapEstimate};
use crate::field::ColorField;
use crate::geom::{self, Vec3, ROOT};
use crate::mesh::{Mesh, MeshOps, RepairReport};
use crate::optimize;
use crate::paint::{self, Node, PaintParams};
use crate::palette;
use crate::scene::Scene;
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

struct Change {
    part: u32,
    tri: u32,
    node: Node,
    /// Hand-painted label before the change (NO_LABEL when none).
    painted: u8,
}

/// Everything a slot merge changes, to undo it.
struct SlotSnapshot {
    palette: Vec<Rgb>,
    targets: Vec<(Rgb, u8)>,
    trees: Vec<Vec<Node>>,
    painted: Vec<Vec<u8>>,
}

enum UndoEntry {
    Stroke(Vec<Change>),
    Slots(Box<SlotSnapshot>),
}

const NO_LABEL: u8 = u8::MAX;

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
        let source_positions = scene.parts.iter().map(|p| p.positions.clone()).collect();
        let mut p = Self {
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
            || (ops.min_piece_mm - self.mesh_ops.min_piece_mm).abs() > 1e-4;
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
            .map(|(part, nodes)| ExportPart {
                name: part.name.clone(),
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
    fn preview_offsets_cover_all_leaves() {
        let p = Project::new(scene(), 2);
        let pv = p.preview();
        assert_eq!(pv.offsets.len(), 5);
        assert_eq!(*pv.offsets.last().unwrap() as usize, pv.labels.len());
        assert_eq!(pv.positions.len(), pv.labels.len() * 9);
    }
}
