//! Application state: the open document, the printer it is prepared for, the
//! print settings and where Blender lives.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use bp_3mf::Printer;
use bp_core::Project;

use crate::prefs::Prefs;
use crate::profiles::Store;

pub struct Document {
    pub project: Project,
    pub file_name: String,
    pub file_size: u64,
    /// A copy of the store's pick, kept in step by the printer commands.
    pub printer: Printer,
    /// Flush volumes the user typed for a swap, by `#RRGGBB` of the old and the
    /// new colour, so they follow the colours when slots move.
    pub flush: HashMap<(String, String), f32>,
}

#[derive(Default)]
pub struct AppState {
    pub doc: Mutex<Option<Document>>,
    pub blender: Mutex<Option<PathBuf>>,
    /// Printer profiles and the current pick; it outlives the open file.
    pub printers: Mutex<Store>,
    /// Waste and plate settings; they outlive the open file too.
    pub prefs: Mutex<Prefs>,
}
