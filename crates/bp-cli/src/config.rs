//! The app's own settings, read from its settings folder so the command line
//! converts with the printers and waste settings the user set in the window:
//! `printers.json` (their printers and the last pick) and `settings.json`.

use std::path::{Path, PathBuf};

use bp_3mf::{Printer, PrinterProfile};
use serde::Deserialize;

/// Tauri's identifier, which names the app's settings folder.
const APP_ID: &str = "com.arnulfovargas.blendparser";

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Selection {
    id: String,
    nozzle_mm: f32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Printers {
    custom: Vec<PrinterProfile>,
    selected: Option<Selection>,
}

/// What the waste estimate and the 3MF take from the app's settings.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Waste {
    pub layer_mm: Option<f32>,
    pub filament_diameter_mm: f32,
    pub density_g_cm3: f32,
    pub flush_multiplier: f32,
    pub start_purge_mm3: f32,
    pub prime_mm3: f32,
    pub flush_reuse: f32,
}

impl Default for Waste {
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

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Plate {
    tower_depth_mm: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    waste: Waste,
    plate: Plate,
}

pub struct Config {
    custom: Vec<PrinterProfile>,
    selected: Option<Selection>,
    pub waste: Waste,
    pub tower_depth_mm: Option<f32>,
}

/// Where Tauri keeps the app's settings on this system.
fn app_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))
    };
    base.map(|b| b.join(APP_ID))
}

fn read<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

impl Config {
    /// The settings in `dir`, or in the app's folder; anything missing or unreadable takes its default.
    pub fn load(dir: Option<&Path>) -> Self {
        let dir = dir.map(Path::to_path_buf).or_else(app_dir).unwrap_or_default();
        let printers: Printers = read(&dir.join("printers.json"));
        let settings: Settings = read(&dir.join("settings.json"));
        Self {
            custom: printers.custom.into_iter().filter(|p| p.custom).collect(),
            selected: printers.selected,
            waste: settings.waste,
            tower_depth_mm: settings.plate.tower_depth_mm,
        }
    }

    /// The catalogue, then the user's printers.
    pub fn profiles(&self) -> Vec<PrinterProfile> {
        bp_3mf::catalog().into_iter().chain(self.custom.iter().cloned()).collect()
    }

    /// The app's last pick, or the first in the catalogue, as the app does.
    pub fn printer(&self) -> Printer {
        let all = self.profiles();
        let sel = self.selected.as_ref();
        let profile =
            sel.and_then(|s| all.iter().find(|p| p.id == bp_3mf::canonical_id(&s.id))).unwrap_or(&all[0]).clone();
        Printer::new(profile, sel.map_or(0.4, |s| s.nozzle_mm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_apps_settings() {
        let dir = std::env::temp_dir().join(format!("bp-cli-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("printers.json"),
            r#"{"custom":[{"id":"custom-1","vendor":"","name":"Mine","volumeMm":[200,200,180],"nozzles":[0.4,0.6],"base":null,"custom":true}],"selected":{"id":"custom-1","nozzleMm":0.6}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"waste":{"flushMultiplier":1.5,"layerMm":0.16},"plate":{"towerDepthMm":30}}"#,
        )
        .unwrap();
        let c = Config::load(Some(&dir));
        assert_eq!(c.printer().profile.name, "Mine");
        assert_eq!(c.printer().nozzle_mm, 0.6);
        assert_eq!(c.waste.flush_multiplier, 1.5);
        assert_eq!(c.waste.layer_mm, Some(0.16));
        assert_eq!(c.waste.prime_mm3, bp_core::WasteParams::default().prime_mm3);
        assert_eq!(c.tower_depth_mm, Some(30.0));
        let _ = std::fs::remove_dir_all(&dir);

        // Nothing there: the app's defaults.
        let c = Config::load(Some(Path::new("/nonexistent/bp")));
        assert_eq!(c.printer().profile.id, bp_3mf::catalog()[0].id);
    }
}
