//! Saving the open document as a `.bpproj` and opening one: the core's bytes
//! for the model and its painting, and the document's own settings as JSON.

use std::path::Path;

use bp_3mf::{Printer, PrinterProfile};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, CmdResult};
use crate::profiles::Store;
use crate::state::Document;

/// The `.bpproj` layout this build writes; a newer one is refused.
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    format: u32,
    /// The BlendParser that saved it, for reference.
    app_version: String,
    file_name: String,
    file_size: u64,
    import_warnings: Vec<bp_blend::ImportWarning>,
    /// The whole profile, so a custom printer that has since gone still opens.
    printer: PrinterProfile,
    nozzle_mm: f32,
    /// Flush the user typed per swap: old colour, new colour, mm³.
    flush: Vec<(String, String, f32)>,
    offset_mm: [f32; 2],
}

pub fn save(doc: &Document, path: &str) -> CmdResult<()> {
    let settings = Settings {
        format: FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        file_name: doc.file_name.clone(),
        file_size: doc.file_size,
        import_warnings: doc.import_warnings.clone(),
        printer: doc.printer.profile.clone(),
        nozzle_mm: doc.printer.nozzle_mm,
        flush: doc.flush.iter().map(|((a, b), v)| (a.clone(), b.clone(), *v)).collect(),
        offset_mm: doc.offset_mm,
    };
    let json = serde_json::to_string_pretty(&settings).map_err(AppError::internal)?;
    let model = doc.project.to_bytes();
    // Written next to it first, so a failed save never leaves half a project.
    let tmp = format!("{path}.tmp");
    let write = || -> std::io::Result<()> {
        let file = std::fs::File::create(&tmp)?;
        bp_3mf::project_file::write(std::io::BufWriter::new(file), &json, &model).map_err(std::io::Error::other)?;
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
    let (json, model) = bp_3mf::project_file::read(std::io::BufReader::new(file))
        .map_err(|_| AppError::with("error.projectCorrupt", json!({ "path": path })))?;
    let format = serde_json::from_str::<serde_json::Value>(&json).ok().and_then(|v| v["format"].as_u64());
    if format.is_some_and(|f| f > FORMAT as u64) {
        return Err(AppError::with("error.projectVersion", json!({ "path": path })));
    }
    let s: Settings =
        serde_json::from_str(&json).map_err(|_| AppError::with("error.projectCorrupt", json!({ "path": path })))?;
    let project = bp_core::Project::from_bytes(&model).map_err(|e| match e {
        bp_core::SnapshotError::Version(_) => AppError::with("error.projectVersion", json!({ "path": path })),
        bp_core::SnapshotError::Corrupt => AppError::with("error.projectCorrupt", json!({ "path": path })),
    })?;
    let mut warnings = s.import_warnings;
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
    })
}

/// Whether `path` names a project rather than a model.
pub fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpproj"))
}

#[cfg(test)]
mod tests {
    use super::*;
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
