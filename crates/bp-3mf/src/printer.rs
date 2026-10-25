//! Printer profiles: the print volume, the nozzles a printer takes and, for the
//! printers Bambu Studio ships, the presets the 3MF names so it opens ready to
//! slice. The catalogue is `catalog.json`, generated from Bambu Studio's
//! `resources/profiles` by `scripts/sync-printers.mjs`.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Nozzle diameters a custom printer can take, in millimetres.
pub const NOZZLES: [f32; 4] = [0.2, 0.4, 0.6, 0.8];

/// Filament preset when the catalogue has none for a nozzle, or for a generic printer.
const GENERIC_PLA: &str = "Generic PLA";

/// A printer as the UI picks it: from the catalogue or made by the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProfile {
    pub id: String,
    /// Brand, to group and search by; empty for the user's own printers.
    #[serde(default)]
    pub vendor: String,
    pub name: String,
    /// Printable width, depth and height in millimetres.
    pub volume_mm: [f32; 3],
    /// Nozzle diameters it takes, in millimetres.
    pub nozzles: Vec<f32>,
    /// Catalogue printer whose Bambu Studio presets the 3MF names; `None`
    /// writes a generic printer with this profile's volume and nozzle.
    pub base: Option<String>,
    /// Made by the user, saved between sessions.
    #[serde(default)]
    pub custom: bool,
}

#[derive(Deserialize)]
struct CatalogFile {
    printers: Vec<CatalogPrinter>,
}

#[derive(Deserialize)]
struct CatalogPrinter {
    id: String,
    vendor: String,
    name: String,
    /// Printable area as min x, min y, max x, max y.
    area: [f32; 4],
    height: f32,
    nozzles: Vec<CatalogNozzle>,
}

#[derive(Deserialize)]
struct CatalogNozzle {
    diameter: f32,
    printer: String,
    process: Option<String>,
    layer: Option<f32>,
    filament: Option<String>,
}

fn printers() -> &'static [CatalogPrinter] {
    static CATALOG: OnceLock<Vec<CatalogPrinter>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<CatalogFile>(include_str!("catalog.json")).expect("catalog.json is valid").printers
    })
}

fn find(id: &str) -> Option<&'static CatalogPrinter> {
    let id = canonical_id(id);
    printers().iter().find(|p| p.id == id)
}

/// Ids of the first catalogue, kept so older saved choices still resolve.
pub fn canonical_id(id: &str) -> &str {
    match id {
        "A1" => "Bambu Lab A1",
        "A1M" => "Bambu Lab A1 mini",
        "P1P" => "Bambu Lab P1P",
        "P1S" => "Bambu Lab P1S",
        "P2S" => "Bambu Lab P2S",
        "X1" => "Bambu Lab X1",
        "X1C" => "Bambu Lab X1 Carbon",
        "X1E" => "Bambu Lab X1E",
        "H2S" => "Bambu Lab H2S",
        other => other,
    }
}

/// Every printer the app knows: Bambu Lab first, then the other brands.
pub fn catalog() -> Vec<PrinterProfile> {
    printers()
        .iter()
        .map(|p| PrinterProfile {
            id: p.id.clone(),
            vendor: p.vendor.clone(),
            name: p.name.clone(),
            volume_mm: [p.area[2] - p.area[0], p.area[3] - p.area[1], p.height],
            nozzles: p.nozzles.iter().map(|n| n.diameter).collect(),
            base: Some(p.id.clone()),
            custom: false,
        })
        .collect()
}

/// What Bambu Studio is told about the printer.
pub(crate) struct Presets {
    pub printer_model: String,
    pub printer_settings_id: String,
    pub process: String,
    pub filament: String,
}

/// The printer and nozzle a model is prepared and exported for.
#[derive(Clone, Debug, PartialEq)]
pub struct Printer {
    pub profile: PrinterProfile,
    pub nozzle_mm: f32,
}

impl Default for Printer {
    fn default() -> Self {
        let a1 = catalog().into_iter().find(|p| p.id == "Bambu Lab A1").expect("the A1 is in the catalogue");
        Self::new(a1, 0.4)
    }
}

impl Printer {
    /// `nozzle_mm` falls back to the profile's closest nozzle when it does not take it.
    pub fn new(profile: PrinterProfile, nozzle_mm: f32) -> Self {
        let nozzle_mm = profile
            .nozzles
            .iter()
            .copied()
            .min_by(|a, b| (a - nozzle_mm).abs().total_cmp(&(b - nozzle_mm).abs()))
            .unwrap_or(0.4);
        Self { profile, nozzle_mm }
    }

    /// Printable area in millimetres.
    pub fn bed_mm(&self) -> (f32, f32) {
        (self.profile.volume_mm[0], self.profile.volume_mm[1])
    }

    pub fn max_height_mm(&self) -> f32 {
        self.profile.volume_mm[2]
    }

    /// Where the model's centre goes: the middle of the catalogue printer's
    /// printable area, which does not always start at 0, 0.
    pub(crate) fn plate_centre(&self) -> (f32, f32) {
        match find(&self.profile.id).filter(|_| !self.profile.custom) {
            Some(p) => ((p.area[0] + p.area[2]) / 2.0, (p.area[1] + p.area[3]) / 2.0),
            None => (self.profile.volume_mm[0] / 2.0, self.profile.volume_mm[1] / 2.0),
        }
    }

    fn nozzle(&self) -> Option<(&'static CatalogPrinter, &'static CatalogNozzle)> {
        let p = find(self.profile.base.as_deref()?)?;
        Some((p, p.nozzles.iter().find(|n| (n.diameter - self.nozzle_mm).abs() < 1e-3)?))
    }

    pub(crate) fn presets(&self) -> Option<Presets> {
        let (p, n) = self.nozzle()?;
        Some(Presets {
            printer_model: p.id.clone(),
            printer_settings_id: n.printer.clone(),
            process: n.process.clone().unwrap_or_default(),
            filament: n.filament.clone().unwrap_or_else(|| GENERIC_PLA.into()),
        })
    }

    /// Layer height of the process the 3MF names (half the nozzle when there is none).
    pub fn layer_mm(&self) -> f32 {
        self.nozzle().and_then(|(_, n)| n.layer).unwrap_or(self.nozzle_mm / 2.0)
    }

    /// Smallest colour patch the nozzle draws cleanly: two lines wide each way.
    pub fn min_detail_mm2(&self) -> f32 {
        (2.0 * self.nozzle_mm).powi(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_id(id: &str) -> PrinterProfile {
        catalog().into_iter().find(|p| p.id == canonical_id(id)).unwrap()
    }

    #[test]
    fn catalogue_covers_bambu_lab_and_other_brands() {
        let all = catalog();
        for id in ["A1", "A1M", "Bambu Lab A2L", "P1S", "X1C", "Prusa MK3S", "Creality K1"] {
            assert!(all.iter().any(|p| p.id == canonical_id(id)), "{id}");
        }
        assert!(all.len() > 50);
        assert_eq!(all[0].vendor, "Bambu Lab");
        assert!(all.iter().all(|p| !p.nozzles.is_empty() && p.volume_mm.iter().all(|&v| v > 0.0)));
    }

    #[test]
    fn every_catalogue_nozzle_names_a_machine_preset() {
        for p in catalog() {
            for &n in &p.nozzles {
                let printer = Printer::new(p.clone(), n);
                let pr = printer.presets().unwrap_or_else(|| panic!("{} {n}", p.id));
                assert!(pr.printer_settings_id.ends_with(" nozzle"), "{}", pr.printer_settings_id);
                assert!(printer.layer_mm() > 0.0 && printer.layer_mm() <= n, "{} {n}", p.id);
            }
        }
    }

    #[test]
    fn bambu_presets_match_bambu_studio() {
        let p = Printer::new(by_id("X1C"), 0.6);
        let pr = p.presets().unwrap();
        assert_eq!(pr.printer_model, "Bambu Lab X1 Carbon");
        assert_eq!(pr.printer_settings_id, "Bambu Lab X1 Carbon 0.6 nozzle");
        assert_eq!(pr.process, "0.30mm Standard @BBL X1C 0.6 nozzle");
        assert_eq!(pr.filament, "Bambu PLA Basic @BBL X1C 0.6 nozzle");
        assert_eq!(p.layer_mm(), 0.3);
        assert_eq!(Printer::new(by_id("A1M"), 0.4).plate_centre(), (90.0, 90.0));
    }

    #[test]
    fn unsupported_nozzle_falls_back_to_the_closest() {
        let mut p = by_id("A1M");
        p.nozzles = vec![0.4, 0.6];
        assert_eq!(Printer::new(p.clone(), 0.8).nozzle_mm, 0.6);
        assert_eq!(Printer::new(p, 0.2).nozzle_mm, 0.4);
    }

    #[test]
    fn generic_profile_has_no_presets() {
        let p = PrinterProfile {
            id: "custom-1".into(),
            vendor: String::new(),
            name: "Ender 3".into(),
            volume_mm: [220.0, 220.0, 250.0],
            nozzles: vec![0.4],
            base: None,
            custom: true,
        };
        let printer = Printer::new(p, 0.4);
        assert!(printer.presets().is_none());
        assert_eq!(printer.layer_mm(), 0.2);
        assert_eq!(printer.plate_centre(), (110.0, 110.0));
    }
}
