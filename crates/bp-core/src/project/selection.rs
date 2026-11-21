//! Picking a region of the surface with a brush or by colour, and cutting it
//! out: removed, or extracted as objects of its own, closed with a shell or a
//! cap. The selection is a mask over the preview's triangles; it never changes
//! the painting. Cutting reads it on the scene's triangles and is one undoable step.

use super::{rotation_between, CutPaint, PaintTransfer, Project, NO_LABEL};
use crate::bvh::RayHit;
use crate::color;
use crate::field::ColorField;
use crate::geom::{self, Vec3};
use crate::paint::Node;
use crate::refine;
use crate::region::{self, Closure, RegionError};
use crate::scene::Part;

/// What makes two triangles the same colour when selecting by colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColourMatch {
    /// The same material of the model as loaded.
    Material,
    /// The same filament slot (the one covering most of the triangle).
    Filament,
    /// The model's colour at the triangle's centre within `delta_e` (CIE76) of the clicked one.
    Similar { delta_e: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefineFailure {
    /// The mesh was simplified, flattened or reshaped: its triangles no longer match the model's.
    Reshaped,
    /// No triangle has a colour border running across it.
    Nothing,
}

/// What happens to the selected region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionAction {
    /// The region goes; the rest is closed where it was.
    Remove,
    /// Each connected region becomes an object of its own, or with `group`
    /// all of them one object. With `lay_flat` they rest on the plate beside
    /// the model (a region on its own with its outer face up); otherwise they
    /// stay where they are.
    Extract { lay_flat: bool, group: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionFailure {
    Region(RegionError),
    /// Removing it would leave nothing.
    Everything,
    /// A piece would enclose no volume: a cap lying on the surface it closes.
    Flat,
}

impl From<RegionError> for RegionFailure {
    fn from(e: RegionError) -> Self {
        Self::Region(e)
    }
}

/// What cutting the selection would make, in preview coordinates and final millimetres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionPreview {
    /// The faces that would close the rest.
    pub closure: Vec<[Vec3; 3]>,
    /// The surface of each region as it would come off, and the way it comes off.
    pub pieces: Vec<Vec<[Vec3; 3]>>,
    pub directions: Vec<Vec3>,
    pub piece_mm3: Vec<f32>,
    pub rest_mm3: f32,
    /// Every result is a closed surface (a part that was open stays open).
    pub closed: bool,
    /// Pieces narrower than two nozzle widths across their smallest side: too small to print well.
    pub small: usize,
}

/// Scene triangles cut, per part: the split, and the scene triangles it was made from.
type Splits = Vec<Option<region::Split>>;

impl Project {
    // ---- the selection mask -------------------------------------------------

    /// Selects (or with `add` false, deselects) the triangles within
    /// `radius_mm` of where the ray lands. With `through` every triangle in the
    /// sphere counts, walls behind included; otherwise only those facing the
    /// ray and joined to the one under it. Returns the hit and the global ids of
    /// the triangles that changed.
    pub fn select(
        &mut self,
        origin: Vec3,
        dir: Vec3,
        radius_mm: f32,
        add: bool,
        through: bool,
    ) -> Option<(RayHit, Vec<u32>)> {
        let hit = self.pick(origin, dir)?;
        let mut found = vec![hit.tri];
        self.picker.0.sphere(&self.picker.1, hit.point, radius_mm / self.scale, &mut found);
        found.sort_unstable();
        found.dedup();
        let (hp, ht) = self.picker.1.owner[hit.tri as usize];
        if !through {
            // Only the surface the ray sees: facing it, and reachable from the hit across the footprint.
            let near: std::collections::HashSet<(u32, u32)> = found
                .iter()
                .map(|&t| self.picker.1.owner[t as usize])
                .filter(|&(p, t)| {
                    let c = self.work.parts[p as usize].corners(t as usize);
                    p == hp && geom::dot(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])), dir) < 0.0
                })
                .collect();
            let mut reached = std::collections::HashSet::from([(hp, ht)]);
            let mut stack = vec![ht];
            while let Some(t) = stack.pop() {
                for &(o, _) in &self.adjacency[hp as usize][t as usize] {
                    if near.contains(&(hp, o)) && reached.insert((hp, o)) {
                        stack.push(o);
                    }
                }
            }
            found.retain(|&t| reached.contains(&self.picker.1.owner[t as usize]));
        }
        let mut changed = vec![];
        for t in found {
            let (p, i) = self.picker.1.owner[t as usize];
            let mark = &mut self.selection[p as usize][i as usize];
            if *mark != add {
                *mark = add;
                changed.push(self.tri_offsets[p as usize] + i);
            }
        }
        Some((hit, changed))
    }

    /// Global ids of the selected triangles.
    pub fn selection(&self) -> Vec<u32> {
        let mut out = vec![];
        for (pi, marks) in self.selection.iter().enumerate() {
            out.extend(marks.iter().enumerate().filter(|m| *m.1).map(|(t, _)| self.tri_offsets[pi] + t as u32));
        }
        out
    }

    /// Selected triangles and how many separate regions they make (joined across edges).
    pub fn selection_regions(&self) -> (usize, usize) {
        let mut count = 0;
        let mut regions = 0;
        for (marks, adj) in self.selection.iter().zip(&self.adjacency) {
            let mut seen = vec![false; marks.len()];
            for start in 0..marks.len() {
                if !marks[start] || seen[start] {
                    continue;
                }
                regions += 1;
                seen[start] = true;
                let mut stack = vec![start];
                while let Some(t) = stack.pop() {
                    count += 1;
                    for &(o, _) in &adj[t] {
                        if marks[o as usize] && !seen[o as usize] {
                            seen[o as usize] = true;
                            stack.push(o as usize);
                        }
                    }
                }
            }
        }
        (count, regions)
    }

    /// Replaces the selection with the given global triangle ids.
    pub fn set_selection(&mut self, ids: &[u32]) {
        for marks in &mut self.selection {
            marks.fill(false);
        }
        for &id in ids {
            let pi = self.tri_offsets.partition_point(|&o| o <= id).saturating_sub(1);
            if let Some(m) = self.selection.get_mut(pi).and_then(|m| m.get_mut((id - self.tri_offsets[pi]) as usize)) {
                *m = true;
            }
        }
    }

    pub fn clear_selection(&mut self) {
        for marks in &mut self.selection {
            marks.fill(false);
        }
    }

    pub fn invert_selection(&mut self) {
        for m in self.selection.iter_mut().flatten() {
            *m = !*m;
        }
    }

    /// Adds every triangle beside the selection.
    pub fn grow_selection(&mut self) {
        for (marks, adj) in self.selection.iter_mut().zip(&self.adjacency) {
            let before = marks.clone();
            for (t, &on) in before.iter().enumerate() {
                if on {
                    for &(o, _) in &adj[t] {
                        marks[o as usize] = true;
                    }
                }
            }
        }
    }

    /// Takes in whole every connected piece the selection touches.
    pub fn select_connected(&mut self) {
        for (marks, adj) in self.selection.iter_mut().zip(&self.adjacency) {
            let mut stack: Vec<usize> = (0..marks.len()).filter(|&t| marks[t]).collect();
            while let Some(t) = stack.pop() {
                for &(o, _) in &adj[t] {
                    if !marks[o as usize] {
                        marks[o as usize] = true;
                        stack.push(o as usize);
                    }
                }
            }
        }
    }

    /// Evens the border: a selected triangle with at most one selected
    /// neighbour leaves it, one outside with two or more joins it.
    pub fn smooth_selection(&mut self) {
        for (marks, adj) in self.selection.iter_mut().zip(&self.adjacency) {
            let before = marks.clone();
            for (t, m) in marks.iter_mut().enumerate() {
                let near = adj[t].iter().filter(|(o, _)| before[*o as usize]).count();
                if before[t] && near <= 1 && !adj[t].is_empty() {
                    *m = false;
                } else if !before[t] && near >= 2 {
                    *m = true;
                }
            }
        }
    }

    // ---- cutting the region out ---------------------------------------------

    /// The selection on the scene's triangles: a scene triangle is in when
    /// most of its preview triangles, by area, are.
    fn scene_selection(&self, pi: usize) -> Vec<bool> {
        let n = self.scene.parts[pi].tris.len();
        let mut area = vec![(0.0f32, 0.0f32); n];
        let work = &self.work.parts[pi];
        for (ti, src) in self.work_sources(pi).into_iter().enumerate() {
            let Some(s) = src.filter(|&s| (s as usize) < n) else { continue };
            let a = geom::area(&work.corners(ti)).max(f32::MIN_POSITIVE);
            area[s as usize].1 += a;
            if self.selection[pi][ti] {
                area[s as usize].0 += a;
            }
        }
        area.iter().map(|&(on, all)| all > 0.0 && on * 2.0 > all).collect()
    }

    /// Splits every part the selection reaches, in the scene's stretched
    /// coordinates (millimetres before the user's scale).
    fn region_splits(&self, closure_mm: Closure, action: RegionAction) -> Result<Splits, RegionFailure> {
        let closure = match closure_mm {
            Closure::Shell { thickness, clearance } => {
                Closure::Shell { thickness: thickness / self.scale, clearance: clearance / self.scale }
            }
            Closure::Cap { depth } => Closure::Cap { depth: depth / self.scale },
        };
        let mut splits = vec![];
        let mut any = false;
        for pi in 0..self.scene.parts.len() {
            let selected = self.scene_selection(pi);
            if !selected.contains(&true) {
                splits.push(None);
                continue;
            }
            any = true;
            splits.push(Some(region::split(&self.scene.parts[pi], &selected, closure)?));
        }
        if !any {
            return Err(RegionError::Empty.into());
        }
        let left = self.scene.parts.iter().zip(&splits).any(|(p, s)| match s {
            Some(s) => !s.rest.part.tris.is_empty(),
            None => !p.tris.is_empty(),
        });
        if action == RegionAction::Remove && !left {
            return Err(RegionFailure::Everything);
        }
        // A thousandth of a cubic millimetre is no piece.
        let tiny = 1e-3 / self.scale.powi(3);
        for s in splits.iter().flatten() {
            let rest = (!s.rest.part.tris.is_empty()).then_some(&s.rest);
            if s.regions.iter().chain(rest).any(|p| p.volume.abs() <= tiny) {
                return Err(RegionFailure::Flat);
            }
        }
        Ok(splits)
    }

    /// What cutting the selection out would make, without changing anything.
    pub fn preview_region(&self, closure_mm: Closure, action: RegionAction) -> Result<RegionPreview, RegionFailure> {
        let splits = self.region_splits(closure_mm, action)?;
        // The preview is the final shape: a flattened base moved it down from where the scene is.
        let lift = match (self.work.bounds(), self.shape.bounds()) {
            (Some((_, hi)), Some((_, shape_hi))) => geom::sub(shape_hi, hi),
            _ => [0.0; 3],
        };
        let at = |c: [Vec3; 3]| c.map(|v| geom::sub(v, lift));
        let mm3 = self.scale.powi(3);
        let mut out = RegionPreview { closed: true, ..Default::default() };
        for (s, part) in splits.iter().zip(&self.scene.parts) {
            let Some(s) = s else {
                out.rest_mm3 += region::volume(part) * mm3;
                continue;
            };
            out.closure.extend(s.closure.iter().map(|&c| at(c)));
            for (p, d) in s.regions.iter().zip(&s.directions) {
                let (lo, hi) = p.part.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                    (std::array::from_fn(|i| lo[i].min(v[i])), std::array::from_fn(|i| hi[i].max(v[i])))
                });
                let thinnest = (0..3).map(|i| hi[i] - lo[i]).fold(f32::MAX, f32::min) * self.scale;
                out.small += usize::from(thinnest < 2.0 * self.print_ops.nozzle_mm);
                out.pieces.push((0..p.part.tris.len()).map(|t| at(p.part.corners(t))).collect());
                out.directions.push(*d);
                out.piece_mm3.push(p.volume * mm3);
                out.closed &= p.closed;
            }
            if !s.rest.part.tris.is_empty() {
                out.rest_mm3 += s.rest.volume * mm3;
                out.closed &= s.rest.closed;
            }
        }
        Ok(out)
    }

    /// Cuts the selection out as one undoable step; the selection is cleared.
    /// Faces that stay keep their colours and hand painting; a shell's inner
    /// faces take the painting of the face they were pushed in from, and the
    /// walls and caps the colour of the surface where they meet it. Returns
    /// how many objects there are now.
    pub fn apply_region(&mut self, closure_mm: Closure, action: RegionAction) -> Result<usize, RegionFailure> {
        let splits = self.region_splits(closure_mm, action)?;
        let transfers: Vec<Option<PaintTransfer>> = self
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
        let unstretch = |p: &mut Part| {
            for v in &mut p.positions {
                *v = std::array::from_fn(|i| v[i] / stretch[i]);
            }
        };
        // Brush marks are found again on the surface as it is before anything moves.
        let paint_of = |pi: usize, piece: &region::Piece| {
            transfers[pi].as_ref().map(|_| CutPaint {
                part: pi,
                origins: piece.origins.clone(),
                corners: (0..piece.part.tris.len()).map(|t| piece.part.corners(t)).collect(),
            })
        };
        let first_new_object = self.scene.parts.iter().map(|p| p.object).max().unwrap_or(0) + 1;
        let (mut kept, mut kept_paint) = (vec![], vec![]);
        let mut moved: Vec<Vec<(Part, Option<CutPaint>)>> = vec![];
        for (pi, (part, split)) in self.scene.parts.iter().zip(splits).enumerate() {
            let Some(split) = split else {
                let mut whole = part.clone();
                let paint = transfers[pi].as_ref().map(|_| CutPaint {
                    part: pi,
                    origins: (0..whole.tris.len() as u32).map(Some).collect(),
                    corners: (0..whole.tris.len()).map(|t| whole.corners(t)).collect(),
                });
                unstretch(&mut whole);
                kept.push(whole);
                kept_paint.push(paint);
                continue;
            };
            if !split.rest.part.tris.is_empty() {
                let paint = paint_of(pi, &split.rest);
                let mut rest = split.rest.part;
                unstretch(&mut rest);
                kept.push(rest);
                kept_paint.push(paint);
            }
            if let RegionAction::Extract { lay_flat, group } = action {
                for (piece, direction) in split.regions.iter().zip(&split.directions) {
                    let paint = paint_of(pi, piece);
                    let mut p = piece.part.clone();
                    // Grouped pieces keep their places relative to each other.
                    if lay_flat && !group {
                        // Outer face up, so it rests on the faces that closed it.
                        let turn = rotation_between(*direction, [0.0, 0.0, 1.0]);
                        for v in &mut p.positions {
                            *v = super::mat_vec(turn, *v);
                        }
                    }
                    unstretch(&mut p);
                    if group && !moved.is_empty() {
                        p.object = first_new_object;
                        p.name = format!("{} ({})", p.name, p.object + 1);
                        moved[0].push((p, paint));
                        continue;
                    }
                    p.object = first_new_object + moved.len() as u32;
                    p.name = format!("{} ({})", p.name, p.object + 1);
                    moved.push(vec![(p, paint)]);
                }
            }
        }
        if let RegionAction::Extract { lay_flat: true, .. } = action {
            self.lay_beside(&kept, &mut moved);
        }
        for (p, paint) in moved.into_iter().flatten() {
            kept.push(p);
            kept_paint.push(paint);
        }
        self.clear_selection();
        let kind = if matches!(action, RegionAction::Remove) { "regionRemove" } else { "regionExtract" };
        self.commit_parts(kept, kept_paint, &transfers, kind);
        Ok(self.object_count())
    }

    // ---- selecting by colour ------------------------------------------------

    /// Selects (or with `add` false, deselects) the triangles the same colour
    /// as preview triangle `seed`, as `how` reads colour: only those joined to
    /// it with `contiguous`, otherwise across the whole model. False when
    /// `seed` is no triangle.
    pub fn select_by_colour(&mut self, seed: u32, how: ColourMatch, contiguous: bool, add: bool) -> bool {
        let pi = self.tri_offsets.partition_point(|&o| o <= seed).saturating_sub(1);
        let Some(ti) = seed.checked_sub(self.tri_offsets.get(pi).copied().unwrap_or(u32::MAX)).map(|t| t as usize)
        else {
            return false;
        };
        if self.work.parts.get(pi).is_none_or(|p| ti >= p.tris.len()) {
            return false;
        }
        // One key per triangle, compared with the seed's.
        let field = ColorField::new(&self.scene, &self.shape);
        let sources: Vec<Vec<Option<u32>>> = match how {
            ColourMatch::Material => (0..self.work.parts.len()).map(|p| self.work_sources(p)).collect(),
            _ => vec![],
        };
        let material = |p: usize, t: usize| {
            let part = &self.scene.parts[p];
            sources[p][t].and_then(|s| part.tri_source.get(s as usize)).and_then(|&m| part.sources.get(m as usize))
        };
        let lab = |p: usize, t: usize| field.sample(&self.shape, p, t, [1.0 / 3.0; 3]).lab();
        let seed_material = (how == ColourMatch::Material).then(|| material(pi, ti)).flatten();
        let seed_slot = self.trees[pi][ti].dominant();
        let seed_lab = lab(pi, ti);
        let same = |p: usize, t: usize| match how {
            ColourMatch::Material => seed_material.is_some() && material(p, t) == seed_material,
            ColourMatch::Filament => self.trees[p][t].dominant() == seed_slot,
            ColourMatch::Similar { delta_e } => color::dist2(lab(p, t), seed_lab) <= delta_e * delta_e,
        };
        let mut hit: Vec<Vec<bool>> = self.work.parts.iter().map(|x| vec![false; x.tris.len()]).collect();
        if contiguous {
            hit[pi][ti] = true;
            let mut stack = vec![ti];
            while let Some(t) = stack.pop() {
                for &(o, _) in &self.adjacency[pi][t] {
                    if !hit[pi][o as usize] && same(pi, o as usize) {
                        hit[pi][o as usize] = true;
                        stack.push(o as usize);
                    }
                }
            }
        } else {
            for (p, marks) in hit.iter_mut().enumerate() {
                for (t, m) in marks.iter_mut().enumerate() {
                    *m = same(p, t);
                }
            }
        }
        for (marks, found) in self.selection.iter_mut().zip(hit) {
            for (m, f) in marks.iter_mut().zip(found) {
                if f {
                    *m = add;
                }
            }
        }
        true
    }

    /// Triangles a colour border runs across, as the painting splits them.
    pub fn split_count(&self) -> usize {
        self.trees.iter().flatten().filter(|n| n.is_split()).count()
    }

    /// Makes every colour border that runs across a triangle an edge of the
    /// mesh: each such triangle becomes its painted pieces, and those beside
    /// them are split to match, so the surface stays closed. Selecting by
    /// filament then follows the borders as painted, not triangle by
    /// triangle. One undoable step; returns how many triangles were split.
    pub fn refine_to_paint(&mut self) -> Result<usize, RefineFailure> {
        let mut trees: Vec<Vec<Option<&Node>>> = vec![];
        for (pi, shape) in self.shape.parts.iter().enumerate() {
            let Some(origin) = &shape.origin else { return Err(RefineFailure::Reshaped) };
            if origin.iter().any(|(_, b)| *b != geom::ROOT) {
                return Err(RefineFailure::Reshaped);
            }
            let mut by_source = vec![None; self.scene.parts[pi].tris.len()];
            for ((src, _), node) in origin.iter().zip(&self.trees[pi]) {
                if let Some(slot) = by_source.get_mut(*src as usize) {
                    *slot = Some(node);
                }
            }
            trees.push(by_source);
        }
        if self.split_count() == 0 {
            return Err(RefineFailure::Nothing);
        }
        let transfers: Vec<Option<PaintTransfer>> = self
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
        let mut split = 0;
        let (mut parts, mut paint) = (vec![], vec![]);
        for (pi, (part, trees)) in self.scene.parts.iter().zip(&trees).enumerate() {
            let r = refine::refine(part, trees);
            split += r.split;
            paint.push(transfers[pi].as_ref().map(|_| CutPaint {
                part: pi,
                origins: r.origins.iter().map(|&o| Some(o)).collect(),
                corners: (0..r.part.tris.len()).map(|t| r.part.corners(t)).collect(),
            }));
            let mut p = r.part;
            for v in &mut p.positions {
                *v = std::array::from_fn(|i| v[i] / stretch[i]);
            }
            parts.push(p);
        }
        self.commit_parts(parts, paint, &transfers, "refine");
        Ok(split)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut;
    use crate::scene::{ColorSource, Scene};

    /// A 20 × 20 × 10 box of 4 × 4 quads a face, grey, standing on the plate.
    fn slab() -> Project {
        let mut part = crate::region::tests::grid_box([0.0; 3], [20.0, 20.0, 10.0], 4);
        part.sources = vec![ColorSource::Flat([0.8, 0.8, 0.8]), ColorSource::Flat([0.1, 0.2, 0.9])];
        Project::new(Scene { parts: vec![part], ..Default::default() }, 1)
    }

    /// Selects the top face's middle with a brush stroke from above.
    fn brush_top(p: &mut Project) -> Vec<u32> {
        let (_, changed) = p.select([0.3, 0.7, 50.0], [0.0, 0.0, -1.0], 6.0, true, false).unwrap();
        changed
    }

    #[test]
    fn the_brush_selects_without_painting_and_sees_only_the_surface_in_front() {
        let mut p = slab();
        let before = p.preview().labels;
        let changed = brush_top(&mut p);
        assert!(!changed.is_empty());
        assert_eq!(p.preview().labels, before, "selecting never repaints");
        assert!(!p.can_undo(), "nor is it a step to undo");
        let tops = p.selection().len();
        // From above, the visible brush stays on the top face; through, it reaches the bottom too.
        let top_z =
            |p: &Project| p.selection().iter().all(|&id| p.picker.1.corners[id as usize].iter().all(|v| v[2] > 9.9));
        assert!(top_z(&p));
        p.clear_selection();
        p.select([0.3, 0.7, 50.0], [0.0, 0.0, -1.0], 11.0, true, true).unwrap();
        assert!(!top_z(&p) && p.selection().len() > tops);
        // Erasing takes them out again.
        p.select([0.3, 0.7, 50.0], [0.0, 0.0, -1.0], 30.0, false, true).unwrap();
        assert!(p.selection().is_empty());
    }

    #[test]
    fn grow_smooth_connected_and_invert_reshape_the_selection() {
        let mut p = slab();
        brush_top(&mut p);
        let (n, regions) = p.selection_regions();
        assert_eq!(regions, 1);
        p.grow_selection();
        assert!(p.selection_regions().0 > n);
        p.select_connected();
        assert_eq!(p.selection().len(), p.work.triangle_count());
        p.invert_selection();
        assert!(p.selection().is_empty());
        // A lone triangle is a speck the smoothing takes away.
        p.set_selection(&[0]);
        p.smooth_selection();
        assert!(p.selection().is_empty());
    }

    #[test]
    fn extracting_a_shell_leaves_a_pocket_and_undoes_in_one_step() {
        let mut p = slab();
        brush_top(&mut p);
        let shell = Closure::Shell { thickness: 2.0, clearance: 0.0 };
        let preview = p.preview_region(shell, RegionAction::Extract { lay_flat: false, group: false }).unwrap();
        assert_eq!(preview.pieces.len(), 1);
        assert!(preview.closed && !preview.closure.is_empty());
        let total = 20.0 * 20.0 * 10.0;
        assert!((preview.rest_mm3 + preview.piece_mm3[0] - total).abs() < 0.05, "{preview:?}");
        // The preview changes nothing.
        assert_eq!(p.object_count(), 1);
        let before = p.preview();

        assert_eq!(p.apply_region(shell, RegionAction::Extract { lay_flat: false, group: false }), Ok(2));
        assert_eq!(p.undo_label(), Some("regionExtract"));
        assert!(p.selection().is_empty());
        for part in &p.scene.parts {
            assert!(cut::valid_solid(part, 1e-5), "{}", part.name);
        }
        // In place, the shell fills its pocket: the model is as tall as before.
        assert!((p.stats().size_mm[2] - 10.0).abs() < 1e-3);
        assert!(p.undo());
        assert_eq!(p.object_count(), 1);
        assert_eq!(p.preview().positions, before.positions);
    }

    #[test]
    fn removing_with_a_cap_and_laying_pieces_flat() {
        let mut p = slab();
        brush_top(&mut p);
        let cap = Closure::Cap { depth: 3.0 };
        assert_eq!(p.apply_region(cap, RegionAction::Remove), Ok(1));
        assert!(p.scene.parts.iter().all(|x| cut::valid_solid(x, 1e-5)));
        assert_eq!(p.undo_label(), Some("regionRemove"));
        assert!(p.undo());

        brush_top(&mut p);
        assert_eq!(
            p.apply_region(
                Closure::Shell { thickness: 2.0, clearance: 0.0 },
                RegionAction::Extract { lay_flat: true, group: false }
            ),
            Ok(2)
        );
        let parts = p.export_parts();
        let span = |o: u32, axis: usize| {
            let vs = parts.iter().filter(|x| x.object == o).flat_map(|x| x.positions.iter().map(move |v| v[axis]));
            vs.fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        };
        // The shell lies on the plate beside the slab, 2 mm thick, its outer face up.
        assert!(span(1, 2).0.abs() < 1e-3 && (span(1, 2).1 - 2.0).abs() < 1e-3, "{:?}", span(1, 2));
        assert!((span(1, 0).0 - span(0, 0).1 - 5.0).abs() < 1e-3);
    }

    #[test]
    fn empty_flat_or_total_selections_are_refused_with_the_reason() {
        let mut p = slab();
        let shell = Closure::Shell { thickness: 1.0, clearance: 0.0 };
        assert_eq!(p.preview_region(shell, RegionAction::Remove), Err(RegionFailure::Region(RegionError::Empty)));
        brush_top(&mut p);
        assert_eq!(p.preview_region(Closure::Cap { depth: 0.0 }, RegionAction::Remove), Err(RegionFailure::Flat));
        assert!(matches!(
            p.preview_region(Closure::Shell { thickness: 15.0, clearance: 0.0 }, RegionAction::Remove),
            Err(RegionFailure::Region(RegionError::ThinWall { .. }))
        ));
        p.select_connected();
        let cap = Closure::Cap { depth: 1.0 };
        assert_eq!(p.preview_region(cap, RegionAction::Remove), Err(RegionFailure::Everything));
        // The whole slab can still come off as an object of its own, with nothing to close.
        let all = p.preview_region(cap, RegionAction::Extract { lay_flat: false, group: false }).unwrap();
        assert!(all.closure.is_empty() && all.rest_mm3 == 0.0);
        // A shell of all of it is its outer skin: a core stays.
        let skin = p.preview_region(shell, RegionAction::Remove).unwrap();
        assert!(skin.closed && skin.rest_mm3 > 0.0 && skin.rest_mm3 < 4000.0);
        assert!(!p.can_undo_cut());
    }

    #[test]
    fn hand_painting_survives_on_both_sides_of_the_cut() {
        let mut p = slab();
        p.set_palette(vec![[0.8, 0.8, 0.8], [0.1, 0.2, 0.9]]);
        // A blue mark on the top face, inside the region, and one on a side, outside it.
        p.paint([0.3, 0.7, 50.0], [0.0, 0.0, -1.0], 2.0, 1).unwrap();
        p.paint([-50.0, 0.4, 5.6], [1.0, 0.0, 0.0], 2.0, 1).unwrap();
        p.end_stroke();
        let painted = p.painted_count();
        brush_top(&mut p);
        p.apply_region(
            Closure::Shell { thickness: 2.0, clearance: 0.0 },
            RegionAction::Extract { lay_flat: false, group: false },
        )
        .unwrap();
        // Both marks are still painted, and the shell's inner face under the top mark took it too.
        assert!(p.painted_count() > painted, "{} vs {painted}", p.painted_count());
    }

    /// Two closed towers side by side (red below, blue above), and the same scene as one project.
    fn twins() -> Project {
        let mut a = crate::region::tests::grid_box([0.0; 3], [10.0, 10.0, 20.0], 4);
        let b = crate::region::tests::grid_box([20.0, 0.0, 0.0], [30.0, 10.0, 20.0], 4);
        let (base, loops) = (a.positions.len() as u32, a.tri_loops.len() as u32 * 3);
        a.positions.extend(&b.positions);
        a.tris.extend(b.tris.iter().map(|t| t.map(|v| v + base)));
        a.tri_loops.extend(b.tri_loops.iter().map(|l| l.map(|v| v + loops)));
        a.tri_source.extend(&b.tri_source);
        a.uv_layers[0].extend(&b.uv_layers[0]);
        a.tri_source = (0..a.tris.len()).map(|t| u16::from(a.corners(t).iter().all(|v| v[2] >= 10.0))).collect();
        a.sources = vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])];
        Project::new(Scene { parts: vec![a], ..Default::default() }, 2)
    }

    fn seed_low(p: &Project) -> u32 {
        (0..p.work.parts[0].tris.len()).find(|&t| p.work.parts[0].corners(t).iter().all(|v| v[2] < 5.0)).unwrap() as u32
    }

    #[test]
    fn selecting_by_colour_takes_every_face_alike_or_only_those_joined() {
        let mut p = twins();
        let seed = seed_low(&p);
        let reds = p.work.parts[0].tris.len() - p.scene.parts[0].tri_source.iter().filter(|&&s| s == 1).count();
        for how in [ColourMatch::Material, ColourMatch::Filament, ColourMatch::Similar { delta_e: 5.0 }] {
            p.clear_selection();
            assert!(p.select_by_colour(seed, how, false, true));
            assert_eq!(p.selection().len(), reds, "{how:?}");
            assert_eq!(p.selection_regions().1, 2, "two towers, two regions: {how:?}");
            p.clear_selection();
            p.select_by_colour(seed, how, true, true);
            assert_eq!(p.selection_regions().1, 1, "{how:?}");
        }
        // Wide enough, red and blue are alike; the painting never changed.
        let labels = p.preview().labels;
        p.clear_selection();
        p.select_by_colour(seed, ColourMatch::Similar { delta_e: 300.0 }, false, true);
        assert_eq!(p.selection().len(), p.work.triangle_count());
        assert_eq!(p.preview().labels, labels);
        assert!(!p.select_by_colour(u32::MAX, ColourMatch::Filament, false, true));
    }

    #[test]
    fn red_bases_come_off_as_two_pieces_or_one_object_with_a_clearance() {
        let mut p = twins();
        let seed = seed_low(&p);
        p.select_by_colour(seed, ColourMatch::Filament, false, true);
        // Only the sides and bottoms of the red halves: a shell 1 thick, 0.2 short of its pocket.
        let shell = Closure::Shell { thickness: 1.0, clearance: 0.2 };
        let separate = RegionAction::Extract { lay_flat: false, group: false };
        let preview = p.preview_region(shell, separate).unwrap();
        assert_eq!(preview.pieces.len(), 2);
        assert!(preview.closed && preview.small == 0);
        assert_eq!(p.apply_region(shell, separate), Ok(3));
        for part in &p.scene.parts {
            assert!(cut::valid_solid(part, 1e-5), "{}", part.name);
        }
        assert!(p.undo());
        p.select_by_colour(seed, ColourMatch::Filament, false, true);
        assert_eq!(p.apply_region(shell, RegionAction::Extract { lay_flat: true, group: true }), Ok(2));
    }

    #[test]
    fn colour_borders_across_triangles_become_edges() {
        // A box textured red on its left half and blue on its right: the border runs across faces.
        let mut part = crate::region::tests::grid_box([0.0; 3], [10.0; 3], 1);
        let uv: Vec<[f32; 2]> = part
            .tri_loops
            .iter()
            .enumerate()
            .flat_map(|(t, _)| part.corners(t).map(|v| [v[0] / 10.0, v[1] / 10.0]))
            .collect();
        part.uv_layers = vec![uv];
        part.sources = vec![ColorSource::Image { image: 0, uv: 0 }];
        let image =
            crate::scene::Image { name: "half".into(), width: 2, height: 1, rgb: vec![230, 20, 20, 20, 20, 230] };
        let mut p = Project::new(Scene { parts: vec![part], images: vec![image], ..Default::default() }, 2);
        let before = (p.work.triangle_count(), p.split_count());
        assert!(before.1 > 0);
        let split = p.refine_to_paint().unwrap();
        assert_eq!(split, before.1);
        assert_eq!(p.undo_label(), Some("refine"));
        assert!(p.work.triangle_count() > before.0);
        assert!(p.split_count() < before.1, "{} vs {}", p.split_count(), before.1);
        assert!(cut::valid_solid(&p.scene.parts[0], 1e-5));
        assert!(p.undo());
        assert_eq!((p.work.triangle_count(), p.split_count()), before);
        // A flattened base reshapes the triangles: refining would not follow the painting.
        p.set_mesh_ops(crate::mesh::MeshOps { flatten_mm: 1.0, ..p.mesh_ops() });
        assert_eq!(p.refine_to_paint(), Err(RefineFailure::Reshaped));
    }
}
