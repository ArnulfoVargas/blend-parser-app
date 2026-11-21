//! Filaments to pick slot colours from: Bambu Lab's own colours, as Bambu
//! Studio lists them (`filaments.json`, written by `scripts/sync-filaments.mjs`),
//! and the user's. For a filament on a slot, the 3MF names its material and,
//! where Bambu Studio ships one for the printer, its preset.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::Printer;

/// A spool colour, from the catalogue or the user's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filament {
    /// Bambu's colour code for catalogue ones ("10300"); `custom-…` for the user's.
    pub id: String,
    pub brand: String,
    /// Line, as Bambu names it: "PLA Basic", "PETG HF".
    pub kind: String,
    /// Colour name in Spanish and in English.
    pub name_es: String,
    pub name_en: String,
    /// `#RRGGBB`.
    pub hex: String,
    pub custom: bool,
}

#[derive(Deserialize)]
struct Type {
    preset: Option<String>,
    material: Option<String>,
    /// Bambu's filament id ("GFA00").
    id: Option<String>,
    #[serde(default)]
    support: bool,
    printers: Vec<String>,
}

#[derive(Deserialize)]
struct Pair {
    from: String,
    to: String,
    purge: f32,
}

#[derive(Deserialize)]
struct Colour {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    hex: String,
    es: String,
    en: String,
}

#[derive(Deserialize)]
struct Source {
    types: HashMap<String, Type>,
    #[serde(default)]
    pairs: Vec<Pair>,
    colours: Vec<Colour>,
}

fn source() -> &'static Source {
    static SOURCE: OnceLock<Source> = OnceLock::new();
    SOURCE.get_or_init(|| serde_json::from_str(include_str!("filaments.json")).expect("filaments.json is valid"))
}

/// Bambu Lab's single-colour filaments, by line and colour code.
pub fn catalog() -> Vec<Filament> {
    source()
        .colours
        .iter()
        .map(|c| Filament {
            id: c.id.clone(),
            brand: "Bambu Lab".into(),
            kind: c.kind.clone(),
            name_es: c.es.clone(),
            name_en: c.en.clone(),
            hex: c.hex.clone(),
            custom: false,
        })
        .collect()
}

/// What the 3MF says about a slot's filament.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotFilament {
    /// Bambu Studio's material (`filament_type`): "PLA", "PETG", "TPU-AMS".
    pub material: String,
    /// Its preset for the printer, when Bambu Studio ships one; otherwise the
    /// printer's own PLA preset stays.
    pub preset: Option<String>,
}

/// The material a line prints as. Lines Bambu Studio has no preset for, and
/// the user's own, read it from their first word ("PETG Translucent" is PETG).
pub fn material(kind: &str) -> String {
    match source().types.get(kind).and_then(|t| t.material.clone()) {
        Some(m) => m,
        None => kind.split([' ', '-']).next().filter(|w| !w.is_empty()).unwrap_or("PLA").to_uppercase(),
    }
}

/// The 3MF's filament for a slot of `kind`, prepared for `printer`.
pub fn for_slot(kind: &str, printer: &Printer) -> SlotFilament {
    let preset = (|| {
        let t = source().types.get(kind)?;
        let own = printer.presets()?.filament;
        // The printer's PLA preset ends in its suffix: "Bambu PLA Basic @BBL X1C 0.6 nozzle".
        let suffix = &own[own.find("@BBL ")?..];
        let name = t.preset.as_deref()?;
        t.printers.iter().any(|p| p == suffix).then(|| format!("{name} {suffix}"))
    })();
    SlotFilament { material: material(kind), preset }
}

/// Flush into a support material, and the least out of one, as Bambu Studio sets them.
const FLUSH_TO_SUPPORT_MM3: f32 = 230.0;
const MIN_FLUSH_FROM_SUPPORT_MM3: f32 = 700.0;
/// Bambu Studio's cap on one flush (`bp_core::waste::MAX_FLUSH_MM3`).
const MAX_FLUSH_MM3: f32 = 900.0;

/// The flush of a swap between two slots' lines (`None` for a slot without a
/// filament), from `colour` (the colour-based flush, nozzle included), as
/// Bambu Studio's flush table does: into a support material a set 230 mm³;
/// otherwise at least what Bambu measured between the two materials (PLA and
/// PETG: 650), and out of a support material at least 700.
pub fn flush(from: Option<&str>, to: Option<&str>, colour: f32) -> f32 {
    let types = &source().types;
    let line = |k: Option<&str>| k.and_then(|k| types.get(k));
    let (a, b) = (line(from), line(to));
    if b.is_some_and(|t| t.support) {
        return FLUSH_TO_SUPPORT_MM3;
    }
    let ids = a.and_then(|t| t.id.as_deref()).zip(b.and_then(|t| t.id.as_deref()));
    let measured = ids.and_then(|(x, y)| source().pairs.iter().find(|p| p.from == x && p.to == y)).map(|p| p.purge);
    let v = measured.map_or(colour, |m| colour.max(m).min(MAX_FLUSH_MM3));
    if a.is_some_and(|t| t.support) {
        v.max(MIN_FLUSH_FROM_SUPPORT_MM3)
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog as printers;

    #[test]
    fn the_catalogue_has_bambus_colours() {
        let all = catalog();
        assert!(all.len() > 200);
        assert!(all.iter().all(|f| f.hex.len() == 7 && f.hex.starts_with('#') && !f.name_en.is_empty()));
        let orange = all.iter().find(|f| f.kind == "PLA Basic" && f.name_en == "Orange").unwrap();
        assert_eq!((orange.hex.as_str(), orange.name_es.as_str()), ("#FF6A13", "Naranja"));
        let mut ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len(), "ids are unique");
    }

    #[test]
    fn names_the_preset_for_the_printer() {
        let x1c = printers().into_iter().find(|p| p.id == "Bambu Lab X1 Carbon").unwrap();
        let s = for_slot("PLA Matte", &Printer::new(x1c.clone(), 0.4));
        assert_eq!(s.material, "PLA");
        assert_eq!(s.preset.as_deref(), Some("Bambu PLA Matte @BBL X1C"));
        let s = for_slot("PETG HF", &Printer::new(x1c, 0.6));
        assert_eq!(s.material, "PETG");
        assert_eq!(s.preset.as_deref(), Some("Bambu PETG HF @BBL X1C 0.6 nozzle"));
        // Not a Bambu printer, or not a Bambu line: the material only.
        let other = printers().into_iter().find(|p| p.vendor != "Bambu Lab").unwrap();
        assert_eq!(for_slot("PLA Matte", &Printer::new(other, 0.4)).preset, None);
        assert_eq!(for_slot("PETG", &Printer::default()), SlotFilament { material: "PETG".into(), preset: None });
        assert_eq!(material("Silk PLA"), "SILK");
    }

    #[test]
    fn material_pairs_and_supports_set_their_own_flush() {
        // PLA after PETG was measured at 650: more than the colours ask for.
        assert_eq!(flush(Some("PETG Basic"), Some("PLA Basic"), 300.0), 650.0);
        assert_eq!(flush(Some("PETG Basic"), Some("PLA Basic"), 800.0), 800.0);
        // Same material, or no filament picked: the colours decide.
        assert_eq!(flush(Some("PLA Basic"), Some("PLA Matte"), 300.0), 300.0);
        assert_eq!(flush(None, Some("PLA Basic"), 300.0), 300.0);
        // Into a support material a set amount; out of one, at least 700.
        assert_eq!(flush(Some("PLA Basic"), Some("Support for PLA"), 500.0), 230.0);
        assert_eq!(flush(Some("Support for PLA"), Some("PLA Basic"), 200.0), 700.0);
    }
}
