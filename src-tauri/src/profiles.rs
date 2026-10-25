//! The printers the user made and the last printer and nozzle picked, kept in
//! `printers.json` in the app's config folder so they survive a restart.

use std::path::{Path, PathBuf};

use bp_3mf::{Printer, PrinterProfile, NOZZLES};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, CmdResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub id: String,
    pub nozzle_mm: f32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[serde(default)]
    pub custom: Vec<PrinterProfile>,
    #[serde(default)]
    pub selected: Option<Selection>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Store {
    /// Reads the file; a missing or unreadable one starts empty.
    pub fn load(path: PathBuf) -> Self {
        let mut store: Store =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        // Choices saved with the first catalogue's short ids ("A1", "X1C").
        for p in &mut store.custom {
            p.base = p.base.take().map(|b| bp_3mf::canonical_id(&b).to_string());
        }
        if let Some(sel) = &mut store.selected {
            sel.id = bp_3mf::canonical_id(&sel.id).to_string();
        }
        store.custom.retain(|p| validate(p).is_ok());
        store.path = Some(path);
        store
    }

    pub fn save(&self) -> CmdResult<()> {
        let Some(path) = &self.path else { return Ok(()) };
        write_atomic(path, &serde_json::to_string_pretty(self).map_err(AppError::internal)?)
            .map_err(|e| AppError::file(&path.display().to_string(), e))
    }

    /// The catalogue, then the user's printers.
    pub fn profiles(&self) -> Vec<PrinterProfile> {
        bp_3mf::catalog().into_iter().chain(self.custom.iter().cloned()).collect()
    }

    /// The picked printer, or the first in the catalogue.
    pub fn printer(&self) -> Printer {
        let all = self.profiles();
        let sel = self.selected.as_ref();
        let profile = sel.and_then(|s| all.iter().find(|p| p.id == s.id)).unwrap_or(&all[0]).clone();
        Printer::new(profile, sel.map_or(0.4, |s| s.nozzle_mm))
    }

    pub fn select(&mut self, id: &str, nozzle_mm: f32) -> CmdResult<Printer> {
        let id = bp_3mf::canonical_id(id);
        let profile = self
            .profiles()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AppError::with("error.unknownPrinter", json!({ "id": id })))?;
        let printer = Printer::new(profile, nozzle_mm);
        self.selected = Some(Selection { id: id.into(), nozzle_mm: printer.nozzle_mm });
        Ok(printer)
    }

    /// Adds or replaces a custom printer; a blank id makes a new one.
    pub fn upsert(&mut self, mut p: PrinterProfile) -> CmdResult<PrinterProfile> {
        p.name = p.name.trim().to_string();
        p.custom = true;
        p.vendor.clear();
        p.nozzles.sort_by(f32::total_cmp);
        p.nozzles.dedup();
        validate(&p)?;
        if self.custom.iter().any(|c| c.id != p.id && c.name.eq_ignore_ascii_case(&p.name))
            || bp_3mf::catalog().iter().any(|c| c.name.eq_ignore_ascii_case(&p.name))
        {
            return Err(AppError::with("error.printerNameTaken", json!({ "name": p.name })));
        }
        if p.id.is_empty() {
            let mut n = self.custom.len() + 1;
            while self.custom.iter().any(|c| c.id == format!("custom-{n}")) {
                n += 1;
            }
            p.id = format!("custom-{n}");
        }
        match self.custom.iter_mut().find(|c| c.id == p.id) {
            Some(c) => *c = p.clone(),
            None if p.id.starts_with("custom-") => self.custom.push(p.clone()),
            None => return Err(AppError::new("error.onlyOwnPrinters")),
        }
        Ok(p)
    }

    pub fn remove(&mut self, id: &str) {
        self.custom.retain(|c| c.id != id);
        if self.selected.as_ref().is_some_and(|s| s.id == id) {
            self.selected = None;
        }
    }
}

fn validate(p: &PrinterProfile) -> CmdResult<()> {
    if p.name.trim().is_empty() || p.name.chars().count() > 60 {
        return Err(AppError::new("error.printerName"));
    }
    if p.volume_mm.iter().any(|v| !(20.0..=2000.0).contains(v)) {
        return Err(AppError::new("error.printerVolume"));
    }
    if p.nozzles.is_empty() || p.nozzles.iter().any(|n| !NOZZLES.iter().any(|k| (k - n).abs() < 1e-3)) {
        return Err(AppError::new("error.printerNozzles"));
    }
    if let Some(base) = &p.base {
        if !bp_3mf::catalog().iter().any(|c| &c.id == base) {
            return Err(AppError::with("error.printerBase", json!({ "base": base })));
        }
    }
    Ok(())
}

/// Writes next to the file and renames, so a crash never leaves half a file.
pub(crate) fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ender() -> PrinterProfile {
        PrinterProfile {
            id: String::new(),
            vendor: "Creality".into(),
            name: " Ender 3 ".into(),
            volume_mm: [220.0, 220.0, 250.0],
            nozzles: vec![0.6, 0.4, 0.4],
            base: None,
            custom: false,
        }
    }

    #[test]
    fn custom_printers_survive_a_restart() {
        let path = std::env::temp_dir().join(format!("bp-printers-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = Store::load(path.clone());
        let p = s.upsert(ender()).unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str(), p.nozzles.as_slice()), ("custom-1", "Ender 3", &[0.4, 0.6][..]));
        s.select(&p.id, 0.6).unwrap();
        s.save().unwrap();

        let again = Store::load(path.clone());
        let printer = again.printer();
        assert_eq!(printer.profile.name, "Ender 3");
        assert_eq!(printer.nozzle_mm, 0.6);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_profiles() {
        let mut s = Store::default();
        assert!(s.upsert(PrinterProfile { name: "".into(), ..ender() }).is_err());
        assert!(s.upsert(PrinterProfile { volume_mm: [5.0, 220.0, 250.0], ..ender() }).is_err());
        assert!(s.upsert(PrinterProfile { nozzles: vec![], ..ender() }).is_err());
        assert!(s.upsert(PrinterProfile { base: Some("Z9".into()), ..ender() }).is_err());
        assert!(s.upsert(PrinterProfile { name: "Bambu Lab A1".into(), ..ender() }).is_err());
        assert!(s.upsert(PrinterProfile { id: "A1".into(), ..ender() }).is_err());
    }

    #[test]
    fn removing_the_selected_printer_falls_back() {
        let mut s = Store::default();
        let p = s.upsert(ender()).unwrap();
        s.select(&p.id, 0.4).unwrap();
        s.remove(&p.id);
        assert_eq!(s.printer().profile.id, "Bambu Lab A1");
    }

    #[test]
    fn old_short_ids_still_resolve() {
        let path = std::env::temp_dir().join(format!("bp-printers-old-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"custom":[{"id":"custom-1","name":"Mía","volumeMm":[200,200,200],"nozzles":[0.4],"base":"X1C","custom":true}],"selected":{"id":"A1M","nozzleMm":0.6}}"#).unwrap();
        let s = Store::load(path.clone());
        assert_eq!(s.printer().profile.id, "Bambu Lab A1 mini");
        assert_eq!(s.printer().nozzle_mm, 0.6);
        assert_eq!(s.custom[0].base.as_deref(), Some("Bambu Lab X1 Carbon"));
        let _ = std::fs::remove_file(&path);
    }
}
