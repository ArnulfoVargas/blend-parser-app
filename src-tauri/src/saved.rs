//! Saving the open document as a `.bpproj` and opening one: the core's bytes
//! for the model and its painting, and the document's own settings as JSON.

use std::path::Path;

use bp_3mf::project_file::{self, ProjectError, Settings, Warning};
use bp_3mf::Printer;
use serde_json::json;

use crate::error::{AppError, CmdResult};
use crate::profiles::Store;
use crate::state::Document;

pub fn save(doc: &Document, path: &str) -> CmdResult<()> {
    let settings = Settings {
        format: project_file::FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        file_name: doc.file_name.clone(),
        file_size: doc.file_size,
        import_warnings: doc
            .import_warnings
            .iter()
            .map(|w| Warning { code: w.code.clone(), detail: w.detail.clone() })
            .collect(),
        printer: doc.printer.profile.clone(),
        nozzle_mm: doc.printer.nozzle_mm,
        flush: doc.flush.iter().map(|((a, b), v)| (a.clone(), b.clone(), *v)).collect(),
        offset_mm: doc.offset_mm,
        filaments: doc
            .project
            .palette()
            .iter()
            .filter_map(|c| {
                let hex = bp_core::color::to_hex(*c);
                doc.filaments.get(&hex).map(|f| (hex, f.clone()))
            })
            .collect(),
        kit: doc.kit,
    };
    let json = serde_json::to_string_pretty(&settings).map_err(AppError::internal)?;
    let model = doc.project.to_bytes();
    // Written next to it first, so a failed save never leaves half a project.
    let tmp = format!("{path}.tmp");
    let write = || -> std::io::Result<()> {
        let file = std::fs::File::create(&tmp)?;
        project_file::write(std::io::BufWriter::new(file), &json, &model).map_err(std::io::Error::other)?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::file(path, e)
    })
}

/// Opens a project for `store`'s printers. A printer that is no longer among
/// them is kept from the file and named in the import warnings.
pub fn open(path: &str, store: &Store) -> CmdResult<Document> {
    let file = std::fs::File::open(path).map_err(|e| AppError::file(path, e))?;
    let (json, model) = project_file::read(std::io::BufReader::new(file))
        .map_err(|_| AppError::with("error.projectCorrupt", json!({ "path": path })))?;
    let s = project_file::parse_settings(&json).map_err(|e| match e {
        ProjectError::Version => AppError::with("error.projectVersion", json!({ "path": path })),
        ProjectError::Corrupt => AppError::with("error.projectCorrupt", json!({ "path": path })),
    })?;
    let project = bp_core::Project::from_bytes(&model).map_err(|e| match e {
        bp_core::SnapshotError::Version(_) => AppError::with("error.projectVersion", json!({ "path": path })),
        bp_core::SnapshotError::Corrupt => AppError::with("error.projectCorrupt", json!({ "path": path })),
    })?;
    let mut warnings: Vec<bp_blend::ImportWarning> =
        s.import_warnings.into_iter().map(|w| bp_blend::ImportWarning { code: w.code, detail: w.detail }).collect();
    let known = store.profiles().into_iter().find(|p| p.id == s.printer.id);
    if known.is_none() {
        warnings.push(bp_blend::ImportWarning { code: "projectPrinter".into(), detail: s.printer.name.clone() });
    }
    let printer = Printer::new(known.unwrap_or(s.printer), s.nozzle_mm);
    Ok(Document {
        project,
        file_name: s.file_name,
        file_size: s.file_size,
        import_warnings: warnings,
        printer,
        flush: s.flush.into_iter().map(|(a, b, v)| ((a, b), v)).collect(),
        offset_mm: s.offset_mm,
        filaments: s.filaments.into_iter().collect(),
        kit: s.kit,
    })
}

/// Whether `path` names a project rather than a model.
pub fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpproj"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_3mf::PrinterProfile;
    use bp_core::{ColorSource, Part, Scene};

    fn doc(printer: Printer) -> Document {
        let part = Part {
            name: "q".into(),
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 0.0, 10.0]],
            tris: vec![[0, 1, 2], [0, 2, 3]],
            tri_loops: vec![[0, 1, 2], [3, 4, 5]],
            tri_source: vec![0, 1],
            sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
            uv_layers: vec![],
            color_layers: vec![],
            object: 0,
        };
        Document {
            project: bp_core::Project::new(Scene { parts: vec![part], ..Default::default() }, 2),
            file_name: "q.blend".into(),
            file_size: 1234,
            import_warnings: vec![],
            printer,
            flush: [(("#E61A1A".to_string(), "#1A1AE6".to_string()), 321.0)].into_iter().collect(),
            offset_mm: [12.0, -3.5],
            filaments: Default::default(),
            kit: Some(bp_3mf::project_file::KitSettings {
                voxel_mm: 0.3,
                depth_mm: 2.0,
                core_slot: Some(1),
                min_piece_mm3: 12.0,
                mix: true,
            }),
        }
    }

    #[test]
    fn a_project_opens_with_its_settings() {
        let dir = std::env::temp_dir().join(format!("bp-saved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("q.bpproj").display().to_string();
        let d = doc(Printer::default());
        save(&d, &path).unwrap();
        let o = open(&path, &Store::default()).unwrap();
        assert_eq!(o.file_name, "q.blend");
        assert_eq!(o.offset_mm, [12.0, -3.5]);
        assert_eq!(o.flush, d.flush);
        assert_eq!(o.kit, d.kit);
        assert_eq!(o.printer.profile.id, d.printer.profile.id);
        assert!(o.import_warnings.is_empty());
        assert_eq!(o.project.export_parts()[0].paint, d.project.export_parts()[0].paint);

        // A custom printer that is gone opens from the file, with a warning.
        let own = PrinterProfile {
            id: "custom-9".into(),
            vendor: String::new(),
            name: "Mine".into(),
            volume_mm: [200.0, 200.0, 200.0],
            nozzles: vec![0.4],
            base: None,
            custom: true,
        };
        save(&doc(Printer::new(own, 0.4)), &path).unwrap();
        let o = open(&path, &Store::default()).unwrap();
        assert_eq!(o.printer.profile.name, "Mine");
        assert_eq!(o.import_warnings[0].code, "projectPrinter");

        // Not a project, or a newer one: refused with its own error.
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(open(&path, &Store::default()).err().unwrap().key, "error.projectCorrupt");
        let mut buf = std::io::Cursor::new(Vec::new());
        bp_3mf::project_file::write(&mut buf, "{\"format\": 99}", &[]).unwrap();
        std::fs::write(&path, buf.into_inner()).unwrap();
        assert_eq!(open(&path, &Store::default()).err().unwrap().key, "error.projectVersion");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
