//! What the UI receives. Everything the panels show comes in one `Summary`
//! after each change, so the UI never has to keep its own copy in sync.

use bp_core::{ColorOps, MeshOps, Project};
use serde::Serialize;

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
    pub hex: String,
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
    pub purge_grams: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub height_mm: f32,
    pub keep_ratio: f32,
    pub flatten_mm: f32,
    pub repair: bool,
    pub min_piece_mm: f32,
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
    pub can_undo: bool,
    /// What the next undo reverts ("trazo", "unión de colores").
    pub undo_label: Option<String>,
    /// Triangles painted by hand; a new colour detection discards them.
    pub painted_triangles: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    /// Global ids of the triangles that changed.
    pub tris: Vec<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseDto {
    pub id: &'static str,
    pub name: &'static str,
}

/// Every printer to pick from, the current pick, and what a custom one can use.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintersDto {
    pub profiles: Vec<bp_3mf::PrinterProfile>,
    pub selected: String,
    pub nozzle_mm: f32,
    pub nozzles: Vec<f32>,
    pub bases: Vec<BaseDto>,
}

pub fn printers(store: &crate::profiles::Store) -> PrintersDto {
    let p = store.printer();
    PrintersDto {
        profiles: store.profiles(),
        selected: p.profile.id,
        nozzle_mm: p.nozzle_mm,
        nozzles: bp_3mf::NOZZLES.to_vec(),
        bases: bp_3mf::bambu_bases().into_iter().map(|(id, name)| BaseDto { id, name }).collect(),
    }
}

pub fn summary(doc: &crate::state::Document) -> Summary {
    let p: &Project = &doc.project;
    let stats = p.stats();
    let MeshOps { keep_ratio, flatten_mm, repair, min_piece_mm } = p.mesh_ops();
    let ColorOps { border_detail, min_island_mm2 } = p.color_ops();
    let bed = doc.printer.bed_mm();
    let min_detail = doc.printer.min_detail_mm2();
    Summary {
        file_name: doc.file_name.clone(),
        file_size: doc.file_size,
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
            .zip(&stats.per_slot)
            .zip(&stats.nearest)
            .map(|((c, &n), &nearest)| SlotDto { hex: bp_core::color::to_hex(*c), triangles: n, nearest })
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
            purge_grams: stats.swaps.purge_grams,
        },
        settings: SettingsDto {
            height_mm: p.height_mm(),
            keep_ratio,
            flatten_mm,
            repair,
            min_piece_mm,
            border_detail,
            min_island_mm2,
            printer: doc.printer.profile.id.clone(),
            nozzle_mm: doc.printer.nozzle_mm,
            layer_mm: doc.printer.layer_mm(),
        },
        can_undo: p.can_undo(),
        undo_label: p.undo_label().map(str::to_string),
        painted_triangles: p.painted_count(),
    }
}
