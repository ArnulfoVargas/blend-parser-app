//! What a multicolour print throws away, from the swaps of [`crate::estimate`].
//!
//! Three contributions, each counted once:
//! - **Flush**: filament pushed out at each swap until the new colour runs
//!   clean, priced per direction (dark to light needs more) and multiplied as
//!   Bambu Studio's flush multiplier does. Part of it can go into the model's
//!   infill or supports instead of the bin.
//! - **Start**: the purge line every print begins with.
//! - **Prime tower**: Bambu Studio 2's square tower with ribs, its walls and
//!   sparse fill on every layer up to the last swap, plus the priming done on
//!   it at each swap. On printers that purge into the tower rather than a
//!   chute the flush ends up there too; it is still reported as flush, never
//!   twice.
//!
//! The flushes, the tower's size and how it is printed follow Bambu Studio
//! (`FlushVolCalc.cpp`, `Print.cpp`, `WipeTower.cpp`). The slicer decides the
//! exact order and volumes; this is an estimate to compare settings and to
//! reserve room on the plate.

use crate::color::{delta_e2000, srgb_to_lab, Rgb};
use crate::estimate::{plan, SwapEstimate};

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
    /// Primed on the tower at each swap (Bambu Studio's `filament_prime_volume`).
    pub prime_mm3: f32,
    /// Share of the flush that infill or supports take instead (0 to 1); an assumption.
    pub flush_reuse: f32,
    /// Whether a prime tower is printed at all.
    pub tower: bool,
    /// Side of the square tower without its brim; `None` sizes it as Bambu
    /// Studio does, from the busiest layer and the print's height.
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
            prime_mm3: 45.0,
            flush_reuse: 0.0,
            tower: true,
            tower_depth_mm: None,
            tower_brim_mm: 3.0,
            purge_chute: true,
        }
    }
}

/// The most Bambu Studio flushes at one swap.
pub const MAX_FLUSH_MM3: f32 = 900.0;

/// Bambu Studio's measured flushes on single-nozzle printers
/// (`resources/flush/flush_data_standard.txt`), above the nozzle's own volume:
/// the colours it was measured with, then (from, to, mm³).
const MEASURED_COLOURS: [u32; 12] = [
    0x000000, 0xC12E1F, 0x00AE42, 0x545454, 0xD1D3D5, 0x5B6579, 0xF4EE2A, 0x9D432C, 0x5E43B7, 0x0A2989, 0xFF6A13,
    0x8E9089,
];
const MEASURED: [(u32, u32, f32); 58] = [
    (0x000000, 0xF4EE2A, 450.0),
    (0x000000, 0x5E43B7, 330.0),
    (0xC12E1F, 0xF4EE2A, 420.0),
    (0xC12E1F, 0xFF6A13, 210.0),
    (0x00AE42, 0xD1D3D5, 330.0),
    (0x00AE42, 0xF4EE2A, 240.0),
    (0x00AE42, 0xFF6A13, 270.0),
    (0x545454, 0x00AE42, 180.0),
    (0x545454, 0xD1D3D5, 240.0),
    (0x545454, 0xF4EE2A, 270.0),
    (0x545454, 0x5E43B7, 120.0),
    (0x545454, 0xFF6A13, 300.0),
    (0x545454, 0x8E9089, 120.0),
    (0xD1D3D5, 0xF4EE2A, 120.0),
    (0xD1D3D5, 0xFF6A13, 150.0),
    (0x5B6579, 0xC12E1F, 120.0),
    (0x5B6579, 0x00AE42, 90.0),
    (0x5B6579, 0xD1D3D5, 120.0),
    (0x5B6579, 0xF4EE2A, 180.0),
    (0x5B6579, 0x9D432C, 120.0),
    (0x5B6579, 0x0A2989, 90.0),
    (0x5B6579, 0xFF6A13, 180.0),
    (0x5B6579, 0x8E9089, 90.0),
    (0xF4EE2A, 0x000000, 120.0),
    (0xF4EE2A, 0xC12E1F, 90.0),
    (0xF4EE2A, 0x00AE42, 150.0),
    (0xF4EE2A, 0x9D432C, 150.0),
    (0xF4EE2A, 0xFF6A13, 90.0),
    (0x9D432C, 0x00AE42, 240.0),
    (0x9D432C, 0xD1D3D5, 300.0),
    (0x9D432C, 0xF4EE2A, 270.0),
    (0x9D432C, 0xFF6A13, 180.0),
    (0x9D432C, 0x8E9089, 210.0),
    (0x5E43B7, 0x00AE42, 180.0),
    (0x5E43B7, 0xD1D3D5, 270.0),
    (0x5E43B7, 0xF4EE2A, 270.0),
    (0x5E43B7, 0x9D432C, 150.0),
    (0x5E43B7, 0xFF6A13, 270.0),
    (0x5E43B7, 0x8E9089, 210.0),
    (0x0A2989, 0xC12E1F, 330.0),
    (0x0A2989, 0x00AE42, 210.0),
    (0x0A2989, 0x545454, 150.0),
    (0x0A2989, 0xD1D3D5, 450.0),
    (0x0A2989, 0x5B6579, 240.0),
    (0x0A2989, 0x9D432C, 270.0),
    (0x0A2989, 0x5E43B7, 180.0),
    (0x0A2989, 0xFF6A13, 390.0),
    (0x0A2989, 0x8E9089, 270.0),
    (0xFF6A13, 0xC12E1F, 90.0),
    (0xFF6A13, 0xD1D3D5, 210.0),
    (0xFF6A13, 0xF4EE2A, 210.0),
    (0xFF6A13, 0x9D432C, 120.0),
    (0xFF6A13, 0x8E9089, 180.0),
    (0x8E9089, 0xC12E1F, 150.0),
    (0x8E9089, 0x00AE42, 120.0),
    (0x8E9089, 0xD1D3D5, 150.0),
    (0x8E9089, 0xF4EE2A, 270.0),
    (0x8E9089, 0xFF6A13, 150.0),
];

fn rgb_of(hex: u32) -> Rgb {
    [(hex >> 16) as f32 / 255.0, ((hex >> 8) & 0xFF) as f32 / 255.0, (hex & 0xFF) as f32 / 255.0]
}

/// The measured flush for a swap whose colours match measured ones (CIEDE2000
/// within 5, the first match in Bambu Studio's order), if it was measured.
fn measured_flush(from: Rgb, to: Rgb) -> Option<f32> {
    let like = |c: Rgb| {
        let lab = srgb_to_lab(c);
        MEASURED_COLOURS.iter().copied().find(|&m| delta_e2000(srgb_to_lab(rgb_of(m)), lab) <= 5.0)
    };
    let (a, b) = (like(from)?, like(to)?);
    MEASURED.iter().find(|m| m.0 == a && m.1 == b).map(|m| m.2)
}

/// Hue, saturation (0 to 1) and value, as Bambu Studio converts them.
fn hsv(c: Rgb) -> (f32, f32, f32) {
    let [r, g, b] = c;
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let h = if d.abs() < 0.001 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max.abs() < 0.001 { 0.0 } else { d / max }, max)
}

/// Bambu Studio's flush for a swap from `from` to `to` on a single nozzle
/// (`FlushVolCalculator`): the measured value where both colours were measured,
/// otherwise its formula, which weighs how far apart the hues are against how
/// much lighter the new colour is (a light colour after a dark one shows any
/// trace of it); then the nozzle's own volume, `min_mm3`, on top, capped at
/// [`MAX_FLUSH_MM3`]. Before the flush multiplier.
pub fn default_flush(from: Rgb, to: Rgb, min_mm3: f32) -> f32 {
    let q = |c: Rgb| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0);
    let (from, to) = (q(from), q(to));
    if from == to {
        return 0.0;
    }
    let colour = measured_flush(from, to).unwrap_or_else(|| {
        let ((h1, s1, v1), (h2, s2, v2)) = (hsv(from), hsv(to));
        let (h1, h2) = (h1.to_radians(), h2.to_radians());
        let (dx, dy) = (h1.cos() * s1 * v1 - h2.cos() * s2 * v2, h1.sin() * s1 * v1 - h2.sin() * s2 * v2);
        let mut hs = (dx * dx + dy * dy).sqrt().min(1.2);
        let luma = |c: Rgb| c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11;
        let (l1, l2) = (luma(from), luma(to));
        let lumi = if l2 >= l1 {
            (l2 - l1).powf(0.7) * 560.0
        } else {
            hs = hs.min(0.67 * v2 + 0.33 * v1);
            (l1 - l2) * 80.0
        };
        let hs = 230.0 * hs;
        // Two sides of a triangle 120° apart: neither difference hides the other.
        (hs * hs + lumi * lumi - 2.0 * hs * lumi * 120f32.to_radians().cos()).sqrt().max(60.0)
    });
    (colour + min_mm3.max(0.0)).min(MAX_FLUSH_MM3).floor()
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

/// Tower line width against the nozzle, as Bambu Studio prints it.
const LINE_PER_NOZZLE: f32 = 1.25;
/// Rib width (`prime_tower_rib_width`) and how far apart sparse lines are (`wipe_tower_bridging`).
const RIB_WIDTH_MM: f32 = 8.0;
const BRIDGING_MM: f32 = 10.0;
/// Infill gap of what is primed on the tower (`prime_tower_infill_gap`, 150 %).
const INFILL_GAP: f32 = 1.5;

/// The least a tower this tall is deep, so it stands (`WipeTower::min_depth_per_height`).
pub fn min_tower_depth(height_mm: f32) -> f32 {
    const STEPS: [(f32, f32); 4] = [(5.0, 5.0), (100.0, 20.0), (250.0, 40.0), (350.0, 60.0)];
    if height_mm <= STEPS[0].0 {
        return STEPS[0].1;
    }
    for w in STEPS.windows(2) {
        let ((h0, d0), (h1, d1)) = (w[0], w[1]);
        if height_mm < h1 {
            return d0 + (height_mm - h0) / (h1 - h0) * (d1 - d0);
        }
    }
    STEPS[3].1
}

/// Side of Bambu Studio's square tower for a print `height_mm` tall whose
/// busiest layer puts `per_layer_mm3` on it: wide enough for that layer at
/// 150 % infill gap, never under the least for its height, plus the ribs.
pub fn tower_side(per_layer_mm3: f32, layer_mm: f32, height_mm: f32) -> f32 {
    let depth = (per_layer_mm3.max(0.0) / layer_mm.max(0.01) * INFILL_GAP).sqrt();
    depth.max(min_tower_depth(height_mm)) + RIB_WIDTH_MM / std::f32::consts::SQRT_2
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
        let layer = p.layer_mm.max(0.01);
        let side = p.tower_depth_mm.unwrap_or_else(|| tower_side(busiest_layer, layer, swaps.layers as f32 * layer));
        let line = p.nozzle_mm * LINE_PER_NOZZLE;
        let height = tower_layers as f32 * layer;
        // The square's outline and its four ribs, on every layer.
        let walls = (4.0 * side + 4.0 * RIB_WIDTH_MM) * line * height;
        // Layers without a swap only bridge across, a line every 10 mm; the first is solid.
        let quiet = tower_layers.saturating_sub(changes.chunk_by(|a, b| a.0 == b.0).count() as u32) as f32;
        let sparse = quiet * side * side * layer * (line / BRIDGING_MM).min(1.0);
        let first = side * side * layer;
        let brim = p.tower_brim_mm.max(0.0);
        let brim_vol = ((side + 2.0 * brim).powi(2) - side * side) * layer;
        let priming = changes.len() as f32 * p.prime_mm3;
        w.tower_mm3 = walls + sparse + first + brim_vol + priming;
        w.tower_footprint_mm = [side + 2.0 * brim; 2];
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
        let to_white = default_flush(black, white, 107.0);
        let to_black = default_flush(white, black, 107.0);
        assert!(to_white > to_black, "{to_white} {to_black}");
        // Black to white: 560 for the lightness, plus the nozzle; white to black: 80 and a little hue.
        assert_eq!(to_white, 667.0);
        assert_eq!(to_black, 187.0);
        assert_eq!(default_flush(white, white, 107.0), 0.0);
        // Nearly the same colour still flushes the nozzle and the formula's floor.
        assert_eq!(default_flush([0.5, 0.2, 0.2], [0.52, 0.2, 0.2], 92.0), 152.0);
        assert!(default_flush(black, white, 600.0) <= MAX_FLUSH_MM3);
    }

    #[test]
    fn measured_colours_use_bambus_measurements() {
        let hex = |h: u32| rgb_of(h);
        // Black to Bambu yellow was measured at 450, on top of the nozzle's 107.
        assert_eq!(default_flush(hex(0x000000), hex(0xF4EE2A), 107.0), 557.0);
        // A colour within ΔE00 5 of a measured one counts as it.
        assert_eq!(default_flush(hex(0x020202), hex(0xF2EC2C), 92.0), 542.0);
        // Not measured in that direction: the formula.
        assert_ne!(default_flush(hex(0xF4EE2A), hex(0x5E43B7), 107.0), 107.0 + 270.0);
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
    fn a_tower_without_a_chute_grows_bigger() {
        let s = swaps(vec![(0, 0b001), (3, 0b111)]);
        let chute = waste(&s, &WasteParams::default(), |_, _| 300.0);
        let tower = waste(&s, &WasteParams { purge_chute: false, ..Default::default() }, |_, _| 300.0);
        assert!(tower.tower_footprint_mm[1] > chute.tower_footprint_mm[1]);
        // Square, as Bambu Studio 2 prints it.
        assert_eq!(tower.tower_footprint_mm[0], tower.tower_footprint_mm[1]);
        // The flush stays flush; the tower only adds its own walls.
        assert_eq!(tower.flush_mm3, chute.flush_mm3);
    }

    #[test]
    fn the_tower_is_sized_as_bambu_studio_sizes_it() {
        // Short prints: the least depth for the height wins, plus the ribs.
        assert_eq!(min_tower_depth(3.0), 5.0);
        assert!((min_tower_depth(175.0) - 30.0).abs() < 1e-4);
        assert_eq!(min_tower_depth(500.0), 60.0);
        let ribs = 8.0 / std::f32::consts::SQRT_2;
        assert!(
            (tower_side(45.0, 0.2, 50.0) - (min_tower_depth(50.0).max((45.0f32 / 0.2 * 1.5).sqrt()) + ribs)).abs()
                < 1e-4
        );
        // A busy layer outgrows it: four swaps' priming at 0.2 mm.
        assert!((tower_side(180.0, 0.2, 10.0) - ((180.0f32 / 0.2 * 1.5).sqrt() + ribs)).abs() < 1e-4);
    }

    #[test]
    fn a_set_tower_size_wins_over_the_estimate() {
        let s = swaps(vec![(0, 0b01), (3, 0b11)]);
        let w = waste(&s, &WasteParams { tower_depth_mm: Some(60.0), ..Default::default() }, |_, _| 300.0);
        assert_eq!(w.tower_footprint_mm, [66.0, 66.0]);
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
