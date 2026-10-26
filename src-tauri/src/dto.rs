//! What the UI receives. Everything the panels show comes in one `Summary`
//! after each change, so the UI never has to keep its own copy in sync.

use bp_core::{ColorOps, MeshOps, Project, Waste, WasteParams};
use serde::Serialize;

use crate::prefs::{PlatePrefs, Prefs, WastePrefs};
use crate::state::Document;

/// Prime tower width and brim the 3MF asks Bambu Studio for.
pub const TOWER_WIDTH_MM: f32 = 20.0;
pub const TOWER_BRIM_MM: f32 = 3.0;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDto {
    pub name: String,
    pub triangles: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialDto {
    pub name: String,
    pub kind: String,
    pub detail: String,
    pub faces: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDto {
    /// The colour it prints: its filament's, or the one found in the model.
    pub hex: String,
    /// The colour found in the model, which decides what the slot paints.
    pub detected_hex: String,
    /// The filament picked for it, and its ΔE from the colour found.
    pub filament: Option<bp_3mf::filament::Filament>,
    pub filament_delta_e: Option<f32>,
    pub triangles: usize,
    /// Closest other slot (0-based) and the ΔE between them.
    pub nearest: Option<(usize, f32)>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairDto {
    pub welded_vertices: usize,
    pub degenerate_triangles: usize,
    pub duplicate_triangles: usize,
    pub loose_pieces: usize,
    pub loose_triangles: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateDto {
    pub layers: u32,
    pub multicolor_layers: u32,
    pub swaps: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeDto {
    /// As the file has it.
    pub original_mm: [f32; 3],
    /// As the user asked for it, before flattening.
    pub target_mm: [f32; 3],
    /// Against the original, per axis.
    pub axis_scale: [f32; 3],
    /// Preview units to millimetres (the preview carries the per-axis stretch already).
    pub preview_scale: f32,
    /// Per-axis stretch in the preview positions, to keep measured points on the model.
    pub stretch: [f32; 3],
    pub can_undo: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WasteDto {
    /// Swaps in the order that flushes least.
    pub swaps: u32,
    pub flush_mm3: f32,
    pub reused_mm3: f32,
    pub start_mm3: f32,
    pub tower_mm3: f32,
    pub total_mm3: f32,
    pub total_g: f32,
    pub total_m: f32,
    pub tower_height_mm: f32,
    /// What it was worked out with.
    pub prefs: WastePrefs,
    pub layer_mm: f32,
    pub nozzle_mm: f32,
    /// The printer flushes into a chute rather than into the tower.
    pub purge_chute: bool,
    /// Flush per swap from slot `i` to `j` at `i * n + j`, before the multiplier.
    pub flush_mm3_matrix: Vec<f32>,
    /// Which of those the user typed.
    pub flush_set: Vec<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlateDto {
    #[serde(flatten)]
    pub check: bp_3mf::PlateCheck,
    pub fits: bool,
    pub prefs: PlatePrefs,
    /// A tower is printed: more than one colour.
    pub needs_tower: bool,
    /// Tower footprint, brim included, and whether its depth is the estimate.
    pub tower_mm: [f32; 2],
    pub tower_estimated: bool,
    /// Where the model's centre sits from the plate's centre.
    pub offset_mm: [f32; 2],
    /// The model is turned from the file's orientation.
    pub rotated: bool,
    /// The last cut can be undone.
    pub can_undo_cut: bool,
    /// Printed objects: the model and the pieces cut off it that were kept.
    pub objects: usize,
    /// Their ids, which number their names in the 3MF (see `bp_3mf::object_name`).
    pub object_ids: Vec<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub height_mm: f32,
    pub keep_ratio: f32,
    pub flatten_mm: f32,
    pub repair: bool,
    pub min_piece_mm: f32,
    pub orient: bool,
    pub fill_holes: bool,
    pub split_vertices: bool,
    pub deep_clean: bool,
    pub drop_floating: bool,
    /// Rows of the rotation applied to the model as loaded.
    pub rotation: bp_core::Mat3,
    pub border_detail: u8,
    pub min_island_mm2: f32,
    pub printer: String,
    pub nozzle_mm: f32,
    pub layer_mm: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub file_name: String,
    pub file_size: u64,
    pub import_warnings: Vec<bp_blend::ImportWarning>,
    pub parts: Vec<PartDto>,
    pub materials: Vec<MaterialDto>,
    pub slots: Vec<SlotDto>,
    pub source_triangles: usize,
    pub triangles: usize,
    pub split_triangles: usize,
    pub size_mm: [f32; 3],
    pub bed_mm: [f32; 2],
    pub fits_bed: bool,
    pub max_height_mm: f32,
    pub fits_height: bool,
    /// Colour patches under `min_detail_mm2`, too small for the nozzle.
    pub small_islands: usize,
    pub min_detail_mm2: f32,
    pub contact_mm2: f32,
    pub repair: RepairDto,
    pub estimate: EstimateDto,
    pub settings: SettingsDto,
    pub size: SizeDto,
    pub waste: WasteDto,
    pub plate: PlateDto,
    pub can_undo: bool,
    /// What the next undo reverts: "stroke" or "merge".
    pub undo_label: Option<String>,
    /// Triangles painted by hand; a new colour detection discards them.
    pub painted_triangles: usize,
}

/// A turn: the new summary and, when the preview could follow as it is,
/// `p' = matrix · p + shift` for its positions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDto {
    pub summary: Summary,
    pub rigid: Option<RigidDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RigidDto {
    pub matrix: [[f32; 3]; 3],
    pub shift: [f32; 3],
}

pub fn turn(doc: &Document, prefs: &Prefs, rigid: Option<bp_core::project::RigidMove>) -> TurnDto {
    TurnDto { summary: summary(doc, prefs), rigid: rigid.map(|m| RigidDto { matrix: m.matrix, shift: m.shift }) }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

/// What the bucket would fill, to show before it does.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketPickDto {
    /// The colour target it found, `#RRGGBB`; the clicked colour itself when it is added.
    pub target_hex: String,
    /// Whether that colour is a new target rather than one the palette has.
    pub added: bool,
    /// The slot that colour paints now; `None` for an added colour.
    pub slot: Option<u8>,
    /// The model's colour under the click and its ΔE to the target.
    pub sample_hex: String,
    pub delta_e: f32,
    /// Global ids of the triangles it reaches.
    pub tris: Vec<u32>,
}

impl From<bp_core::project::BucketPick> for BucketPickDto {
    fn from(p: bp_core::project::BucketPick) -> Self {
        Self {
            target_hex: bp_core::color::to_hex(p.target_rgb),
            added: p.target.is_none(),
            slot: p.target.map(|_| p.slot),
            sample_hex: bp_core::color::to_hex(p.sample),
            delta_e: p.delta_e,
            tris: p.tris,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    /// Global ids of the triangles that changed.
    pub tris: Vec<u32>,
}

/// Every filament to pick from, the user's inventory, and whether colours come only from it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilamentsDto {
    pub filaments: Vec<bp_3mf::filament::Filament>,
    pub owned: Vec<String>,
    pub inventory_only: bool,
}

pub fn filaments(store: &crate::filaments::FilamentStore) -> FilamentsDto {
    FilamentsDto {
        filaments: store.all(),
        owned: store.owned.iter().cloned().collect(),
        inventory_only: store.inventory_only,
    }
}

/// Every printer to pick from, the current pick, and what a custom one can use.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintersDto {
    pub profiles: Vec<bp_3mf::PrinterProfile>,
    pub selected: String,
    pub nozzle_mm: f32,
    /// Nozzles a custom printer can take.
    pub nozzles: Vec<f32>,
}

pub fn printers(store: &crate::profiles::Store) -> PrintersDto {
    let p = store.printer();
    PrintersDto {
        profiles: store.profiles(),
        selected: p.profile.id,
        nozzle_mm: p.nozzle_mm,
        nozzles: bp_3mf::NOZZLES.to_vec(),
    }
}

/// The layer height the estimate and the export use.
pub fn layer_mm(printer: &bp_3mf::Printer, prefs: &WastePrefs) -> f32 {
    prefs.layer_mm.unwrap_or_else(|| printer.layer_mm())
}

/// Flush per swap between every pair of slots, the user's values first.
pub fn flush_matrix(doc: &Document) -> (Vec<f32>, Vec<bool>) {
    let pal = doc.project.palette();
    // Typed flush follows the slots' colours; the estimate, the colours they print.
    let shown = doc.slot_colors();
    let n = pal.len();
    let mut volumes = Vec::with_capacity(n * n);
    let mut set = Vec::with_capacity(n * n);
    for (&a, &sa) in pal.iter().zip(&shown) {
        for (&b, &sb) in pal.iter().zip(&shown) {
            let key = (bp_core::color::to_hex(a), bp_core::color::to_hex(b));
            let typed = doc.flush.get(&key).copied().filter(|_| a != b);
            volumes.push(typed.unwrap_or_else(|| bp_core::waste::default_flush(sa, sb)));
            set.push(typed.is_some());
        }
    }
    (volumes, set)
}

pub fn waste_params(doc: &Document, prefs: &Prefs) -> WasteParams {
    let w = &prefs.waste;
    WasteParams {
        layer_mm: layer_mm(&doc.printer, w),
        nozzle_mm: doc.printer.nozzle_mm,
        filament_diameter_mm: w.filament_diameter_mm,
        density_g_cm3: w.density_g_cm3,
        flush_multiplier: w.flush_multiplier,
        start_purge_mm3: w.start_purge_mm3,
        prime_mm3: w.prime_mm3,
        flush_reuse: w.flush_reuse,
        tower: doc.project.palette().len() > 1,
        tower_width_mm: TOWER_WIDTH_MM,
        tower_depth_mm: prefs.plate.tower_depth_mm,
        tower_brim_mm: TOWER_BRIM_MM,
        purge_chute: doc.printer.purges_into_chute(),
    }
}

/// The waste estimate and the plate check, which takes the tower's room from it.
pub fn waste_and_plate(
    doc: &Document,
    prefs: &Prefs,
    swaps: &bp_core::estimate::SwapEstimate,
    size: [f32; 3],
) -> (Waste, PlateDto) {
    let (matrix, _) = flush_matrix(doc);
    let n = doc.project.palette().len();
    let params = waste_params(doc, prefs);
    let waste = bp_core::waste::waste(swaps, &params, |a, b| matrix[a as usize * n + b as usize]);
    let needs_tower = params.tower;
    // With no swap yet the tower still has to be planned: as deep as it is wide.
    let tower_mm = match (prefs.plate.tower_depth_mm, waste.tower_footprint_mm) {
        (Some(d), _) => [TOWER_WIDTH_MM + 2.0 * TOWER_BRIM_MM, d + 2.0 * TOWER_BRIM_MM],
        (None, f) if f[1] > 0.0 => f,
        _ => [TOWER_WIDTH_MM + 2.0 * TOWER_BRIM_MM; 2],
    };
    let reserve = bp_3mf::Reserve {
        tower: prefs.plate.tower && needs_tower,
        tower_mm,
        tower_gap_mm: prefs.plate.tower_gap_mm,
        margin_mm: prefs.plate.margin_mm,
    };
    let check = bp_3mf::plate::check_at(&doc.printer, [size[0], size[1]], doc.offset_mm, &reserve);
    let plate = PlateDto {
        fits: check.fits(),
        check,
        prefs: prefs.plate,
        needs_tower,
        tower_mm,
        tower_estimated: prefs.plate.tower_depth_mm.is_none(),
        offset_mm: doc.offset_mm,
        rotated: doc.project.is_rotated(),
        can_undo_cut: doc.project.can_undo_cut(),
        objects: doc.project.object_count(),
        object_ids: doc.project.object_ids(),
    };
    (waste, plate)
}

pub fn summary(doc: &Document, prefs: &Prefs) -> Summary {
    let p: &Project = &doc.project;
    let stats = p.stats();
    let MeshOps {
        keep_ratio,
        flatten_mm,
        repair,
        min_piece_mm,
        orient,
        fill_holes,
        split_vertices,
        drop_floating,
        deep_clean,
    } = p.mesh_ops();
    let ColorOps { border_detail, min_island_mm2 } = p.color_ops();
    let bed = doc.printer.bed_mm();
    let min_detail = doc.printer.min_detail_mm2();
    let (waste, plate) = waste_and_plate(doc, prefs, &stats.swaps, stats.size_mm);
    let (flush_mm3_matrix, flush_set) = flush_matrix(doc);
    Summary {
        file_name: doc.file_name.clone(),
        file_size: doc.file_size,
        import_warnings: doc.import_warnings.clone(),
        parts: p.scene().parts.iter().map(|x| PartDto { name: x.name.clone(), triangles: x.tris.len() }).collect(),
        materials: p
            .scene()
            .materials
            .iter()
            .map(|m| MaterialDto {
                name: m.name.clone(),
                kind: m.kind.clone(),
                detail: m.detail.clone(),
                faces: m.faces,
            })
            .collect(),
        slots: p
            .palette()
            .iter()
            .zip(doc.slot_colors())
            .zip(doc.slot_filaments())
            .zip(&stats.per_slot)
            .zip(&stats.nearest)
            .map(|((((c, shown), f), &n), &nearest)| SlotDto {
                hex: bp_core::color::to_hex(shown),
                detected_hex: bp_core::color::to_hex(*c),
                filament: f.cloned(),
                filament_delta_e: f.map(|_| bp_core::palette::delta_e(*c, shown)),
                triangles: n,
                nearest,
            })
            .collect(),
        source_triangles: stats.source_triangles,
        triangles: stats.triangles,
        split_triangles: stats.split_triangles,
        size_mm: stats.size_mm,
        bed_mm: [bed.0, bed.1],
        fits_bed: stats.size_mm[0] <= bed.0 && stats.size_mm[1] <= bed.1,
        max_height_mm: doc.printer.max_height_mm(),
        fits_height: stats.size_mm[2] <= doc.printer.max_height_mm(),
        small_islands: p.small_islands(min_detail),
        min_detail_mm2: min_detail,
        contact_mm2: stats.contact_mm2,
        repair: RepairDto {
            welded_vertices: stats.repair.welded_vertices,
            degenerate_triangles: stats.repair.degenerate_triangles,
            duplicate_triangles: stats.repair.duplicate_triangles,
            loose_pieces: stats.repair.loose_pieces,
            loose_triangles: stats.repair.loose_triangles,
        },
        estimate: EstimateDto {
            layers: stats.swaps.layers,
            multicolor_layers: stats.swaps.multicolor_layers,
            swaps: stats.swaps.swaps,
        },
        settings: SettingsDto {
            height_mm: p.height_mm(),
            keep_ratio,
            flatten_mm,
            repair,
            min_piece_mm,
            orient,
            fill_holes,
            split_vertices,
            deep_clean,
            drop_floating,
            rotation: p.rotation(),
            border_detail,
            min_island_mm2,
            printer: doc.printer.profile.id.clone(),
            nozzle_mm: doc.printer.nozzle_mm,
            layer_mm: layer_mm(&doc.printer, &prefs.waste),
        },
        size: SizeDto {
            original_mm: p.original_size_mm(),
            target_mm: p.size_mm(),
            axis_scale: p.axis_scale(),
            preview_scale: p.scale(),
            stretch: p.stretch(),
            can_undo: p.can_undo_size(),
        },
        waste: WasteDto {
            swaps: waste.swaps,
            flush_mm3: waste.flush_mm3,
            reused_mm3: waste.reused_mm3,
            start_mm3: waste.start_mm3,
            tower_mm3: waste.tower_mm3,
            total_mm3: waste.total_mm3,
            total_g: waste.total_g,
            total_m: waste.total_m,
            tower_height_mm: waste.tower_height_mm,
            prefs: prefs.waste,
            layer_mm: layer_mm(&doc.printer, &prefs.waste),
            nozzle_mm: doc.printer.nozzle_mm,
            purge_chute: doc.printer.purges_into_chute(),
            flush_mm3_matrix,
            flush_set,
        },
        plate,
        can_undo: p.can_undo(),
        undo_label: p.undo_label().map(str::to_string),
        painted_triangles: p.painted_count(),
    }
}

/// The saved print settings and the defaults "Restablecer" goes back to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsDto {
    pub waste: WastePrefs,
    pub plate: PlatePrefs,
    pub waste_defaults: WastePrefs,
    pub plate_defaults: PlatePrefs,
}

pub fn prefs(p: &Prefs) -> PrefsDto {
    PrefsDto {
        waste: p.waste,
        plate: p.plate,
        waste_defaults: WastePrefs::default(),
        plate_defaults: PlatePrefs::default(),
    }
}

/// A step of opening a file, as a key the UI translates and the name it mentions.
#[derive(Clone, Serialize)]
pub struct ProgressDto {
    pub key: String,
    pub name: String,
}
