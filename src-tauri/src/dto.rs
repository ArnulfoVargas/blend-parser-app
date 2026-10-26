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
    pub object: u32,
    pub preview_start: usize,
    pub preview_end: usize,
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
    /// Split across plates, each plate's check; empty on one plate (the fields above say it all).
    pub plates: Vec<PlateGroupDto>,
}

/// One plate of a model split across several.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlateGroupDto {
    pub objects: usize,
    pub size_mm: [f32; 3],
    /// Centre of what is on it, in preview coordinates: the plate's centre (with the offset) goes there.
    pub centre: [f32; 3],
    pub fits: bool,
    pub fits_height: bool,
    #[serde(flatten)]
    pub check: bp_3mf::PlateCheck,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    /// Global ids of the triangles that changed.
    pub tris: Vec<u32>,
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
    let n = pal.len();
    let mut volumes = Vec::with_capacity(n * n);
    let mut set = Vec::with_capacity(n * n);
    for &a in pal {
        for &b in pal {
            let key = (bp_core::color::to_hex(a), bp_core::color::to_hex(b));
            let typed = doc.flush.get(&key).copied().filter(|_| a != b);
            volumes.push(typed.unwrap_or_else(|| bp_core::waste::default_flush(a, b)));
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
    // Split across plates, every plate is checked on its own and the tightest speaks for all.
    let groups = doc.project.plate_groups();
    let plates: Vec<PlateGroupDto> = if groups.len() > 1 {
        groups
            .iter()
            .map(|g| {
                let check =
                    bp_3mf::plate::check_at(&doc.printer, [g.size_mm[0], g.size_mm[1]], doc.offset_mm, &reserve);
                PlateGroupDto {
                    objects: g.objects,
                    size_mm: g.size_mm,
                    centre: g.centre,
                    fits: check.fits(),
                    fits_height: g.size_mm[2] <= doc.printer.max_height_mm(),
                    check,
                }
            })
            .collect()
    } else {
        vec![]
    };
    let check = plates
        .iter()
        .min_by(|a, b| a.check.fit_factor.total_cmp(&b.check.fit_factor))
        .map(|p| p.check.clone())
        .unwrap_or_else(|| bp_3mf::plate::check_at(&doc.printer, [size[0], size[1]], doc.offset_mm, &reserve));
    let plate = PlateDto {
        fits: if plates.is_empty() { check.fits() } else { plates.iter().all(|p| p.fits) },
        plates,
        check,
        prefs: prefs.plate,
        needs_tower,
        tower_mm,
        tower_estimated: prefs.plate.tower_depth_mm.is_none(),
        offset_mm: doc.offset_mm,
        rotated: doc.project.is_rotated(),
        can_undo_cut: doc.project.can_undo_cut(),
        objects: doc.project.object_count(),
    };
    (waste, plate)
}

/// Width, depth and height to check against the printer: the model's, or split
/// across plates, the largest of any plate's.
pub fn printed_size(p: &Project, stats: &bp_core::Stats) -> [f32; 3] {
    let groups = p.plate_groups();
    if groups.len() < 2 {
        return stats.size_mm;
    }
    groups.iter().fold([0.0; 3], |m, g| std::array::from_fn(|i| m[i].max(g.size_mm[i])))
}

pub fn summary(doc: &Document, prefs: &Prefs) -> Summary {
    let p: &Project = &doc.project;
    let mut stats = p.stats();
    stats.size_mm = printed_size(p, &stats);
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
        parts: p
            .scene()
            .parts
            .iter()
            .zip(p.preview_part_ranges())
            .map(|(x, (start, end))| PartDto {
                name: x.name.clone(),
                triangles: x.tris.len(),
                object: x.object,
                preview_start: start,
                preview_end: end,
            })
            .collect(),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceDto {
    pub part: usize,
    pub part_name: String,
    pub object: u32,
    pub triangles: usize,
    pub size_mm: [f32; 3],
    /// Box centre in preview coordinates.
    pub centre: [f32; 3],
    pub closed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiecesDto {
    pub pieces: Vec<PieceDto>,
    /// For each preview triangle, the index of its piece (`u32::MAX` for none).
    pub tri_piece: Vec<u32>,
}

pub fn pieces(p: &Project, weld_mm: f32) -> PiecesDto {
    let (found, tri_piece) = p.detect_pieces(weld_mm);
    let parts = &p.scene().parts;
    PiecesDto {
        pieces: found
            .into_iter()
            .map(|x| PieceDto {
                part_name: parts[x.part].name.clone(),
                object: parts[x.part].object,
                part: x.part,
                triangles: x.triangles,
                size_mm: x.size_mm,
                centre: x.centre,
                closed: x.closed,
            })
            .collect(),
        tri_piece,
    }
}

/// Why a cut is proposed, in final millimetres.
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SignalDto {
    Neck { area_mm2: f32, before_mm2: f32, after_mm2: f32 },
    Colour { from: u8, to: u8 },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalDto {
    pub part: usize,
    pub axis: usize,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub signal: SignalDto,
}

pub fn proposal(p: bp_core::CutProposal) -> ProposalDto {
    ProposalDto {
        part: p.part,
        axis: p.axis,
        point: p.point,
        normal: p.normal,
        signal: match p.signal {
            bp_core::separate::Signal::Neck { area, before, after } => {
                SignalDto::Neck { area_mm2: area, before_mm2: before, after_mm2: after }
            }
            bp_core::separate::Signal::Colour { from, to } => SignalDto::Colour { from, to },
        },
    }
}

/// A dry run of a cut with connectors.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectReviewDto {
    /// Per connector: "outside", "overlap", "thinWall", "tooDeep", or null when fine.
    pub issues: Vec<Option<&'static str>>,
    /// Objects after the cut, when every connector is fine and the cut closes.
    pub objects: Option<usize>,
}

/// Cuts that make every piece fit, and what they were worked out for.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitProposalDto {
    /// Positions along X, Y and Z in preview coordinates.
    pub cuts: [Vec<f32>; 3],
    /// Room on each plate for pieces: width and depth.
    pub room_mm: [f32; 2],
    pub height_mm: f32,
    /// The model as it prints now.
    pub size_mm: [f32; 3],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPieceDto {
    pub name: String,
    pub size_mm: [f32; 3],
    /// 0-based; null when it fits no plate.
    pub plate: Option<usize>,
    pub laid: bool,
    pub small: bool,
    pub dowel: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitReportDto {
    pub pieces: Vec<SplitPieceDto>,
    pub plates: usize,
    pub cuts: usize,
    pub joints: usize,
    pub joints_missing: usize,
    pub fits: bool,
}

pub fn split_report(r: bp_core::SplitReport) -> SplitReportDto {
    SplitReportDto {
        fits: r.fits(),
        plates: r.plates,
        cuts: r.cuts,
        joints: r.joints,
        joints_missing: r.joints_missing,
        pieces: r
            .pieces
            .into_iter()
            .map(|p| SplitPieceDto {
                name: p.name,
                size_mm: p.size_mm,
                plate: p.plate,
                laid: p.laid,
                small: p.small,
                dowel: p.dowel,
            })
            .collect(),
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
