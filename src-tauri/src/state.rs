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
    pub import_warnings: Vec<bp_blend::ImportWarning>,
    /// A copy of the store's pick, kept in step by the printer commands.
    pub printer: Printer,
    /// Flush volumes the user typed for a swap, by `#RRGGBB` of the old and the
    /// new colour, so they follow the colours when slots move.
    pub flush: HashMap<(String, String), f32>,
    /// Where the model's centre sits from the plate's centre, in millimetres.
    pub offset_mm: [f32; 2],
    /// The filament picked for a slot, by the slot's colour (`#RRGGBB`), so it
    /// follows the colour when slots move.
    pub filaments: HashMap<String, bp_3mf::filament::Filament>,
    /// How the colour kit was last cut, to offer the same next time.
    pub kit: Option<bp_3mf::project_file::KitSettings>,
}

impl Document {
    /// Paints the model with the `count` owned spools closest to its colours;
    /// each slot takes its spool's colour and the spool itself.
    pub fn suggest_from_inventory(&mut self, owned: &[bp_3mf::filament::Filament], count: usize) {
        let colors: Vec<_> = owned.iter().map(|f| bp_core::color::parse_hex(&f.hex).unwrap_or([0.5; 3])).collect();
        let picks = self.project.suggest_from(&colors, count);
        self.project.set_palette(picks.iter().map(|&i| colors[i]).collect());
        self.filaments = picks.iter().map(|&i| (bp_core::color::to_hex(colors[i]), owned[i].clone())).collect();
    }

    /// The filament on each slot, if one was picked.
    pub fn slot_filaments(&self) -> Vec<Option<&bp_3mf::filament::Filament>> {
        self.project.palette().iter().map(|c| self.filaments.get(&bp_core::color::to_hex(*c))).collect()
    }

    /// The colour each slot prints: its filament's, or the colour found in the model.
    pub fn slot_colors(&self) -> Vec<bp_core::color::Rgb> {
        let palette = self.project.palette();
        self.slot_filaments()
            .iter()
            .zip(palette)
            .map(|(f, &c)| f.and_then(|f| bp_core::color::parse_hex(&f.hex)).unwrap_or(c))
            .collect()
    }
}

#[derive(Default)]
pub struct AppState {
    pub doc: Mutex<Option<Document>>,
    pub blender: Mutex<Option<PathBuf>>,
    /// Printer profiles and the current pick; it outlives the open file.
    pub printers: Mutex<Store>,
    /// Waste and plate settings; they outlive the open file too.
    pub prefs: Mutex<Prefs>,
    /// The user's filaments and inventory.
    pub filaments: Mutex<crate::filaments::FilamentStore>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bp_core::{ColorSource, Part, Scene};

    #[test]
    fn inventory_colours_come_only_from_owned_spools() {
        // Red, blue and a little white, in bands.
        let mut positions = vec![];
        for z in 0..=3 {
            positions.push([0.0, 0.0, z as f32 * 10.0]);
            positions.push([10.0, 0.0, z as f32 * 10.0]);
        }
        let tris: Vec<[u32; 3]> =
            (0..3u32).flat_map(|q| [[2 * q, 2 * q + 1, 2 * q + 3], [2 * q, 2 * q + 3, 2 * q + 2]]).collect();
        let part = Part {
            name: "bands".into(),
            positions,
            tri_loops: (0..tris.len() as u32).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect(),
            tris,
            tri_source: vec![0, 0, 1, 1, 2, 2],
            sources: vec![
                ColorSource::Flat([0.8, 0.1, 0.1]),
                ColorSource::Flat([0.1, 0.12, 0.4]),
                ColorSource::Flat([0.95, 0.95, 0.95]),
            ],
            uv_layers: vec![],
            color_layers: vec![],
            object: 0,
        };
        let mut d = Document {
            project: Project::new(Scene { parts: vec![part], ..Default::default() }, 3),
            file_name: "bands.blend".into(),
            file_size: 0,
            import_warnings: vec![],
            printer: Printer::default(),
            flush: HashMap::new(),
            offset_mm: [0.0; 2],
            filaments: HashMap::new(),
            kit: None,
        };
        let catalog = bp_3mf::filament::catalog();
        let owned: Vec<_> = catalog.iter().filter(|f| f.kind == "PLA Basic").cloned().collect();
        d.suggest_from_inventory(&owned, 3);
        let hexes: Vec<String> = owned.iter().map(|f| f.hex.clone()).collect();
        let shown: Vec<String> = d.slot_colors().iter().map(|c| bp_core::color::to_hex(*c)).collect();
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|h| hexes.contains(h)), "{shown:?}");
        assert!(d.slot_filaments().iter().all(Option::is_some));
        // The red band went to Bambu's red.
        assert!(d.slot_filaments().iter().any(|f| f.is_some_and(|f| f.name_en == "Red")), "{shown:?}");
    }
}
