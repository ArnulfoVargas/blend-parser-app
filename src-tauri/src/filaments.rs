//! The user's filaments: their own spools beside Bambu Lab's catalogue, which
//! ones they have, and whether colours are picked only from those. Kept in
//! `filaments.json` in the app's config folder.

use std::collections::BTreeSet;
use std::path::PathBuf;

use bp_3mf::filament::Filament;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, CmdResult};
use crate::profiles::write_atomic;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FilamentStore {
    pub custom: Vec<Filament>,
    /// Ids of the filaments the user has.
    pub owned: BTreeSet<String>,
    /// Suggest colours only from the owned filaments.
    pub inventory_only: bool,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl FilamentStore {
    /// Reads the file; a missing or unreadable one starts empty.
    pub fn load(path: PathBuf) -> Self {
        let mut s: FilamentStore =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        s.custom.retain(|f| validate(f).is_ok());
        s.path = Some(path);
        s
    }

    pub fn save(&self) -> CmdResult<()> {
        let Some(path) = &self.path else { return Ok(()) };
        write_atomic(path, &serde_json::to_string_pretty(self).map_err(AppError::internal)?)
            .map_err(|e| AppError::file(&path.display().to_string(), e))
    }

    /// The catalogue, then the user's own.
    pub fn all(&self) -> Vec<Filament> {
        bp_3mf::filament::catalog().into_iter().chain(self.custom.iter().cloned()).collect()
    }

    pub fn find(&self, id: &str) -> Option<Filament> {
        self.all().into_iter().find(|f| f.id == id)
    }

    /// The filaments the user has, in catalogue order.
    pub fn owned(&self) -> Vec<Filament> {
        self.all().into_iter().filter(|f| self.owned.contains(&f.id)).collect()
    }

    /// Colours come only from the owned filaments: asked for, and there are some.
    pub fn inventory(&self) -> Option<Vec<Filament>> {
        let owned = self.owned();
        (self.inventory_only && !owned.is_empty()).then_some(owned)
    }

    pub fn set_owned(&mut self, id: &str, owned: bool) -> CmdResult<()> {
        if self.find(id).is_none() {
            return Err(AppError::new("error.unknownFilament"));
        }
        if owned {
            self.owned.insert(id.to_string());
        } else {
            self.owned.remove(id);
        }
        Ok(())
    }

    /// Adds or replaces one of the user's filaments; a blank id makes a new one, owned.
    pub fn upsert(&mut self, mut f: Filament) -> CmdResult<Filament> {
        f.name_es = f.name_es.trim().to_string();
        f.name_en = if f.name_en.trim().is_empty() { f.name_es.clone() } else { f.name_en.trim().to_string() };
        f.brand = f.brand.trim().to_string();
        f.kind = f.kind.trim().to_string();
        f.hex = f.hex.trim().to_uppercase();
        f.custom = true;
        validate(&f)?;
        match self.custom.iter_mut().find(|c| !f.id.is_empty() && c.id == f.id) {
            Some(c) => *c = f.clone(),
            None => {
                let n = (1..).find(|n| !self.custom.iter().any(|c| c.id == format!("custom-{n}"))).unwrap_or(1);
                f.id = format!("custom-{n}");
                self.owned.insert(f.id.clone());
                self.custom.push(f.clone());
            }
        }
        Ok(f)
    }

    pub fn remove(&mut self, id: &str) {
        self.custom.retain(|c| c.id != id);
        self.owned.remove(id);
    }
}

fn validate(f: &Filament) -> CmdResult<()> {
    if f.name_es.is_empty() || f.name_es.chars().count() > 60 {
        return Err(AppError::new("error.filamentName"));
    }
    if f.kind.is_empty() || f.kind.chars().count() > 40 || f.brand.chars().count() > 40 {
        return Err(AppError::new("error.filamentKind"));
    }
    if bp_core::color::parse_hex(&f.hex).is_none() {
        return Err(AppError::with("error.badColor", serde_json::json!({ "color": f.hex })));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(name: &str, hex: &str) -> Filament {
        Filament {
            id: String::new(),
            brand: "Sunlu".into(),
            kind: "PLA Matte".into(),
            name_es: name.into(),
            name_en: String::new(),
            hex: hex.into(),
            custom: false,
        }
    }

    #[test]
    fn inventory_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("bp-filaments-{}.json", std::process::id()));
        let mut s = FilamentStore::load(path.clone());
        assert!(s.inventory().is_none(), "nothing owned: colours stay free");
        let mine = s.upsert(own("Verde menta", "#7fd1ae")).unwrap();
        assert_eq!(
            (mine.id.as_str(), mine.hex.as_str(), mine.name_en.as_str()),
            ("custom-1", "#7FD1AE", "Verde menta")
        );
        s.set_owned("10300", true).unwrap();
        s.inventory_only = true;
        s.save().unwrap();

        let again = FilamentStore::load(path.clone());
        let ids: Vec<String> = again.inventory().unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["10300".to_string(), "custom-1".to_string()]);
        assert!(s.upsert(own("", "#000000")).is_err());
        assert!(s.upsert(own("Bad", "green")).is_err());
        assert!(s.set_owned("nope", true).is_err());
        s.remove("custom-1");
        assert!(!s.owned.contains("custom-1"));
        let _ = std::fs::remove_file(path);
    }
}
