//! Room on the plate: the printable area, the zones the printer excludes and
//! the space a prime tower will take. The model sits centred, where the 3MF
//! puts it; the tower stays where the 3MF already tells Bambu Studio to put it.
//! Nothing here changes the export: the reservation only answers whether the
//! model fits, and how large it can be, once that space is set aside.

use crate::printer::Printer;

/// An axis-aligned rectangle in plate millimetres (x0 ≤ x1, y0 ≤ y1).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }

    pub fn centred(cx: f32, cy: f32, w: f32, d: f32) -> Self {
        Self::new(cx - w / 2.0, cy - d / 2.0, cx + w / 2.0, cy + d / 2.0)
    }

    pub fn grow(&self, by: f32) -> Self {
        Self::new(self.x0 - by, self.y0 - by, self.x1 + by, self.y1 + by)
    }

    /// Overlap with some area, not just touching.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 - 1e-4 && o.x0 < self.x1 - 1e-4 && self.y0 < o.y1 - 1e-4 && o.y0 < self.y1 - 1e-4
    }

    pub fn contains(&self, o: &Rect) -> bool {
        o.x0 >= self.x0 - 1e-4 && o.y0 >= self.y0 - 1e-4 && o.x1 <= self.x1 + 1e-4 && o.y1 <= self.y1 + 1e-4
    }
}

/// How much room is set aside around the model and the tower, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reserve {
    /// Keep the tower's space free. Off, the whole plate is the model's.
    pub tower: bool,
    /// Tower footprint, brim included: width (X) and estimated depth (Y).
    pub tower_mm: [f32; 2],
    /// Gap between the model and the tower.
    pub tower_gap_mm: f32,
    /// Gap between the model and the edges of the printable area.
    pub margin_mm: f32,
}

impl Default for Reserve {
    fn default() -> Self {
        Self { tower: true, tower_mm: [26.0, 26.0], tower_gap_mm: 5.0, margin_mm: 5.0 }
    }
}

/// Where everything lands and what is wrong with it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlateCheck {
    pub area: Rect,
    pub excluded: Vec<Rect>,
    pub model: Rect,
    /// The tower's estimated footprint, when there is one to reserve.
    pub tower: Option<Rect>,
    /// The model's edge inside the margin; outside it the model is too close to the edge.
    pub usable: Rect,
    /// The model leaves the printable area.
    pub off_plate: bool,
    /// The model is closer to the edge than the margin.
    pub in_margin: bool,
    /// The model covers a zone the printer excludes.
    pub on_excluded: bool,
    /// The model takes space the tower needs, gap included.
    pub on_tower: bool,
    /// The tower does not fit where the 3MF places it.
    pub tower_off_plate: bool,
    /// Largest uniform scale of the model's footprint that avoids all of the above.
    pub fit_factor: f32,
}

impl PlateCheck {
    pub fn fits(&self) -> bool {
        !(self.off_plate || self.in_margin || self.on_excluded || self.on_tower)
    }
}

/// Lays out a model `footprint_mm` wide and deep, centred on the plate.
pub fn check(printer: &Printer, footprint_mm: [f32; 2], reserve: &Reserve) -> PlateCheck {
    let area = printer.printable_area();
    let excluded = printer.excluded_areas();
    let (cx, cy) = printer.plate_centre();
    let [w, d] = footprint_mm.map(|v| v.max(1e-3));
    let model = Rect::centred(cx, cy, w, d);
    let tower = reserve.tower.then(|| {
        let (x, y) = printer.tower_origin();
        Rect::new(x, y, x + reserve.tower_mm[0], y + reserve.tower_mm[1])
    });
    let usable = area.grow(-reserve.margin_mm.max(0.0));
    let keepout = tower.map(|t| t.grow(reserve.tower_gap_mm.max(0.0)));

    // The model grows from the plate centre: each obstacle caps how far.
    let mut k = (2.0 * (cx - usable.x0) / w)
        .min(2.0 * (usable.x1 - cx) / w)
        .min(2.0 * (cy - usable.y0) / d)
        .min(2.0 * (usable.y1 - cy) / d)
        .max(0.0);
    for o in excluded.iter().chain(&keepout) {
        let dx = (o.x0 - cx).max(cx - o.x1);
        let dy = (o.y0 - cy).max(cy - o.y1);
        // Clear of the obstacle along X or along Y is enough.
        let clear = (if dx >= 0.0 { 2.0 * dx / w } else { 0.0 }).max(if dy >= 0.0 { 2.0 * dy / d } else { 0.0 });
        k = k.min(clear);
    }

    PlateCheck {
        off_plate: !area.contains(&model),
        in_margin: !usable.contains(&model),
        on_excluded: excluded.iter().any(|e| e.overlaps(&model)),
        on_tower: keepout.is_some_and(|t| t.overlaps(&model)),
        tower_off_plate: tower.is_some_and(|t| !area.contains(&t)),
        fit_factor: k,
        area,
        excluded,
        model,
        tower,
        usable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::catalog;

    fn printer(id: &str) -> Printer {
        Printer::new(catalog().into_iter().find(|p| p.id == id).unwrap(), 0.4)
    }

    #[test]
    fn a_model_that_fits_alone_can_still_hit_the_tower() {
        let a1 = printer("Bambu Lab A1");
        let alone = check(&a1, [230.0, 230.0], &Reserve { tower: false, margin_mm: 0.0, ..Default::default() });
        assert!(alone.fits(), "{alone:?}");
        let with_tower = check(&a1, [230.0, 230.0], &Reserve { margin_mm: 0.0, ..Default::default() });
        assert!(with_tower.on_tower && !with_tower.fits());

        // Scaling by the fit factor clears the tower and its gap.
        let k = with_tower.fit_factor;
        assert!(k < 1.0 && k > 0.3, "{k}");
        let fitted = check(&a1, [230.0 * k, 230.0 * k], &Reserve { margin_mm: 0.0, ..Default::default() });
        assert!(fitted.fits(), "{fitted:?}");
        let more = check(&a1, [230.0 * k * 1.02, 230.0 * k * 1.02], &Reserve { margin_mm: 0.0, ..Default::default() });
        assert!(!more.fits());
    }

    #[test]
    fn a_bigger_tower_or_gap_leaves_less_room() {
        let a1 = printer("Bambu Lab A1");
        let fit = |r: Reserve| check(&a1, [100.0, 100.0], &r).fit_factor;
        let base = fit(Reserve::default());
        assert!(fit(Reserve { tower_mm: [26.0, 80.0], ..Default::default() }) <= base);
        assert!(fit(Reserve { tower_gap_mm: 20.0, ..Default::default() }) < base);
        assert!(fit(Reserve { tower: false, ..Default::default() }) > base);
        assert!(
            fit(Reserve { margin_mm: 20.0, tower: false, ..Default::default() })
                < fit(Reserve { tower: false, ..Default::default() })
        );
    }

    #[test]
    fn margins_and_off_plate() {
        let a1 = printer("Bambu Lab A1");
        let big = check(&a1, [300.0, 100.0], &Reserve::default());
        assert!(big.off_plate && big.in_margin);
        let edge = check(&a1, [250.0, 100.0], &Reserve { tower: false, ..Default::default() });
        assert!(!edge.off_plate && edge.in_margin);
    }

    #[test]
    fn excluded_zones_count() {
        let x1c = printer("Bambu Lab X1 Carbon");
        let c = check(&x1c, [250.0, 250.0], &Reserve { tower: false, margin_mm: 0.0, ..Default::default() });
        assert!(!c.excluded.is_empty(), "the X1 Carbon keeps a corner free");
        assert!(c.on_excluded);
        assert!(check(&x1c, [100.0, 100.0], &Reserve { tower: false, ..Default::default() }).fits());
    }
}
