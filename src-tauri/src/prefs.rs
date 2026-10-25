//! Print settings the waste estimate and the plate reservation use, kept in
//! `settings.json` next to `printers.json` so a new model starts with the
//! values the user set last time.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{in_range, AppError, CmdResult};
use crate::profiles::write_atomic;

/// What the waste estimate assumes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WastePrefs {
    /// Layer height; `None` follows the printer's process.
    pub layer_mm: Option<f32>,
    pub filament_diameter_mm: f32,
    pub density_g_cm3: f32,
    pub flush_multiplier: f32,
    pub start_purge_mm3: f32,
    pub prime_mm3: f32,
    /// Share of the flush infill and supports take (0 to 0.9).
    pub flush_reuse: f32,
}

impl Default for WastePrefs {
    fn default() -> Self {
        let p = bp_core::WasteParams::default();
        Self {
            layer_mm: None,
            filament_diameter_mm: p.filament_diameter_mm,
            density_g_cm3: p.density_g_cm3,
            flush_multiplier: p.flush_multiplier,
            start_purge_mm3: p.start_purge_mm3,
            prime_mm3: p.prime_mm3,
            flush_reuse: p.flush_reuse,
        }
    }
}

impl WastePrefs {
    /// Checks every value; `nozzle_mm` bounds the layer height.
    pub fn validated(self, nozzle_mm: f32) -> CmdResult<Self> {
        if let Some(l) = self.layer_mm {
            let max = (nozzle_mm * 0.8 * 100.0).round() / 100.0;
            if !(l.is_finite() && (0.04..=max).contains(&l)) {
                return Err(AppError::with(
                    "error.layerRange",
                    json!({ "min": 0.04, "max": max, "nozzle": nozzle_mm }),
                ));
            }
        }
        in_range(self.filament_diameter_mm, 1.0..=3.5, "error.filamentRange")?;
        in_range(self.density_g_cm3, 0.5..=3.0, "error.densityRange")?;
        in_range(self.flush_multiplier, 0.1..=3.0, "error.multiplierRange")?;
        in_range(self.start_purge_mm3, 0.0..=2000.0, "error.startPurgeRange")?;
        in_range(self.prime_mm3, 0.0..=500.0, "error.primeRange")?;
        in_range(self.flush_reuse, 0.0..=0.9, "error.reuseRange")?;
        Ok(self)
    }
}

/// What is kept free on the plate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlatePrefs {
    /// Keep the prime tower's space free.
    pub tower: bool,
    /// Tower depth; `None` uses the estimate.
    pub tower_depth_mm: Option<f32>,
    pub tower_gap_mm: f32,
    pub margin_mm: f32,
}

impl Default for PlatePrefs {
    fn default() -> Self {
        let r = bp_3mf::Reserve::default();
        Self { tower: true, tower_depth_mm: None, tower_gap_mm: r.tower_gap_mm, margin_mm: r.margin_mm }
    }
}

impl PlatePrefs {
    pub fn validated(self) -> CmdResult<Self> {
        if let Some(d) = self.tower_depth_mm {
            in_range(d, 5.0..=250.0, "error.towerDepthRange")?;
        }
        in_range(self.tower_gap_mm, 0.0..=50.0, "error.towerGapRange")?;
        in_range(self.margin_mm, 0.0..=50.0, "error.marginRange")?;
        Ok(self)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub waste: WastePrefs,
    pub plate: PlatePrefs,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Prefs {
    /// Reads the file; a missing, unreadable or out-of-range one falls back to the defaults.
    pub fn load(path: PathBuf) -> Self {
        let mut p: Prefs =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        p.waste = p.waste.validated(1.0).unwrap_or_default();
        p.plate = p.plate.validated().unwrap_or_default();
        p.path = Some(path);
        p
    }

    pub fn save(&self) -> CmdResult<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let body = serde_json::to_string_pretty(self).map_err(AppError::internal)?;
        write_atomic(path, &body).map_err(|e| AppError::file(&path.display().to_string(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_restart_and_bad_files_fall_back() {
        let path = std::env::temp_dir().join(format!("bp-settings-{}.json", std::process::id()));
        let mut p = Prefs::load(path.clone());
        assert_eq!(p.waste, WastePrefs::default());
        p.waste.flush_multiplier = 1.5;
        p.plate.tower = false;
        p.save().unwrap();
        let again = Prefs::load(path.clone());
        assert_eq!(again.waste.flush_multiplier, 1.5);
        assert!(!again.plate.tower);

        std::fs::write(&path, r#"{"waste":{"densityGCm3":99},"plate":{"marginMm":3}}"#).unwrap();
        let fixed = Prefs::load(path.clone());
        assert_eq!(fixed.waste, WastePrefs::default());
        assert_eq!(fixed.plate.margin_mm, 3.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_out_of_range_values() {
        let ok = WastePrefs::default();
        assert!(ok.validated(0.4).is_ok());
        assert_eq!(WastePrefs { layer_mm: Some(0.4), ..ok }.validated(0.4).unwrap_err().key, "error.layerRange");
        assert!(WastePrefs { layer_mm: Some(0.28), ..ok }.validated(0.4).is_ok());
        assert_eq!(WastePrefs { density_g_cm3: f32::NAN, ..ok }.validated(0.4).unwrap_err().key, "error.densityRange");
        assert_eq!(WastePrefs { flush_reuse: 1.0, ..ok }.validated(0.4).unwrap_err().key, "error.reuseRange");
        assert!(PlatePrefs { tower_depth_mm: Some(2.0), ..Default::default() }.validated().is_err());
        assert!(PlatePrefs { margin_mm: -1.0, ..Default::default() }.validated().is_err());
    }
}
