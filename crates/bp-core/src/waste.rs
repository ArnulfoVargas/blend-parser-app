//! What a multicolour print throws away, from the swaps of [`crate::estimate`].
//!
//! Three contributions, each counted once:
//! - **Flush**: filament pushed out at each swap until the new colour runs
//!   clean, priced per direction (dark to light needs more) and multiplied as
//!   Bambu Studio's flush multiplier does. Part of it can go into the model's
//!   infill or supports instead of the bin.
//! - **Start**: the purge line every print begins with.
//! - **Prime tower**: its walls and sparse fill on every layer up to the last
//!   swap, plus the priming done on it at each swap. On printers that purge
//!   into the tower rather than a chute the flush ends up there too; it is
//!   still reported as flush, never twice.
//!
//! The slicer decides the exact order and volumes; this is an estimate to
//! compare settings and to reserve room on the plate.

use crate::color::{srgb_to_lab, Rgb};
use crate::estimate::{plan, SwapEstimate};
use crate::palette::delta_e;

/// Print settings the estimate depends on, in millimetres, mm³ and g/cm³.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WasteParams {
    pub layer_mm: f32,
    pub nozzle_mm: f32,
    pub filament_diameter_mm: f32,
    pub density_g_cm3: f32,
    /// Scales every flush volume, like Bambu Studio's flush multiplier.
    pub flush_multiplier: f32,
    /// Purge line at the start of the print.
    pub start_purge_mm3: f32,
    /// Primed on the tower at each swap.
    pub prime_mm3: f32,
    /// Share of the flush that infill or supports take instead (0 to 1); an assumption.
    pub flush_reuse: f32,
    /// Whether a prime tower is printed at all.
    pub tower: bool,
    /// Tower width without its brim.
    pub tower_width_mm: f32,
    /// Tower depth without its brim; `None` estimates it from the busiest layer.
    pub tower_depth_mm: Option<f32>,
    pub tower_brim_mm: f32,
    /// The printer purges into a chute; otherwise the flush goes into the tower.
    pub purge_chute: bool,
}

impl Default for WasteParams {
    fn default() -> Self {
        Self {
            layer_mm: 0.2,
            nozzle_mm: 0.4,
            filament_diameter_mm: 1.75,
            density_g_cm3: 1.24,
            flush_multiplier: 1.0,
            start_purge_mm3: 120.0,
            prime_mm3: 20.0,
            flush_reuse: 0.0,
            tower: true,
            tower_width_mm: 20.0,
            tower_depth_mm: None,
            tower_brim_mm: 3.0,
            purge_chute: true,
        }
    }
}

/// Smallest and largest flush Bambu Studio suggests for single-nozzle printers.
pub const MIN_FLUSH_MM3: f32 = 107.0;
pub const MAX_FLUSH_MM3: f32 = 800.0;

/// Flush for a swap from `from` to `to`: more for very different colours, and
/// more again when the new colour is lighter, which shows any trace of the old.
pub fn default_flush(from: Rgb, to: Rgb) -> f32 {
    if from == to {
        return 0.0;
    }
    let lighter = (srgb_to_lab(to)[0] - srgb_to_lab(from)[0]).max(0.0);
    let k = ((0.6 * delta_e(from, to) + 1.2 * lighter) / 150.0).clamp(0.0, 1.0);
    (MIN_FLUSH_MM3 + (MAX_FLUSH_MM3 - MIN_FLUSH_MM3) * k).round()
}

/// The volumes and what they weigh. Volumes in mm³.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Waste {
    /// Swaps in the order that flushes least; can differ from the fewest swaps.
    pub swaps: u32,
    /// Flushed at the swaps and thrown away.
    pub flush_mm3: f32,
    /// Flush that went into infill or supports: not waste, shown apart.
    pub reused_mm3: f32,
    pub start_mm3: f32,
    /// Tower walls, fill, brim and priming, without any flush.
    pub tower_mm3: f32,
    pub total_mm3: f32,
    pub total_g: f32,
    /// Length of filament the total takes, in metres.
    pub total_m: f32,
    /// Estimated tower footprint, brim included, and height.
    pub tower_footprint_mm: [f32; 2],
    pub tower_height_mm: f32,
}

/// `flush(from, to)` gives the volume of one swap before the multiplier. The
/// colours are ordered, layer by layer, to flush as little as possible.
pub fn waste(swaps: &SwapEstimate, params: &WasteParams, flush: impl Fn(u8, u8) -> f32) -> Waste {
    let p = params;
    let changes = plan(&swaps.layer_sets, |a, b| flush(a, b).max(0.0));
    let mut w = Waste { swaps: changes.len() as u32, start_mm3: p.start_purge_mm3.max(0.0), ..Default::default() };
    let reuse = p.flush_reuse.clamp(0.0, 1.0);
    let mut busiest_layer = 0.0f32;
    for layer in changes.chunk_by(|a, b| a.0 == b.0) {
        let mut into_tower = 0.0;
        for &(_, from, to) in layer {
            let v = flush(from, to).max(0.0) * p.flush_multiplier.max(0.0);
            w.reused_mm3 += v * reuse;
            w.flush_mm3 += v * (1.0 - reuse);
            into_tower += p.prime_mm3 + if p.purge_chute { 0.0 } else { v * (1.0 - reuse) };
        }
        busiest_layer = busiest_layer.max(into_tower);
    }
    let tower_layers = changes.last().map_or(0, |c| c.0 + 1);
    if p.tower && tower_layers > 0 {
        let width = p.tower_width_mm.max(1.0);
        let layer = p.layer_mm.max(0.01);
        // The busiest layer has to fit in one layer of the tower; it is never
        // shallower than it is wide.
        let depth = p.tower_depth_mm.unwrap_or((busiest_layer / (layer * width)).clamp(width, 250.0));
        let line = p.nozzle_mm * 1.125;
        let height = tower_layers as f32 * layer;
        let walls = 2.0 * (width + depth) * line * height;
        // Layers without a swap get a sparse fill, about a tenth of the area.
        let quiet = tower_layers.saturating_sub(changes.chunk_by(|a, b| a.0 == b.0).count() as u32) as f32;
        let sparse = quiet * width * depth * layer * 0.1;
        let brim = p.tower_brim_mm.max(0.0);
        let brim_vol = ((width + 2.0 * brim) * (depth + 2.0 * brim) - width * depth) * layer;
        let priming = changes.len() as f32 * p.prime_mm3;
        w.tower_mm3 = walls + sparse + brim_vol + priming;
        w.tower_footprint_mm = [width + 2.0 * brim, depth + 2.0 * brim];
        w.tower_height_mm = height;
    }
    w.total_mm3 = w.flush_mm3 + w.start_mm3 + w.tower_mm3;
    w.total_g = w.total_mm3 * p.density_g_cm3 / 1000.0;
    let section = std::f32::consts::PI * (p.filament_diameter_mm / 2.0).powi(2);
    w.total_m = if section > 0.0 { w.total_mm3 / section / 1000.0 } else { 0.0 };
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layers with these colours (slot bit masks).
    fn swaps(layer_sets: Vec<(u32, u32)>) -> SwapEstimate {
        SwapEstimate { layers: 50, layer_sets, ..Default::default() }
    }

    #[test]
    fn flush_depends_on_the_direction() {
        let (black, white) = ([0.0; 3], [1.0; 3]);
        let to_white = default_flush(black, white);
        let to_black = default_flush(white, black);
        assert!(to_white > to_black, "{to_white} {to_black}");
        assert_eq!(to_white, MAX_FLUSH_MM3);
        assert_eq!(default_flush(white, white), 0.0);
        assert!(default_flush([0.5, 0.2, 0.2], [0.52, 0.2, 0.2]) < 150.0);
    }

    #[test]
    fn contributions_add_up_without_counting_twice() {
        // Slot 0, then 0 and 1, then 0 alone, then both again: three swaps.
        let s = swaps(vec![(0, 0b01), (3, 0b11), (6, 0b01), (9, 0b11)]);
        let p = WasteParams::default();
        let w = waste(&s, &p, |_, _| 200.0);
        assert_eq!(w.swaps, 3);
        assert_eq!(w.flush_mm3, 600.0);
        assert_eq!(w.start_mm3, 120.0);
        assert!(w.tower_mm3 > 3.0 * p.prime_mm3);
        assert!((w.total_mm3 - (w.flush_mm3 + w.start_mm3 + w.tower_mm3)).abs() < 1e-3);
        assert!((w.total_g - w.total_mm3 * 1.24 / 1000.0).abs() < 1e-4);
        assert!((w.tower_height_mm - 2.0).abs() < 1e-4);

        // The multiplier scales the flush only; reuse moves part of it out of the waste.
        let doubled = waste(&s, &WasteParams { flush_multiplier: 2.0, ..p }, |_, _| 200.0);
        assert_eq!(doubled.flush_mm3, 1200.0);
        assert_eq!(doubled.tower_mm3, w.tower_mm3);
        let reused = waste(&s, &WasteParams { flush_reuse: 0.25, ..p }, |_, _| 200.0);
        assert_eq!((reused.flush_mm3, reused.reused_mm3), (450.0, 150.0));

        // Without a tower its share goes away and nothing else changes.
        let none = waste(&s, &WasteParams { tower: false, ..p }, |_, _| 200.0);
        assert_eq!(none.tower_mm3, 0.0);
        assert_eq!(none.tower_footprint_mm, [0.0, 0.0]);
        assert_eq!(none.flush_mm3, w.flush_mm3);
    }

    #[test]
    fn a_tower_without_a_chute_grows_deeper() {
        let s = swaps(vec![(0, 0b001), (3, 0b111)]);
        let chute = waste(&s, &WasteParams::default(), |_, _| 300.0);
        let tower = waste(&s, &WasteParams { purge_chute: false, ..Default::default() }, |_, _| 300.0);
        assert!(tower.tower_footprint_mm[1] > chute.tower_footprint_mm[1]);
        assert_eq!(tower.tower_footprint_mm[0], 26.0);
        // The flush stays flush; the tower only adds its own walls.
        assert_eq!(tower.flush_mm3, chute.flush_mm3);
    }

    #[test]
    fn a_set_tower_depth_wins_over_the_estimate() {
        let s = swaps(vec![(0, 0b01), (3, 0b11)]);
        let w = waste(&s, &WasteParams { tower_depth_mm: Some(60.0), ..Default::default() }, |_, _| 300.0);
        assert_eq!(w.tower_footprint_mm, [26.0, 66.0]);
    }

    #[test]
    fn colours_go_in_the_order_that_flushes_least() {
        // Two colours on every layer; going to slot 1 costs five times more.
        let s = swaps((0..4).map(|l| (l, 0b11)).collect());
        let w =
            waste(&s, &WasteParams { tower: false, ..Default::default() }, |_, b| if b == 1 { 500.0 } else { 100.0 });
        // Four swaps that alternate: start on 1 and the dear one happens only twice.
        assert_eq!(w.swaps, 4);
        assert_eq!(w.flush_mm3, 1200.0);
    }

    #[test]
    fn no_swaps_no_tower() {
        let w = waste(&swaps(vec![(0, 0b01), (1, 0b01)]), &WasteParams::default(), |_, _| 300.0);
        assert_eq!((w.flush_mm3, w.tower_mm3, w.total_mm3), (0.0, 0.0, 120.0));
    }
}
