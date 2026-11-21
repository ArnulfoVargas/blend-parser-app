//! What the UI receives. Everything the panels show comes in one `Summary`
//! after each change, so the UI never has to keep its own copy in sync.

use bp_core::{ColorOps, MeshOps, Project, Waste, WasteParams};
use serde::{Deserialize, Serialize};

use crate::prefs::{PlatePrefs, Prefs, WastePrefs};
use crate::state::Document;

/// Prime tower brim the 3MF asks Bambu Studio for.
pub const TOWER_BRIM_MM: f32 = 3.0;

/// What the colour kit export produced.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitDto {
    pub pieces: usize,
    pub plates: usize,
    /// Pieces larger than the plate.
    pub oversize: usize,
    /// Pieces to look at before printing them, and regions too thin to make a piece.
    pub review: usize,
    pub dropped: usize,
    /// Pins keying pieces together, and joins with no room for one.
    pub pins: usize,
    pub unpinned: usize,
    /// Size of the ZIP written.
    pub bytes: u64,
}

/// How far the colour kit export has got: `split`, `plates`, `pictures` or
/// `files`, with how many of `total` are done (0 of 0 when it cannot tell).
#[derive(Clone, Serialize)]
pub struct KitProgressDto {
    pub stage: String,
    pub done: usize,
    pub total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDto {
    pub name: String,
    pub triangles: usize,
    pub object: u32,
    pub plate: u32,
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
    pub filled_holes: usize,
    pub flipped_triangles: usize,
    pub split_vertices: usize,
    pub dropped_pieces: usize,
    pub fin_triangles: usize,
}

/// What each advanced repair finds, as [`bp_core::MeshIssues`] counts it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuesDto {
    pub holes: usize,
    pub open_edges: usize,
    pub flipped_triangles: usize,
    pub shared_vertices: usize,
    pub floating_pieces: usize,
    pub stray_triangles: usize,
}

impl From<bp_core::MeshIssues> for IssuesDto {
    fn from(i: bp_core::MeshIssues) -> Self {
        Self {
            holes: i.holes,
            open_edges: i.open_edges,
            flipped_triangles: i.flipped_triangles,
            shared_vertices: i.shared_vertices,
            floating_pieces: i.floating_pieces,
            stray_triangles: i.stray_triangles,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThickenDto {
    pub moved_vertices: usize,
    pub max_offset_mm: f32,
    pub area_mm2: f32,
}

/// What hollowing did: volumes in mm³, the voxel (its accuracy) in mm, and
/// why it did nothing ("open", "noRoom") when it did not.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HollowDto {
    pub solid_mm3: f32,
    pub volume_mm3: f32,
    pub cell_mm: f32,
    pub drains: u8,
    pub drains_wanted: u8,
    pub refused: Option<&'static str>,
}

/// The best turn for printing and what it gains, as now and after.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrientationDto {
    /// Whether it differs from the current turn.
    pub changes: bool,
    pub rotation: bp_core::Mat3,
    /// Which way, in the preview as it is, will face the plate.
    pub down: [f32; 3],
    pub overhang_mm2: [f32; 2],
    pub contact_mm2: [f32; 2],
}

/// The mesh built without the advanced repairs, and as it is now.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshIssuesDto {
    pub before: IssuesDto,
    pub after: IssuesDto,
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
    pub protect_colors: bool,
    pub protect_detail: bool,
    pub subdivide: u8,
    pub smooth: u8,
    pub crease_deg: f32,
    pub hollow_mm: f32,
    pub drains: u8,
    pub drain_mm: f32,
    pub thicken_mm: f32,
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
    /// How far simplifying moved the surface at most, in mm; 0 unsimplified.
    pub deviation_mm: f32,
    /// The model's volume in mm³, and what hollowing did when it was asked for.
    pub volume_mm3: f32,
    pub hollow: Option<HollowDto>,
    /// What thickening moved: vertices, the furthest one in mm and the area in mm².
    pub thicken: Option<ThickenDto>,
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
    /// How the colour kit was last cut, to fill its dialog.
    pub kit: Option<bp_3mf::project_file::KitSettings>,
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

/// A wall measured where a ray landed: the point, the point across (preview
/// units) and the thickness in mm.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallDto {
    pub point: [f32; 3],
    pub across: [f32; 3],
    pub mm: Option<f32>,
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
    // Typed flush follows the slots' colours; the estimate, the colours and
    // materials they print, as Bambu Studio's flush table works it out.
    let shown = doc.slot_colors();
    let kinds: Vec<Option<&str>> = doc.slot_filaments().iter().map(|f| f.map(|f| f.kind.as_str())).collect();
    let least = doc.printer.flush_min_mm3();
    let n = pal.len();
    let mut volumes = Vec::with_capacity(n * n);
    let mut set = Vec::with_capacity(n * n);
    for ((&a, &sa), &ka) in pal.iter().zip(&shown).zip(&kinds) {
        for ((&b, &sb), &kb) in pal.iter().zip(&shown).zip(&kinds) {
            let key = (bp_core::color::to_hex(a), bp_core::color::to_hex(b));
            let typed = doc.flush.get(&key).copied().filter(|_| a != b);
            let estimate = || match bp_core::waste::default_flush(sa, sb, least) {
                0.0 => 0.0,
                v => bp_3mf::filament::flush(ka, kb, v),
            };
            volumes.push(typed.unwrap_or_else(estimate));
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
    // Square, as Bambu Studio 2 prints it. With no swap yet it is still planned:
    // as for one swap on the model's height.
    let tower_mm = match (prefs.plate.tower_depth_mm, waste.tower_footprint_mm) {
        (Some(d), _) => [d + 2.0 * TOWER_BRIM_MM; 2],
        (None, f) if f[1] > 0.0 => f,
        _ => [bp_core::waste::tower_side(params.prime_mm3, params.layer_mm, size[2]) + 2.0 * TOWER_BRIM_MM; 2],
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
                let check = if g.objects == 0 {
                    bp_3mf::plate::check_at(
                        &doc.printer,
                        [0.0; 2],
                        [0.0; 2],
                        &bp_3mf::Reserve { tower: false, ..reserve },
                    )
                } else {
                    bp_3mf::plate::check_at(&doc.printer, [g.size_mm[0], g.size_mm[1]], doc.offset_mm, &reserve)
                };
                PlateGroupDto {
                    objects: g.objects,
                    size_mm: g.size_mm,
                    centre: g.centre,
                    fits: g.objects == 0 || check.fits(),
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
        .filter(|p| p.objects > 0)
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
        object_ids: doc.project.object_ids(),
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
        protect_colors,
        protect_detail,
        subdivide,
        smooth,
        crease_deg,
        hollow_mm,
        drains,
        drain_mm,
        thicken_mm,
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
                plate: x.plate,
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
        deviation_mm: stats.deviation_mm,
        volume_mm3: stats.volume_mm3,
        thicken: stats.thicken.map(|t| ThickenDto {
            moved_vertices: t.moved_vertices,
            max_offset_mm: t.max_offset,
            area_mm2: t.area,
        }),
        hollow: stats.hollow.map(|h| HollowDto {
            solid_mm3: h.solid_volume,
            volume_mm3: h.volume,
            cell_mm: h.cell,
            drains: h.drains,
            drains_wanted: h.drains_wanted,
            refused: h.refused.map(|r| match r {
                bp_core::hollow::Refusal::Open => "open",
                bp_core::hollow::Refusal::NoRoom => "noRoom",
            }),
        }),
        repair: RepairDto {
            welded_vertices: stats.repair.welded_vertices,
            degenerate_triangles: stats.repair.degenerate_triangles,
            duplicate_triangles: stats.repair.duplicate_triangles,
            loose_pieces: stats.repair.loose_pieces,
            loose_triangles: stats.repair.loose_triangles,
            filled_holes: stats.repair.filled_holes,
            flipped_triangles: stats.repair.flipped_triangles,
            split_vertices: stats.repair.split_vertices,
            dropped_pieces: stats.repair.dropped_pieces,
            fin_triangles: stats.repair.fin_triangles,
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
            protect_colors,
            protect_detail,
            subdivide,
            smooth,
            crease_deg,
            hollow_mm,
            drains,
            drain_mm,
            thicken_mm,
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
        kit: doc.kit,
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

/// The region selection: its triangles (global preview ids) and how many separate regions they make.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDto {
    pub tris: Vec<u32>,
    pub count: usize,
    pub regions: usize,
}

pub fn selection(p: &Project) -> SelectionDto {
    let (count, regions) = p.selection_regions();
    SelectionDto { tris: p.selection(), count, regions }
}

/// A brush stroke of the region selection: where it landed and the triangles that changed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectStrokeDto {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub tris: Vec<u32>,
}

/// How a cut region is closed: a shell `mm` thick (the piece `clearance_mm`
/// smaller than its pocket), or a cap `mm` deep.
#[derive(Deserialize, Clone, Copy)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClosureDto {
    Shell {
        mm: f32,
        #[serde(default)]
        clearance_mm: f32,
    },
    Cap {
        mm: f32,
    },
}

/// What happens to the region: it goes, or it becomes objects of its own.
#[derive(Deserialize, Clone, Copy)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RegionActionDto {
    Remove,
    Extract {
        lay_flat: bool,
        #[serde(default)]
        group: bool,
    },
}

impl From<ClosureDto> for bp_core::region::Closure {
    fn from(c: ClosureDto) -> Self {
        match c {
            ClosureDto::Shell { mm, clearance_mm } => Self::Shell { thickness: mm, clearance: clearance_mm },
            ClosureDto::Cap { mm } => Self::Cap { depth: mm },
        }
    }
}

impl From<RegionActionDto> for bp_core::RegionAction {
    fn from(a: RegionActionDto) -> Self {
        match a {
            RegionActionDto::Remove => Self::Remove,
            RegionActionDto::Extract { lay_flat, group } => Self::Extract { lay_flat, group },
        }
    }
}

/// What cutting the region out would make, in preview coordinates: the
/// closing faces and each piece's surface as flat xyz triangles (9 floats each).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionPreviewDto {
    pub closure: Vec<f32>,
    pub pieces: Vec<Vec<f32>>,
    pub directions: Vec<[f32; 3]>,
    pub piece_mm3: Vec<f32>,
    pub rest_mm3: f32,
    pub closed: bool,
    /// Pieces narrower than two nozzle widths.
    pub small: usize,
}

pub fn region_preview(p: bp_core::RegionPreview) -> RegionPreviewDto {
    let flat = |tris: &[[[f32; 3]; 3]]| tris.iter().flatten().flatten().copied().collect();
    RegionPreviewDto {
        closure: flat(&p.closure),
        pieces: p.pieces.iter().map(|t| flat(t)).collect(),
        directions: p.directions,
        piece_mm3: p.piece_mm3,
        rest_mm3: p.rest_mm3,
        closed: p.closed,
        small: p.small,
    }
}

/// What makes faces the same colour when selecting by colour.
#[derive(Deserialize, Clone, Copy)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ColourMatchDto {
    Material,
    Filament,
    Similar { delta_e: f32 },
}

impl From<ColourMatchDto> for bp_core::ColourMatch {
    fn from(c: ColourMatchDto) -> Self {
        match c {
            ColourMatchDto::Material => Self::Material,
            ColourMatchDto::Filament => Self::Filament,
            ColourMatchDto::Similar { delta_e } => Self::Similar { delta_e },
        }
    }
}
