use super::{CutPaint, PaintTransfer, Project, UndoEntry, NO_LABEL};
use crate::fit::{self, Room};
use crate::geom::{self, Vec3};
use crate::scene::Part;
use std::collections::BTreeMap;

type Layout = Vec<(Part, usize)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateEditError {
    Plate,
    Object,
    ObjectLimit,
    NoRoom,
    DoesNotFit,
}

impl Project {
    /// Adds a plate even if it has no objects yet.
    pub fn add_plate(&mut self) {
        self.end_stroke();
        self.undo.push(UndoEntry::AddPlate(self.minimum_plates));
        self.minimum_plates = self.plate_count() + 1;
    }

    /// Moves every component of the object together, beside the destination's contents.
    pub fn move_to_plate(&mut self, object: u32, plate: u32, bed_width_mm: f32) -> Result<(), PlateEditError> {
        self.check_plate(plate)?;
        let part = self.scene.parts.iter().find(|p| p.object == object).ok_or(PlateEditError::Object)?;
        if part.plate == plate {
            return Ok(());
        }
        let mut parts = self.layout_parts();
        self.place_beside(&mut parts, object, plate, bed_width_mm)?;
        self.commit_layout(parts, "movePlate");
        Ok(())
    }

    /// Deletes every object on the plate, keeping the plate itself.
    pub fn clear_plate(&mut self, plate: u32) -> Result<(), PlateEditError> {
        self.check_plate(plate)?;
        if self.scene.parts.iter().any(|p| p.plate == plate) {
            let mut parts = self.layout_parts();
            parts.retain(|(part, _)| part.plate != plate);
            self.commit_layout(parts, "clearPlate");
        }
        Ok(())
    }

    /// Copies a whole object, including its components and brush painting.
    pub fn clone_object(&mut self, object: u32, label: &str, bed_width_mm: f32) -> Result<(), PlateEditError> {
        let (mut parts, copy, plate) = self.cloned_layout(object, label)?;
        self.place_beside(&mut parts, copy, plate, bed_width_mm)?;
        self.commit_layout(parts, "cloneObject");
        Ok(())
    }

    fn cloned_layout(&self, object: u32, label: &str) -> Result<(Layout, u32, u32), PlateEditError> {
        let mut parts = self.layout_parts();
        let plate = parts.iter().find(|(p, _)| p.object == object).ok_or(PlateEditError::Object)?.0.plate;
        let id = parts
            .iter()
            .map(|(p, _)| p.object)
            .max()
            .and_then(|id| id.checked_add(1))
            .ok_or(PlateEditError::ObjectLimit)?;
        let copies: Layout = parts
            .iter()
            .filter(|(p, _)| p.object == object)
            .map(|(p, i)| {
                let name = format!("{} · {label} {}", p.name, u64::from(id) + 1);
                (Part { object: id, name, ..p.clone() }, *i)
            })
            .collect();
        parts.extend(copies);
        Ok((parts, id, plate))
    }

    /// Packs whole objects across the printer's plates without changing their scale.
    pub fn arrange_plates(&mut self, room: &Room, bed_width_mm: f32) -> Result<(), PlateEditError> {
        let mut parts = self.layout_parts();
        if !parts.is_empty() {
            self.pack_layout(&mut parts, room, bed_width_mm)?;
            self.commit_layout(parts, "arrangePlates");
        }
        Ok(())
    }

    /// Places a copy and reflows the plates as one undoable edit.
    pub fn clone_and_arrange(
        &mut self,
        object: u32,
        label: &str,
        room: &Room,
        bed_width_mm: f32,
    ) -> Result<(), PlateEditError> {
        let (mut parts, _, _) = self.cloned_layout(object, label)?;
        self.pack_layout(&mut parts, room, bed_width_mm)?;
        self.commit_layout(parts, "cloneObject");
        Ok(())
    }

    fn pack_layout(&self, parts: &mut [(Part, usize)], room: &Room, bed_width_mm: f32) -> Result<(), PlateEditError> {
        if !room.valid() {
            return Err(PlateEditError::NoRoom);
        }
        let unit = self.stretch.map(|axis| axis * self.scale);
        let mut boxes = BTreeMap::<u32, (Vec3, Vec3)>::new();
        for (part, _) in parts.iter() {
            for vertex in &part.positions {
                let p = std::array::from_fn(|i| vertex[i] * unit[i]);
                let (lo, hi) = boxes.entry(part.object).or_insert((p, p));
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
        let footprints: Vec<_> = boxes.values().map(|&(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]]).collect();
        let spots = fit::pack(&footprints, room);
        let placements = boxes
            .into_iter()
            .zip(spots)
            .map(|((id, (lo, hi)), spot)| {
                let size = geom::sub(hi, lo);
                let spot = spot.filter(|_| size[2] <= room.height_mm + 1e-3).ok_or(PlateEditError::DoesNotFit)?;
                Ok((id, (lo, size, spot)))
            })
            .collect::<Result<BTreeMap<_, _>, PlateEditError>>()?;
        for (part, _) in parts.iter_mut() {
            let &(origin, size, spot) = placements.get(&part.object).ok_or(PlateEditError::Object)?;
            part.plate = spot.plate as u32;
            for p in &mut part.positions {
                let [x, y, z]: Vec3 = std::array::from_fn(|i| p[i] * unit[i] - origin[i]);
                let [x, y] = if spot.turned { [size[1] - y, x] } else { [x, y] };
                *p = [(x + spot.at[0]) / unit[0], (y + spot.at[1]) / unit[1], z / unit[2]];
            }
        }
        self.space_plates(parts, bed_width_mm);
        Ok(())
    }

    fn check_plate(&self, plate: u32) -> Result<(), PlateEditError> {
        if (plate as usize) < self.plate_count() {
            Ok(())
        } else {
            Err(PlateEditError::Plate)
        }
    }

    fn layout_parts(&self) -> Layout {
        self.scene
            .parts
            .iter()
            .zip(&self.source_positions)
            .enumerate()
            .map(|(i, (part, positions))| (Part { positions: positions.clone(), ..part.clone() }, i))
            .collect()
    }

    fn place_beside(
        &self,
        parts: &mut [(Part, usize)],
        object: u32,
        plate: u32,
        bed_width_mm: f32,
    ) -> Result<(), PlateEditError> {
        let (lo, hi) =
            bounds(parts.iter().filter(|(p, _)| p.object == object).map(|(p, _)| p)).ok_or(PlateEditError::Object)?;
        let target = bounds(parts.iter().filter(|(p, _)| p.plate == plate && p.object != object).map(|(p, _)| p));
        let spacing = 5.0 / (self.scale * self.stretch[0]);
        let shift = match target {
            Some((a, b)) => [b[0] - lo[0] + spacing, (a[1] + b[1] - lo[1] - hi[1]) / 2.0, 0.0],
            None => [-lo[0], -(lo[1] + hi[1]) / 2.0, 0.0],
        };
        for (part, _) in parts.iter_mut().filter(|(p, _)| p.object == object) {
            part.plate = plate;
            translate(part, shift);
        }
        self.space_plates(parts, bed_width_mm);
        Ok(())
    }

    /// Keep occupied plates apart in the viewer, including plates with oversized objects.
    fn space_plates(&self, parts: &mut [(Part, usize)], bed_width_mm: f32) {
        let mut columns = BTreeMap::<u32, (f32, f32)>::new();
        for (part, _) in parts.iter() {
            for v in &part.positions {
                let x = columns.entry(part.plate).or_insert((v[0], v[0]));
                x.0 = x.0.min(v[0]);
                x.1 = x.1.max(v[0]);
            }
        }
        let bed = bed_width_mm / (self.scale * self.stretch[0]);
        let mut left = 0.0;
        for (lo, hi) in columns.values_mut() {
            let span = (*hi - *lo).max(bed);
            *lo = left + (span - (*hi - *lo)) / 2.0 - *lo;
            left += span + bed * 0.2;
        }
        for (part, _) in parts {
            if let Some(&(x, _)) = columns.get(&part.plate) {
                translate(part, [x, 0.0, 0.0]);
            }
        }
    }

    fn commit_layout(&mut self, changes: Layout, kind: &'static str) {
        let plate_count = self.plate_count();
        let transfers: Vec<_> = self
            .scene
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                self.painted[i]
                    .iter()
                    .any(|&label| label != NO_LABEL)
                    .then(|| PaintTransfer::new(&self.shape.parts[i], &self.painted[i], part.tris.len()))
            })
            .collect();
        let (parts, paint) = changes
            .into_iter()
            .map(|(part, i)| {
                let source = &self.scene.parts[i];
                let paint = transfers[i].as_ref().map(|_| CutPaint {
                    part: i,
                    origins: (0..source.tris.len()).map(|t| Some(t as u32)).collect(),
                    corners: (0..source.tris.len()).map(|t| source.corners(t)).collect(),
                });
                (part, paint)
            })
            .unzip();
        self.commit_parts(parts, paint, &transfers, kind);
        self.minimum_plates = plate_count;
    }
}

fn translate(part: &mut Part, shift: Vec3) {
    for vertex in &mut part.positions {
        *vertex = geom::add(*vertex, shift);
    }
}

fn bounds<'a>(parts: impl Iterator<Item = &'a Part>) -> Option<(Vec3, Vec3)> {
    let mut points = parts.flat_map(|p| p.positions.iter().copied());
    let first = points.next()?;
    Some(points.fold((first, first), |(lo, hi), p| {
        (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
    }))
}
