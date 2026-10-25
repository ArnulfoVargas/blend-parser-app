//! Application state: the open document and where Blender lives.

use std::path::PathBuf;
use std::sync::Mutex;

use bp_3mf::Printer;
use bp_core::Project;

pub struct Document {
    pub project: Project,
    pub file_name: String,
    pub file_size: u64,
    pub printer: Printer,
    pub printer_id: String,
}

#[derive(Default)]
pub struct AppState {
    pub doc: Mutex<Option<Document>>,
    pub blender: Mutex<Option<PathBuf>>,
}
