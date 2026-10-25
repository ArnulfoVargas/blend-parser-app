//! Printer profiles: the print volume, the nozzles a printer takes and, for the
//! printers Bambu Studio ships, the presets the 3MF names so it opens ready to
//! slice. Every preset name below exists in Bambu Studio's `resources/profiles/BBL`.

use serde::{Deserialize, Serialize};

/// Nozzle diameters the app offers, in millimetres.
pub const NOZZLES: [f32; 4] = [0.2, 0.4, 0.6, 0.8];

/// A printer as the UI picks it: from the catalogue or made by the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProfile {
    pub id: String,
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

/// Presets for one nozzle: (diameter, process, filament).
type NozzlePresets = [(f32, &'static str, &'static str); 4];

struct Bambu {
    id: &'static str,
    /// `printer_model` and the start of `printer_settings_id`.
    model: &'static str,
    volume: [f32; 3],
    presets: NozzlePresets,
}

const BAMBU: [Bambu; 9] = [
    Bambu {
        id: "A1",
        model: "Bambu Lab A1",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL A1 0.2 nozzle", "Bambu PLA Basic @BBL A1 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL A1", "Bambu PLA Basic @BBL A1"),
            (0.6, "0.30mm Strength @BBL A1 0.6 nozzle", "Bambu PLA Basic @BBL A1"),
            (0.8, "0.40mm Standard @BBL A1 0.8 nozzle", "Bambu PLA Basic @BBL A1"),
        ],
    },
    Bambu {
        id: "A1M",
        model: "Bambu Lab A1 mini",
        volume: [180.0, 180.0, 180.0],
        presets: [
            (0.2, "0.10mm Standard @BBL A1M 0.2 nozzle", "Bambu PLA Basic @BBL A1M 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL A1M", "Bambu PLA Basic @BBL A1M"),
            (0.6, "0.30mm Standard @BBL A1M 0.6 nozzle", "Bambu PLA Basic @BBL A1M"),
            (0.8, "0.40mm Standard @BBL A1M 0.8 nozzle", "Bambu PLA Basic @BBL A1M"),
        ],
    },
    Bambu {
        id: "P1P",
        model: "Bambu Lab P1P",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL P1P 0.2 nozzle", "Bambu PLA Basic @BBL P1P 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL P1P", "Bambu PLA Basic @BBL P1P"),
            (0.6, "0.30mm Standard @BBL P1P 0.6 nozzle", "Bambu PLA Basic @BBL P1P 0.6 nozzle"),
            (0.8, "0.40mm Standard @BBL P1P 0.8 nozzle", "Bambu PLA Basic @BBL P1P"),
        ],
    },
    Bambu {
        id: "P1S",
        model: "Bambu Lab P1S",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL X1C 0.2 nozzle", "Bambu PLA Basic @BBL X1C 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL X1C", "Bambu PLA Basic @BBL P1S 0.4 nozzle"),
            (0.6, "0.30mm Standard @BBL X1C 0.6 nozzle", "Bambu PLA Basic @BBL P1S 0.6 nozzle"),
            (0.8, "0.40mm Standard @BBL X1C 0.8 nozzle", "Bambu PLA Basic @BBL X1C 0.8 nozzle"),
        ],
    },
    Bambu {
        id: "P2S",
        model: "Bambu Lab P2S",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL P2S 0.2 nozzle", "Bambu PLA Basic @BBL P2S 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL P2S", "Bambu PLA Basic @BBL P2S"),
            (0.6, "0.30mm Standard @BBL P2S 0.6 nozzle", "Bambu PLA Basic @BBL P2S 0.6 nozzle"),
            (0.8, "0.40mm Standard @BBL P2S 0.8 nozzle", "Bambu PLA Basic @BBL P2S 0.8 nozzle"),
        ],
    },
    Bambu {
        id: "X1",
        model: "Bambu Lab X1",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL X1C 0.2 nozzle", "Bambu PLA Basic @BBL X1C 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL X1C", "Bambu PLA Basic @BBL X1"),
            (0.6, "0.30mm Standard @BBL X1 0.6 nozzle", "Bambu PLA Basic @BBL X1"),
            (0.8, "0.40mm Standard @BBL X1 0.8 nozzle", "Bambu PLA Basic @BBL X1"),
        ],
    },
    Bambu {
        id: "X1C",
        model: "Bambu Lab X1 Carbon",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL X1C 0.2 nozzle", "Bambu PLA Basic @BBL X1C 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL X1C", "Bambu PLA Basic @BBL X1C"),
            (0.6, "0.30mm Standard @BBL X1C 0.6 nozzle", "Bambu PLA Basic @BBL X1C 0.6 nozzle"),
            (0.8, "0.40mm Standard @BBL X1C 0.8 nozzle", "Bambu PLA Basic @BBL X1C 0.8 nozzle"),
        ],
    },
    Bambu {
        id: "X1E",
        model: "Bambu Lab X1E",
        volume: [256.0, 256.0, 256.0],
        presets: [
            (0.2, "0.10mm Standard @BBL X1C 0.2 nozzle", "Bambu PLA Basic @BBL X1C 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL X1C", "Bambu PLA Basic @BBL X1C"),
            (0.6, "0.30mm Standard @BBL X1C 0.6 nozzle", "Bambu PLA Basic @BBL X1C"),
            (0.8, "0.40mm Standard @BBL X1C 0.8 nozzle", "Bambu PLA Basic @BBL X1C 0.8 nozzle"),
        ],
    },
    Bambu {
        id: "H2S",
        model: "Bambu Lab H2S",
        volume: [340.0, 320.0, 340.0],
        presets: [
            (0.2, "0.10mm Standard @BBL H2S 0.2 nozzle", "Bambu PLA Basic @BBL H2S 0.2 nozzle"),
            (0.4, "0.20mm Standard @BBL H2S", "Bambu PLA Basic @BBL H2S"),
            (0.6, "0.30mm Standard @BBL H2S 0.6 nozzle", "Bambu PLA Basic @BBL H2S 0.6 nozzle"),
            (0.8, "0.40mm Standard @BBL H2S 0.8 nozzle", "Bambu PLA Basic @BBL H2S 0.8 nozzle"),
        ],
    },
];

fn bambu(id: &str) -> Option<&'static Bambu> {
    BAMBU.iter().find(|b| b.id == id)
}

/// The printers the app knows, Bambu Lab first as Bambu Studio lists them.
pub fn catalog() -> Vec<PrinterProfile> {
    BAMBU
        .iter()
        .map(|b| PrinterProfile {
            id: b.id.into(),
            name: b.model.into(),
            volume_mm: b.volume,
            nozzles: NOZZLES.to_vec(),
            base: Some(b.id.into()),
            custom: false,
        })
        .collect()
}

/// Catalogue printers whose presets a custom profile can borrow, as (id, name).
pub fn bambu_bases() -> Vec<(&'static str, &'static str)> {
    BAMBU.iter().map(|b| (b.id, b.model)).collect()
}

/// What Bambu Studio is told about the printer.
pub(crate) struct Presets {
    pub printer_model: &'static str,
    pub printer_settings_id: String,
    pub process: &'static str,
    pub filament: &'static str,
}

/// The printer and nozzle a model is prepared and exported for.
#[derive(Clone, Debug, PartialEq)]
pub struct Printer {
    pub profile: PrinterProfile,
    pub nozzle_mm: f32,
}

impl Default for Printer {
    fn default() -> Self {
        Self::new(catalog().swap_remove(0), 0.4)
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

    /// Where the model's centre goes on the plate.
    pub(crate) fn plate_centre(&self) -> (f32, f32) {
        let (w, d) = self.bed_mm();
        (w / 2.0, d / 2.0)
    }

    pub(crate) fn presets(&self) -> Option<Presets> {
        let b = bambu(self.profile.base.as_deref()?)?;
        let &(n, process, filament) = b.presets.iter().find(|p| (p.0 - self.nozzle_mm).abs() < 1e-3)?;
        Some(Presets {
            printer_model: b.model,
            printer_settings_id: format!("{} {n} nozzle", b.model),
            process,
            filament,
        })
    }

    /// Layer height of the process the 3MF names (half the nozzle when there is none).
    pub fn layer_mm(&self) -> f32 {
        self.presets().and_then(|p| p.process.split("mm").next()?.parse().ok()).unwrap_or(self.nozzle_mm / 2.0)
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
        catalog().into_iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn every_catalogue_printer_has_presets_for_every_nozzle() {
        for p in catalog() {
            for n in NOZZLES {
                let printer = Printer::new(p.clone(), n);
                let pr = printer.presets().unwrap_or_else(|| panic!("{} {n}", p.id));
                assert_eq!(pr.printer_settings_id, format!("{} {n} nozzle", p.name));
                assert!((printer.layer_mm() - n / 2.0).abs() < 1e-6, "{} {n}", p.id);
            }
        }
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
