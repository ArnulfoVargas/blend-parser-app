//! Application state: the open document, the printer it is prepared for and
//! where Blender lives.

use std::path::PathBuf;
use std::sync::Mutex;

use bp_3mf::Printer;
use bp_core::Project;

use crate::profiles::Store;

pub struct Document {
    pub project: Project,
    pub file_name: String,
    pub file_size: u64,
    /// A copy of the store's pick, kept in step by the printer commands.
    pub printer: Printer,
}

#[derive(Default)]
pub struct AppState {
    pub doc: Mutex<Option<Document>>,
    pub blender: Mutex<Option<PathBuf>>,
    /// Printer profiles and the current pick; it outlives the open file.
    pub printers: Mutex<Store>,
}
