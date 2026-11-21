//! Tauri commands: thin adapters from the UI to the core use cases. Anything
//! that rebuilds the painting runs off the main thread. Errors go out as keys
//! (see `error.rs`) so the UI words them in the user's language.

use std::path::{Path, PathBuf};

use bp_core::{ColorOps, MeshOps, PrintOps};
use serde_json::json;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{self, BucketPickDto, HitDto, PaintDto, PrefsDto, PrintersDto, ProgressDto, Summary, TurnDto};
use crate::error::{in_range, AppError, CmdResult};
use crate::prefs::{PlatePrefs, Prefs, WastePrefs};
use crate::profiles::Store;
use crate::state::{AppState, Document};

/// Runs `f` on the open document in a blocking worker, with the print settings.
async fn with_doc<R: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&mut Document, &Prefs) -> CmdResult<R> + Send + 'static,
) -> CmdResult<R> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut guard = state.doc.lock()?;
        let doc = guard.as_mut().ok_or(AppError::new("error.noFile"))?;
        let prefs = state.prefs.lock()?;
        f(doc, &prefs)
    })
    .await
    .map_err(AppError::internal)?
}

/// What the core prepares for: the nozzle and the layer height it prints at.
pub(crate) fn print_ops(p: &bp_3mf::Printer, prefs: &Prefs) -> PrintOps {
    PrintOps { nozzle_mm: p.nozzle_mm, layer_mm: dto::layer_mm(p, &prefs.waste) }
}

fn with_store<R>(app: &AppHandle, f: impl FnOnce(&mut Store) -> CmdResult<R>) -> CmdResult<R> {
    let state = app.state::<AppState>();
    let mut s = state.printers.lock()?;
    f(&mut s)
}

fn with_prefs<R>(app: &AppHandle, f: impl FnOnce(&mut Prefs) -> CmdResult<R>) -> CmdResult<R> {
    let state = app.state::<AppState>();
    let mut p = state.prefs.lock()?;
    f(&mut p)
}

fn has_doc(app: &AppHandle) -> CmdResult<bool> {
    Ok(app.state::<AppState>().doc.lock()?.is_some())
}

/// Applies the store's pick and the print settings to the open document, if there is one.
async fn apply_printer(app: AppHandle) -> CmdResult<Option<Summary>> {
    let printer = with_store(&app, |s| Ok(s.printer()))?;
    if !has_doc(&app)? {
        return Ok(None);
    }
    with_doc(app, move |d, prefs| {
        d.project.set_print_ops(print_ops(&printer, prefs));
        d.printer = printer;
        Ok(Some(dto::summary(d, prefs)))
    })
    .await
}

fn blender_path(app: &AppHandle) -> CmdResult<PathBuf> {
    let state = app.state::<AppState>();
    let mut b = state.blender.lock()?;
    if b.is_none() {
        *b = bp_blend::find_blender();
    }
    b.clone().ok_or_else(|| bp_blend::BlendError::BlenderNotFound.into())
}

#[tauri::command]
pub fn blender_status(app: AppHandle) -> Option<String> {
    blender_path(&app).ok().map(|p| p.display().to_string())
}

/// Files the model references and whether each is at hand, with `extras` the
/// files and folders the user added. Reads the model only; Blender is not started.
#[tauri::command]
pub async fn inspect_model(
    path: String,
    extras: Vec<String>,
    library: Option<String>,
) -> CmdResult<Vec<bp_blend::companions::Companion>> {
    tauri::async_runtime::spawn_blocking(move || {
        let extras: Vec<PathBuf> = extras.into_iter().map(PathBuf::from).collect();
        Ok(bp_blend::companions::inspect_with(Path::new(&path), &extras, library.as_deref().map(Path::new)))
    })
    .await
    .map_err(AppError::internal)?
}

/// A supported model or project passed on the command line ("Open with…"), if any.
#[tauri::command]
pub fn initial_file() -> Option<String> {
    std::env::args().skip(1).find(|a| bp_blend::supports(Path::new(a)) || crate::saved::is_project(Path::new(a)))
}

#[tauri::command]
pub fn set_blender_path(app: AppHandle, path: String) -> CmdResult<String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(AppError::with("error.notAFile", json!({ "path": path })));
    }
    *app.state::<AppState>().blender.lock()? = Some(p);
    Ok(path)
}

#[tauri::command]
pub async fn open_blend(
    app: AppHandle,
    path: String,
    extras: Option<Vec<String>>,
    textures: Option<Vec<(String, String)>>,
    library: Option<String>,
    blend: Option<String>,
) -> CmdResult<Summary> {
    let blender = blender_path(&app)?;
    let printer = with_store(&app, |s| Ok(s.printer()))?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let file = Path::new(&path);
        let size = std::fs::metadata(file).map_err(|e| AppError::file(&path, e))?.len();
        // Steps arrive as "key name"; the UI words them.
        let emit = |msg: &str| {
            let (key, name) = msg.split_once(' ').unwrap_or((msg, ""));
            let _ = handle.emit("progress", ProgressDto { key: key.to_string(), name: name.to_string() });
        };
        emit("blender");
        let additions = bp_blend::Additions {
            files: extras.unwrap_or_default().into_iter().map(PathBuf::from).collect(),
            textures: textures.unwrap_or_default().into_iter().map(|(m, p)| (m, PathBuf::from(p))).collect(),
            library: library.map(PathBuf::from),
            blend: blend.map(PathBuf::from),
        };
        let imported = bp_blend::extract_with_files(&blender, file, &additions, &mut |m| emit(m))?;
        emit("palette");
        let mut project = bp_core::Project::new(imported.scene, 4);
        let state = handle.state::<AppState>();
        let prefs = state.prefs.lock()?;
        project.set_print_ops(print_ops(&printer, &prefs));
        let mut doc = Document {
            project,
            file_name: file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone()),
            file_size: size,
            import_warnings: imported.warnings,
            printer,
            flush: Default::default(),
            offset_mm: [0.0; 2],
            filaments: Default::default(),
            kit: None,
        };
        // With colours from the inventory only, the model starts with spools the user has.
        if let Some(owned) = state.filaments.lock()?.inventory() {
            doc.suggest_from_inventory(&owned, 4);
        }
        let summary = dto::summary(&doc, &prefs);
        *state.doc.lock()? = Some(doc);
        Ok(summary)
    })
    .await
    .map_err(AppError::internal)?
}

/// Saves the open document as a `.bpproj` at `path`.
#[tauri::command]
pub async fn save_project(app: AppHandle, path: String) -> CmdResult<()> {
    with_doc(app, move |d, _| crate::saved::save(d, &path)).await
}

/// Opens a `.bpproj`, without Blender, in place of the open document. The
/// printer it was prepared for is picked again when it is still known.
#[tauri::command]
pub async fn open_project(app: AppHandle, path: String) -> CmdResult<Summary> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let mut doc = {
            let mut store = state.printers.lock()?;
            let doc = crate::saved::open(&path, &store)?;
            let id = doc.printer.profile.id.clone();
            if store.profiles().iter().any(|p| p.id == id) {
                store.select(&id, doc.printer.nozzle_mm)?;
                store.save()?;
            }
            doc
        };
        let prefs = state.prefs.lock()?;
        doc.project.set_print_ops(print_ops(&doc.printer, &prefs));
        let summary = dto::summary(&doc, &prefs);
        *state.doc.lock()? = Some(doc);
        Ok(summary)
    })
    .await
    .map_err(AppError::internal)?
}

#[tauri::command]
pub async fn set_palette(app: AppHandle, colors: Vec<String>) -> CmdResult<Summary> {
    let parsed: Vec<_> = colors
        .iter()
        .map(|c| bp_core::color::parse_hex(c).ok_or_else(|| AppError::with("error.badColor", json!({ "color": c }))))
        .collect::<Result<_, _>>()?;
    with_doc(app, move |d, prefs| {
        d.project.set_palette(parsed);
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Detects up to `count` colours again, picking from the owned spools in inventory mode.
#[tauri::command]
pub async fn suggest_palette(app: AppHandle, count: usize) -> CmdResult<Summary> {
    let owned = app.state::<AppState>().filaments.lock()?.inventory();
    with_doc(app, move |d, prefs| {
        match &owned {
            Some(owned) => d.suggest_from_inventory(owned, count),
            None => {
                let pal = d.project.suggest_palette(count);
                d.project.set_palette(pal);
            }
        }
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Every filament to pick from, which ones the user has, and whether colours
/// come only from those.
#[tauri::command]
pub fn filaments(app: AppHandle) -> CmdResult<dto::FilamentsDto> {
    let state = app.state::<AppState>();
    let store = state.filaments.lock()?;
    Ok(dto::filaments(&store))
}

fn with_filaments(
    app: &AppHandle,
    f: impl FnOnce(&mut crate::filaments::FilamentStore) -> CmdResult<()>,
) -> CmdResult<dto::FilamentsDto> {
    let state = app.state::<AppState>();
    let mut store = state.filaments.lock()?;
    f(&mut store)?;
    store.save()?;
    Ok(dto::filaments(&store))
}

#[tauri::command]
pub fn set_filament_owned(app: AppHandle, id: String, owned: bool) -> CmdResult<dto::FilamentsDto> {
    with_filaments(&app, |s| s.set_owned(&id, owned))
}

/// Saves one of the user's filaments; a blank id makes a new one, which they own.
#[tauri::command]
pub fn save_filament(app: AppHandle, filament: bp_3mf::filament::Filament) -> CmdResult<dto::FilamentsDto> {
    with_filaments(&app, |s| s.upsert(filament).map(|_| ()))
}

#[tauri::command]
pub fn delete_filament(app: AppHandle, id: String) -> CmdResult<dto::FilamentsDto> {
    with_filaments(&app, |s| {
        s.remove(&id);
        Ok(())
    })
}

/// Whether suggested colours come only from the owned filaments.
#[tauri::command]
pub fn set_inventory_only(app: AppHandle, on: bool) -> CmdResult<dto::FilamentsDto> {
    with_filaments(&app, |s| {
        s.inventory_only = on;
        Ok(())
    })
}

/// Puts filament `id` on `slot`, or takes it off with `None`. The slot keeps
/// painting what it painted; it prints in the filament's colour.
#[tauri::command]
pub async fn set_slot_filament(app: AppHandle, slot: usize, id: Option<String>) -> CmdResult<Summary> {
    let filament = match &id {
        Some(id) => {
            Some(app.state::<AppState>().filaments.lock()?.find(id).ok_or(AppError::new("error.unknownFilament"))?)
        }
        None => None,
    };
    with_doc(app, move |d, prefs| {
        let hex =
            d.project.palette().get(slot).map(|c| bp_core::color::to_hex(*c)).ok_or(AppError::new("error.badSlots"))?;
        match filament {
            Some(f) => d.filaments.insert(hex, f),
            None => d.filaments.remove(&hex),
        };
        Ok(dto::summary(d, prefs))
    })
    .await
}

const SIZE_RANGE: std::ops::RangeInclusive<f32> = 1.0..=1000.0;

/// Width, depth and height in millimetres; unequal factors stretch the model.
#[tauri::command]
pub async fn set_size(app: AppHandle, mm: [f32; 3]) -> CmdResult<Summary> {
    for v in mm {
        in_range(v, SIZE_RANGE, "error.sizeRange")?;
    }
    with_doc(app, move |d, prefs| {
        d.project.set_size_mm(mm);
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn undo_size(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.undo_size();
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn reset_size(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.reset_size();
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// The largest uniform size that fits the plate, its margin and the tower's
/// reserved space, and that the printer reaches in height. The model goes back
/// to the plate's centre first: that is where it has the most room to grow.
#[tauri::command]
pub async fn fit_to_plate(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        let moved = d.offset_mm;
        d.offset_mm = [0.0; 2];
        let s = dto::summary(d, prefs);
        let flat = s.size.target_mm[2] - s.size_mm[2];
        let by_height = (d.printer.max_height_mm() + flat) / s.size.target_mm[2];
        let k = s.plate.check.fit_factor.min(by_height);
        // A hair under the limit, so the footprint does not touch the edge it was fitted to.
        let size = s.size.target_mm.map(|v| (v * k * 0.999 * 100.0).floor() / 100.0);
        if k.is_nan() || k <= 0.0 || size.iter().any(|v| !SIZE_RANGE.contains(v)) {
            d.offset_mm = moved;
            return Err(AppError::new("error.noRoom"));
        }
        d.project.set_size_mm(size);
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Moves the model's centre to `mm` from the plate's centre.
#[tauri::command]
pub async fn set_offset(app: AppHandle, mm: [f32; 2]) -> CmdResult<Summary> {
    let mm = mm.map(|v| if v.is_finite() { v.clamp(-1000.0, 1000.0) } else { 0.0 });
    with_doc(app, move |d, prefs| {
        d.offset_mm = mm;
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Turns the model `degrees` about X (0), Y (1) or Z (2); it lands back on the
/// plate. When the turn moved the preview as it is, says how, so the UI moves
/// it instead of fetching it again.
#[tauri::command]
pub async fn rotate(app: AppHandle, axis: usize, degrees: f32) -> CmdResult<TurnDto> {
    with_doc(app, move |d, prefs| {
        let rigid = d.project.rotate(axis, degrees);
        Ok(dto::turn(d, prefs, rigid))
    })
    .await
}

#[tauri::command]
pub async fn reset_rotation(app: AppHandle) -> CmdResult<TurnDto> {
    with_doc(app, |d, prefs| {
        let rigid = d.project.reset_rotation();
        Ok(dto::turn(d, prefs, rigid))
    })
    .await
}

fn cut_error(error: bp_core::cut::CutError) -> AppError {
    AppError::new(match error {
        bp_core::cut::CutError::Misses => "error.cutMisses",
        bp_core::cut::CutError::InvalidPlane => "error.cutPlane",
        bp_core::cut::CutError::InvalidGeometry => "error.cutGeometry",
    })
}

/// Dry run on a snapshot: validates the actual caps and retained geometry without
/// changing the document, its paint or its undo history. Releases the document
/// lock before the expensive cut so navigation and new file loads stay responsive.
#[tauri::command]
pub async fn preview_cut(
    app: AppHandle,
    point: [f32; 3],
    normal: [f32; 3],
    keep_low: bool,
    keep_other: bool,
    lay_on_cut: bool,
    selected: Vec<usize>,
) -> CmdResult<usize> {
    let mut project = with_doc(app, |d, _| Ok(d.project.clone())).await?;
    tauri::async_runtime::spawn_blocking(move || {
        project.cut_plane(point, normal, keep_low, keep_other, lay_on_cut, &selected).map_err(cut_error)?;
        Ok(project.object_count())
    })
    .await
    .map_err(AppError::internal)?
}

/// Cuts selected parts using the exact plane displayed by the preview.
/// Both halves can be retained and optionally laid on their cut faces.
#[tauri::command]
pub async fn cut_model(
    app: AppHandle,
    point: [f32; 3],
    normal: [f32; 3],
    keep_low: bool,
    keep_other: bool,
    lay_on_cut: bool,
    selected: Vec<usize>,
) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.cut_plane(point, normal, keep_low, keep_other, lay_on_cut, &selected).map_err(cut_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// The connectors of a cut as the UI sends them.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorsDto {
    /// "pin" or "dowel".
    kind: String,
    /// Pins stand out of half A (below the plane) unless this is false.
    pins_on_a: bool,
    /// "round" or "square".
    shape: String,
    size_mm: f32,
    depth_mm: f32,
    clearance_mm: f32,
    angle_deg: f32,
    at: Vec<[f32; 3]>,
}

impl ConnectorsDto {
    fn core(self) -> CmdResult<bp_core::connect::Connectors> {
        use bp_core::connect::{Connectors, Kind, Shape};
        let kind = match self.kind.as_str() {
            "pin" => Kind::Pin { on_low: self.pins_on_a },
            "dowel" => Kind::Dowel,
            _ => return Err(AppError::new("error.connectParams")),
        };
        let shape = match self.shape.as_str() {
            "round" => Shape::Round,
            "square" => Shape::Square,
            "hexagon" => Shape::Hexagon,
            _ => return Err(AppError::new("error.connectParams")),
        };
        let c = Connectors {
            kind,
            shape,
            size_mm: self.size_mm,
            depth_mm: self.depth_mm,
            clearance_mm: self.clearance_mm,
            angle_deg: self.angle_deg,
            at: self.at,
        };
        if !c.valid() || c.at.len() > 64 {
            return Err(AppError::new("error.connectParams"));
        }
        Ok(c)
    }
}

fn issue_key(issue: Option<bp_core::connect::Issue>) -> Option<&'static str> {
    use bp_core::connect::Issue;
    issue.map(|i| match i {
        Issue::Outside => "outside",
        Issue::Overlap => "overlap",
        Issue::ThinWall => "thinWall",
        Issue::TooDeep => "tooDeep",
    })
}

fn connect_error(e: bp_core::ConnectError) -> AppError {
    match e {
        bp_core::ConnectError::Cut(e) => cut_error(e),
        bp_core::ConnectError::Params => AppError::new("error.connectParams"),
        bp_core::ConnectError::NoConnectors => AppError::new("error.connectNone"),
        bp_core::ConnectError::Connectors(_) => AppError::new("error.connectIssues"),
    }
}

/// Where connectors of this size fit on the cut, in preview coordinates.
#[tauri::command]
pub async fn suggest_connectors(
    app: AppHandle,
    point: [f32; 3],
    normal: [f32; 3],
    selected: Vec<usize>,
    connectors: ConnectorsDto,
) -> CmdResult<Vec<[f32; 3]>> {
    let c = connectors.core()?;
    with_doc(app, move |d, _| d.project.suggest_connectors(point, normal, &selected, &c).map_err(cut_error)).await
}

/// Dry run of a cut with connectors on a copy: what is wrong with each
/// connector or, when they are all fine, how many objects the cut leaves.
#[tauri::command]
pub async fn preview_connected_cut(
    app: AppHandle,
    point: [f32; 3],
    normal: [f32; 3],
    lay_on_cut: bool,
    selected: Vec<usize>,
    connectors: ConnectorsDto,
) -> CmdResult<dto::ConnectReviewDto> {
    let c = connectors.core()?;
    let mut project = with_doc(app, |d, _| Ok(d.project.clone())).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let issues = project.check_connectors(point, normal, &selected, &c).map_err(cut_error)?;
        if issues.iter().any(Option::is_some) || c.at.is_empty() {
            return Ok(dto::ConnectReviewDto { issues: issues.into_iter().map(issue_key).collect(), objects: None });
        }
        let objects = project.cut_connected(point, normal, lay_on_cut, &selected, &c, "").map_err(connect_error)?;
        Ok(dto::ConnectReviewDto { issues: vec![None; c.at.len()], objects: Some(objects) })
    })
    .await
    .map_err(AppError::internal)?
}

/// Cuts keeping both halves with the connectors made in them; one undoable step.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn cut_connected(
    app: AppHandle,
    point: [f32; 3],
    normal: [f32; 3],
    lay_on_cut: bool,
    selected: Vec<usize>,
    connectors: ConnectorsDto,
    dowel_label: String,
) -> CmdResult<Summary> {
    let c = connectors.core()?;
    let label: String = dowel_label.chars().filter(|c| !c.is_control()).take(32).collect();
    with_doc(app, move |d, prefs| {
        d.project.cut_connected(point, normal, lay_on_cut, &selected, &c, &label).map_err(connect_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// What a split for the printer asks for, as the UI sends it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SplitPlanDto {
    /// Cut positions along X, Y and Z in preview coordinates.
    cuts: [Vec<f32>; 3],
    lay_on_cut: bool,
    gap_mm: f32,
    /// Dowels across the joints: "round" or "square", and their sizes.
    dowels: Option<SplitDowelsDto>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SplitDowelsDto {
    shape: String,
    size_mm: f32,
    depth_mm: f32,
    clearance_mm: f32,
}

impl SplitPlanDto {
    fn core(self) -> CmdResult<(bp_core::SplitPlan, f32)> {
        let dowels = match self.dowels {
            None => None,
            Some(d) => Some(
                ConnectorsDto {
                    kind: "dowel".into(),
                    pins_on_a: true,
                    shape: d.shape,
                    size_mm: d.size_mm,
                    depth_mm: d.depth_mm,
                    clearance_mm: d.clearance_mm,
                    angle_deg: 0.0,
                    at: vec![],
                }
                .core()?,
            ),
        };
        in_range(self.gap_mm, 0.0..=50.0, "error.splitGap")?;
        Ok((bp_core::SplitPlan { cuts: self.cuts, lay_on_cut: self.lay_on_cut, dowels }, self.gap_mm))
    }
}

/// The room every plate has for pieces: centred, clear of the margin, the
/// excluded zones and the tower, and as tall as the printer prints.
fn plate_room(d: &Document, prefs: &Prefs, gap_mm: f32, offset_mm: [f32; 2]) -> bp_core::fit::Room {
    let stats = d.project.stats();
    let (_, plate) = dto::waste_and_plate(d, prefs, &stats.swaps, stats.size_mm);
    let reserve = bp_3mf::Reserve {
        tower: prefs.plate.tower && plate.needs_tower,
        tower_mm: plate.tower_mm,
        tower_gap_mm: prefs.plate.tower_gap_mm,
        margin_mm: prefs.plate.margin_mm,
    };
    bp_core::fit::Room {
        area_mm: bp_3mf::plate::centred_room_at(&d.printer, &reserve, offset_mm),
        height_mm: d.printer.max_height_mm(),
        gap_mm,
    }
}

fn split_error(e: bp_core::SplitError) -> AppError {
    match e {
        bp_core::SplitError::NoCuts => AppError::new("error.splitNoCuts"),
        bp_core::SplitError::Params => AppError::new("error.splitParams"),
        bp_core::SplitError::Cut(e) => cut_error(e),
        bp_core::SplitError::DoesNotFit(_) => AppError::new("error.splitDoesNotFit"),
    }
}

/// Where to cut so every piece fits the printer, and the room it was worked out for.
#[tauri::command]
pub async fn propose_split(app: AppHandle, gap_mm: f32) -> CmdResult<dto::SplitProposalDto> {
    in_range(gap_mm, 0.0..=50.0, "error.splitGap")?;
    with_doc(app, move |d, prefs| {
        let room = plate_room(d, prefs, gap_mm, [0.0; 2]);
        let stats = d.project.stats();
        Ok(dto::SplitProposalDto {
            cuts: d.project.propose_split(&room),
            room_mm: room.area_mm,
            height_mm: room.height_mm,
            size_mm: stats.size_mm,
        })
    })
    .await
}

/// What a split would make, without changing the document.
#[tauri::command]
pub async fn preview_split(app: AppHandle, plan: SplitPlanDto) -> CmdResult<dto::SplitReportDto> {
    let (plan, gap) = plan.core()?;
    let (project, room) =
        with_doc(app, move |d, prefs| Ok((d.project.clone(), plate_room(d, prefs, gap, [0.0; 2])))).await?;
    tauri::async_runtime::spawn_blocking(move || {
        project.preview_split(&plan, &room).map(dto::split_report).map_err(split_error)
    })
    .await
    .map_err(AppError::internal)?
}

/// Splits the model into pieces that fit and shares them out across plates; one undoable step.
#[tauri::command]
pub async fn split_for_printer(app: AppHandle, plan: SplitPlanDto, dowel_label: String) -> CmdResult<Summary> {
    let (plan, gap) = plan.core()?;
    let label: String = dowel_label.chars().filter(|c| !c.is_control()).take(32).collect();
    with_doc(app, move |d, prefs| {
        let room = plate_room(d, prefs, gap, [0.0; 2]);
        d.project.split_for_printer(&plan, &room, &label).map_err(split_error)?;
        // Each plate's pieces go on its centre.
        d.offset_mm = [0.0; 2];
        Ok(dto::summary(d, prefs))
    })
    .await
}

fn plate_edit_error(error: bp_core::PlateEditError) -> AppError {
    AppError::new(match error {
        bp_core::PlateEditError::Plate => "error.plateMissing",
        bp_core::PlateEditError::Object => "error.plateObjectMissing",
        bp_core::PlateEditError::ObjectLimit => "error.plateObjectLimit",
        bp_core::PlateEditError::NoRoom => "error.noRoom",
        bp_core::PlateEditError::DoesNotFit => "error.plateArrangeOversize",
    })
}

#[tauri::command]
pub async fn add_plate(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.add_plate();
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn move_to_plate(app: AppHandle, object: u32, plate: u32) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.move_to_plate(object, plate, d.printer.bed_mm().0).map_err(plate_edit_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn clone_object(app: AppHandle, object: u32, label: String, gap_mm: Option<f32>) -> CmdResult<Summary> {
    if let Some(gap) = gap_mm {
        in_range(gap, 0.0..=50.0, "error.splitGap")?;
    }
    let label: String = label.chars().filter(|c| !c.is_control()).take(32).collect();
    with_doc(app, move |d, prefs| {
        let width = d.printer.bed_mm().0;
        if let Some(gap) = gap_mm {
            let room = plate_room(d, prefs, gap, d.offset_mm);
            d.project.clone_and_arrange(object, &label, &room, width)
        } else {
            d.project.clone_object(object, &label, width)
        }
        .map_err(plate_edit_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn arrange_plates(app: AppHandle, gap_mm: f32) -> CmdResult<Summary> {
    in_range(gap_mm, 0.0..=50.0, "error.splitGap")?;
    with_doc(app, move |d, prefs| {
        let room = plate_room(d, prefs, gap_mm, d.offset_mm);
        d.project.arrange_plates(&room, d.printer.bed_mm().0).map_err(plate_edit_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn clear_plate(app: AppHandle, plate: u32) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.clear_plate(plate).map_err(plate_edit_error)?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn undo_cut(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.undo_cut();
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Tolerance for welding corners when looking for pieces, in millimetres.
const WELD_RANGE: std::ops::RangeInclusive<f32> = 0.0..=5.0;

/// The model's disconnected pieces and, per preview triangle, the piece it is in.
#[tauri::command]
pub async fn detect_pieces(app: AppHandle, weld_mm: f32) -> CmdResult<dto::PiecesDto> {
    in_range(weld_mm, WELD_RANGE, "error.weldRange")?;
    with_doc(app, move |d, _| Ok(dto::pieces(&d.project, weld_mm))).await
}

/// Pieces with the same group become one object; `None` leaves a piece where it is.
#[tauri::command]
pub async fn separate_pieces(
    app: AppHandle,
    weld_mm: f32,
    groups: Vec<Option<u32>>,
    lay_flat: bool,
) -> CmdResult<Summary> {
    in_range(weld_mm, WELD_RANGE, "error.weldRange")?;
    with_doc(app, move |d, prefs| {
        d.project.separate_pieces(weld_mm, &groups, lay_flat).map_err(|e| {
            AppError::new(match e {
                bp_core::SeparateError::Stale => "error.separateStale",
                bp_core::SeparateError::NothingToSeparate => "error.separateNothing",
            })
        })?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Planes to review for splitting a continuous mesh; nothing is cut here.
#[tauri::command]
pub async fn propose_cuts(app: AppHandle, selected: Vec<usize>) -> CmdResult<Vec<dto::ProposalDto>> {
    with_doc(app, move |d, _| Ok(d.project.propose_cuts(&selected).into_iter().map(dto::proposal).collect())).await
}

/// Brush radius for the region selection, and how thick a shell or deep a cap can be, in millimetres.
const REGION_RADIUS: std::ops::RangeInclusive<f32> = 0.1..=100.0;
const CLOSURE_RANGE: std::ops::RangeInclusive<f32> = -50.0..=50.0;

/// Adds to the region selection (or with `add` false, takes from it) where
/// the ray lands; with `through`, the brush reaches walls behind the surface too.
#[tauri::command]
pub async fn select_region(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    radius_mm: f32,
    add: bool,
    through: bool,
) -> CmdResult<Option<dto::SelectStrokeDto>> {
    in_range(radius_mm, REGION_RADIUS, "error.radiusRange")?;
    with_doc(app, move |d, _| {
        Ok(d.project.select(origin, dir, radius_mm, add, through).map(|(h, tris)| dto::SelectStrokeDto {
            point: h.point,
            normal: h.normal,
            tris,
        }))
    })
    .await
}

/// Reshapes the region selection: "grow", "connected", "smooth", "invert" or "clear"; "get" only reads it.
#[tauri::command]
pub async fn selection_op(app: AppHandle, op: String) -> CmdResult<dto::SelectionDto> {
    with_doc(app, move |d, _| {
        match op.as_str() {
            "grow" => d.project.grow_selection(),
            "connected" => d.project.select_connected(),
            "smooth" => d.project.smooth_selection(),
            "invert" => d.project.invert_selection(),
            "clear" => d.project.clear_selection(),
            "get" => {}
            _ => return Err(AppError::with("error.internal", json!({ "detail": op }))),
        }
        Ok(dto::selection(&d.project))
    })
    .await
}

fn closure(c: dto::ClosureDto) -> CmdResult<bp_core::region::Closure> {
    let (dto::ClosureDto::Shell { mm, .. } | dto::ClosureDto::Cap { mm }) = c;
    in_range(mm, CLOSURE_RANGE, "error.closureRange")?;
    if let dto::ClosureDto::Shell { clearance_mm, .. } = c {
        if mm <= 0.0 {
            return Err(AppError::with("error.closureRange", json!({ "min": 0.0, "max": CLOSURE_RANGE.end() })));
        }
        in_range(clearance_mm, 0.0..=5.0, "error.clearanceRange")?;
    }
    Ok(c.into())
}

/// Selects (or with `add` false, deselects) the faces the same colour as
/// preview triangle `seed`: only those joined to it with `contiguous`. With
/// `base`, the selection is that first, so trying a pick again with another
/// tolerance is one step that no other pick can come between.
#[tauri::command]
pub async fn select_by_colour(
    app: AppHandle,
    seed: u32,
    how: dto::ColourMatchDto,
    contiguous: bool,
    add: bool,
    base: Option<Vec<u32>>,
) -> CmdResult<dto::SelectionDto> {
    if let dto::ColourMatchDto::Similar { delta_e } = how {
        in_range(delta_e, 0.0..=100.0, "error.deltaERange")?;
    }
    with_doc(app, move |d, _| {
        if let Some(base) = base {
            d.project.set_selection(&base);
        }
        d.project.select_by_colour(seed, how.into(), contiguous, add);
        Ok(dto::selection(&d.project))
    })
    .await
}

/// Makes colour borders that cross triangles edges of the mesh; one undoable step.
#[tauri::command]
pub async fn refine_to_paint(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.refine_to_paint()?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// What cutting the selected region out would make; nothing changes.
#[tauri::command]
pub async fn preview_region(
    app: AppHandle,
    closure: dto::ClosureDto,
    action: dto::RegionActionDto,
) -> CmdResult<dto::RegionPreviewDto> {
    let c = self::closure(closure)?;
    with_doc(app, move |d, _| Ok(dto::region_preview(d.project.preview_region(c, action.into())?))).await
}

/// Cuts the selected region out as one undoable step.
#[tauri::command]
pub async fn apply_region(
    app: AppHandle,
    closure: dto::ClosureDto,
    action: dto::RegionActionDto,
) -> CmdResult<Summary> {
    let c = self::closure(closure)?;
    with_doc(app, move |d, prefs| {
        d.project.apply_region(c, action.into())?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn reduce_palette(app: AppHandle, target: usize) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.reduce_palette(target);
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn merge_slots(app: AppHandle, from: usize, into: usize) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.merge_slots(from, into);
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_mesh_ops(
    app: AppHandle,
    keep_ratio: f32,
    flatten_mm: f32,
    repair: bool,
    min_piece_mm: f32,
    protect_colors: Option<bool>,
    protect_detail: Option<bool>,
) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        // The settings panel has no say over the printability fixes; keep them.
        let now = d.project.mesh_ops();
        let ops = MeshOps {
            keep_ratio: keep_ratio.clamp(0.05, 1.0),
            flatten_mm: flatten_mm.max(0.0),
            repair,
            min_piece_mm: min_piece_mm.max(0.0),
            protect_colors: protect_colors.unwrap_or(now.protect_colors),
            protect_detail: protect_detail.unwrap_or(now.protect_detail),
            ..now
        };
        d.project.set_mesh_ops(ops);
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn set_color_ops(app: AppHandle, border_detail: u8, min_island_mm2: f32) -> CmdResult<Summary> {
    let ops = ColorOps { border_detail, min_island_mm2: min_island_mm2.max(0.0) };
    with_doc(app, move |d, prefs| {
        d.project.set_color_ops(ops);
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub fn printers(app: AppHandle) -> CmdResult<PrintersDto> {
    with_store(&app, |s| Ok(dto::printers(s)))
}

/// Picks a printer and nozzle; it is remembered for the next file and session.
/// Returns the new summary when a file is open.
#[tauri::command]
pub async fn set_printer(app: AppHandle, id: String, nozzle_mm: f32) -> CmdResult<Option<Summary>> {
    let printer = with_store(&app, |s| {
        let p = s.select(&id, nozzle_mm)?;
        s.save()?;
        Ok(p)
    })?;
    // A layer height the new nozzle cannot print goes back to the printer's own.
    with_prefs(&app, |p| {
        if p.waste.validated(printer.nozzle_mm).is_err() {
            p.waste.layer_mm = None;
            p.save()?;
        }
        Ok(())
    })?;
    apply_printer(app).await
}

/// Saves a custom printer (a blank id makes a new one) and picks it.
#[tauri::command]
pub async fn save_printer(app: AppHandle, profile: bp_3mf::PrinterProfile, nozzle_mm: f32) -> CmdResult<PrintersDto> {
    let dto = with_store(&app, |s| {
        let saved = s.upsert(profile)?;
        s.select(&saved.id, nozzle_mm)?;
        s.save()?;
        Ok(dto::printers(s))
    })?;
    apply_printer(app).await?;
    Ok(dto)
}

#[tauri::command]
pub async fn delete_printer(app: AppHandle, id: String) -> CmdResult<PrintersDto> {
    let dto = with_store(&app, |s| {
        s.remove(&id);
        s.save()?;
        Ok(dto::printers(s))
    })?;
    apply_printer(app).await?;
    Ok(dto)
}

#[tauri::command]
pub fn prefs(app: AppHandle) -> CmdResult<PrefsDto> {
    with_prefs(&app, |p| Ok(dto::prefs(p)))
}

/// Saves what the waste estimate assumes; the open model is estimated again.
#[tauri::command]
pub async fn set_waste(app: AppHandle, waste: WastePrefs) -> CmdResult<Option<Summary>> {
    let nozzle = with_store(&app, |s| Ok(s.printer().nozzle_mm))?;
    let waste = waste.validated(nozzle)?;
    with_prefs(&app, |p| {
        p.waste = waste;
        p.save()
    })?;
    apply_printer(app).await
}

/// Saves what is kept free on the plate; the open model is checked again.
#[tauri::command]
pub async fn set_plate(app: AppHandle, plate: PlatePrefs) -> CmdResult<Option<Summary>> {
    let plate = plate.validated()?;
    with_prefs(&app, |p| {
        p.plate = plate;
        p.save()
    })?;
    apply_printer(app).await
}

/// Sets the flush of a swap between two slots of the open model; `None` goes
/// back to the estimate from their colours. Without slots, clears them all.
#[tauri::command]
pub async fn set_flush(app: AppHandle, from: Option<usize>, to: Option<usize>, mm3: Option<f32>) -> CmdResult<Summary> {
    if let Some(v) = mm3 {
        in_range(v, 0.0..=2000.0, "error.flushRange")?;
    }
    with_doc(app, move |d, prefs| {
        let hex = |i: usize| d.project.palette().get(i).map(|c| bp_core::color::to_hex(*c));
        match (from.and_then(hex), to.and_then(hex)) {
            (Some(a), Some(b)) if a != b => match mm3 {
                Some(v) => {
                    d.flush.insert((a, b), v);
                }
                None => {
                    d.flush.remove(&(a, b));
                }
            },
            _ if from.is_none() && to.is_none() => d.flush.clear(),
            _ => return Err(AppError::new("error.badSlots")),
        }
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// The paintable mesh as one binary blob:
/// `u32 leaves, u32 tris | f32 positions[9 * leaves] | u32 offsets[tris + 1] | u8 labels[leaves] | u8 rgb[3 * leaves]`.
#[tauri::command]
pub async fn preview_mesh(app: AppHandle) -> CmdResult<Response> {
    let bytes = with_doc(app, |d, _| Ok(d.project.preview_bytes())).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn pick(app: AppHandle, origin: [f32; 3], dir: [f32; 3]) -> CmdResult<Option<HitDto>> {
    with_doc(app, move |d, _| Ok(d.project.pick(origin, dir).map(|h| HitDto { point: h.point, normal: h.normal })))
        .await
}

/// The wall where the ray lands, as the thickness check reads it. `mm` is None
/// with nothing across (an open surface).
#[tauri::command]
pub async fn measure_thickness(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    tolerance_mm: f32,
) -> CmdResult<Option<dto::WallDto>> {
    if !tolerance_mm.is_finite() || !(0.00001..=0.1).contains(&tolerance_mm) {
        return Err(AppError::new("error.printabilityParams"));
    }
    with_doc(app, move |d, _| {
        Ok(d.project.wall_at(origin, dir, tolerance_mm).map(|(point, across, mm)| dto::WallDto { point, across, mm }))
    })
    .await
}

/// The working mesh's edges, each once, as one binary blob:
/// `u32 vertices, u32 edges | f32 positions[3 * vertices] | u32 ends[2 * edges]`.
#[tauri::command]
pub async fn wire_mesh(app: AppHandle) -> CmdResult<Response> {
    let bytes = with_doc(app, |d, _| {
        let (positions, edges) = d.project.wire();
        let mut out = Vec::with_capacity(8 + positions.len() * 12 + edges.len() * 8);
        out.extend((positions.len() as u32).to_le_bytes());
        out.extend((edges.len() as u32).to_le_bytes());
        for p in &positions {
            for v in p {
                out.extend(v.to_le_bytes());
            }
        }
        for e in &edges {
            for v in e {
                out.extend(v.to_le_bytes());
            }
        }
        Ok(out)
    })
    .await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn paint(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    radius_mm: f32,
    slot: u8,
) -> CmdResult<Option<PaintDto>> {
    with_doc(app, move |d, _| {
        Ok(d.project.paint(origin, dir, radius_mm, slot).map(|(h, tris)| PaintDto {
            point: h.point,
            normal: h.normal,
            tris,
        }))
    })
    .await
}

/// What a bucket click along the ray would fill: in the connected `region`
/// only or everywhere, and with `add` the clicked colour as a target of its own.
#[tauri::command]
pub async fn bucket_pick(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    region: bool,
    add: bool,
) -> CmdResult<Option<BucketPickDto>> {
    with_doc(app, move |d, _| Ok(d.project.bucket_pick(origin, dir, region, add).map(Into::into))).await
}

/// Sends the model colour under the click to `slot`; one undoable step.
#[tauri::command]
pub async fn bucket(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    slot: u8,
    region: bool,
    add: bool,
) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        d.project.bucket(origin, dir, slot, region, add).ok_or(AppError::new("error.bucketMisses"))?;
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn end_stroke(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.end_stroke();
        Ok(dto::summary(d, prefs))
    })
    .await
}

#[tauri::command]
pub async fn undo(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d, prefs| {
        d.project.undo();
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Set to stop the colour kit being exported.
static KIT_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stops the colour kit export at its next picture or file; nothing is left behind.
#[tauri::command]
pub fn cancel_color_zip() {
    KIT_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The model split into solid pieces per colour, laid out on plates, with
/// STLs and an assembly guide in `lang`, all in one ZIP. It is written next to
/// `path` and renamed into place only once complete; `kitProgress` events say
/// how far it has got, and [`cancel_color_zip`] stops it.
#[tauri::command]
pub async fn export_color_zip(
    app: AppHandle,
    path: String,
    options: bp_3mf::project_file::KitSettings,
    lang: String,
) -> CmdResult<dto::KitDto> {
    let bp_3mf::project_file::KitSettings { voxel_mm, depth_mm, core_slot, min_piece_mm3, mix, contents, pins } =
        options;
    let lang = bp_3mf::KitLang::from_code(&lang);
    in_range(voxel_mm, 0.2..=2.0, "error.kitDetailRange")?;
    in_range(depth_mm, 0.0..=50.0, "error.kitDepthRange")?;
    if let Some(p) = pins {
        in_range(p.diameter_mm, 2.0..=20.0, "error.kitPinDiameterRange")?;
        in_range(p.depth_mm, 1.0..=20.0, "error.kitPinDepthRange")?;
        in_range(p.clearance_mm, 0.0..=1.0, "error.kitPinClearanceRange")?;
    }
    if !contents.any() {
        return Err(AppError::new("error.kitNothing"));
    }
    KIT_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    let handle = app.clone();
    with_doc(app, move |d, prefs| {
        let emit = |stage: &str, done: usize, total: usize| {
            let _ = handle.emit("kitProgress", dto::KitProgressDto { stage: stage.to_string(), done, total });
        };
        let cancelled = || KIT_CANCEL.load(std::sync::atomic::Ordering::Relaxed);
        emit("split", 0, 0);
        let params = bp_core::SplitParams {
            voxel_mm,
            depth_mm,
            core_slot,
            min_piece_mm3: min_piece_mm3.max(0.0),
            pins: pins.map(|p| bp_core::PinParams {
                diameter_mm: p.diameter_mm,
                depth_mm: p.depth_mm,
                clearance_mm: p.clearance_mm,
                ..Default::default()
            }),
            exact_skin: true,
        };
        d.kit = Some(bp_3mf::project_file::KitSettings {
            voxel_mm,
            depth_mm,
            core_slot,
            min_piece_mm3: params.min_piece_mm3,
            mix,
            contents,
            pins,
        });
        let split = d.project.split_by_color(params);
        if split.pieces.is_empty() {
            return Err(AppError::new("error.kitEmpty"));
        }
        if cancelled() {
            return Err(AppError::new("error.kitCancelled"));
        }
        emit("plates", 0, 0);
        // The plate as the user keeps it: margin, gap and, where colours mix, the tower.
        let waste = dto::waste_params(d, prefs);
        let tallest = split
            .pieces
            .iter()
            .map(|p| {
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for v in &p.positions {
                    lo = std::array::from_fn(|a| lo[a].min(v[a]));
                    hi = std::array::from_fn(|a| hi[a].max(v[a]));
                }
                // Lying on its broad face, a piece stands about its thinnest.
                (0..3).map(|a| hi[a] - lo[a]).fold(f32::MAX, f32::min)
            })
            .fold(0.0, f32::max);
        let side = prefs
            .plate
            .tower_depth_mm
            .unwrap_or_else(|| bp_core::waste::tower_side(waste.prime_mm3, waste.layer_mm, tallest))
            + 2.0 * dto::TOWER_BRIM_MM;
        let reserve = bp_3mf::Reserve {
            tower: prefs.plate.tower,
            tower_mm: [side; 2],
            tower_gap_mm: prefs.plate.tower_gap_mm,
            margin_mm: prefs.plate.margin_mm,
        };
        let kit = bp_core::kit::kit(split, bp_3mf::kit_bed(&d.printer, &reserve, prefs.plate.tower_gap_mm, mix));
        // Each slot in the colour it prints, with its filament where one was picked.
        let palette: Vec<String> = d.slot_colors().iter().map(|c| bp_core::color::to_hex(*c)).collect();
        let slots: Vec<_> =
            d.slot_filaments().iter().map(|f| f.map(|f| bp_3mf::filament::for_slot(&f.kind, &d.printer))).collect();
        let filament_names: Vec<Option<String>> = d
            .slot_filaments()
            .iter()
            .map(|f| {
                f.map(|f| {
                    let name = if lang == bp_3mf::KitLang::En { &f.name_en } else { &f.name_es };
                    format!("{} {} {}", f.brand, f.kind, name).trim().to_string()
                })
            })
            .collect();
        let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let info = bp_3mf::KitInfo {
            name: &name,
            palette: &palette,
            slots: &slots,
            filament_names: &filament_names,
            printer: &d.printer,
            params: d.kit.unwrap_or(options),
            density_g_cm3: prefs.waste.density_g_cm3,
            flush: bp_3mf::Flush {
                volumes_mm3: dto::flush_matrix(d).0,
                multiplier: prefs.waste.flush_multiplier,
                into_infill: prefs.waste.flush_reuse > 0.0,
                prime_mm3: prefs.waste.prime_mm3,
            },
            lang,
        };
        // Never a half-written ZIP at `path`: write beside it, then rename.
        let part = format!("{path}.part");
        let written = (|| {
            let file = std::fs::File::create(&part).map_err(|e| AppError::file(&part, e))?;
            let mut out = std::io::BufWriter::new(file);
            let s = bp_3mf::write_kit_zip_with(&mut out, &kit, &info, &|stage, done, total| {
                let stage = match stage {
                    bp_3mf::KitStage::Pictures => "pictures",
                    bp_3mf::KitStage::Files => "files",
                };
                emit(stage, done, total);
                !cancelled()
            })
            .map_err(|e| {
                if cancelled() {
                    AppError::new("error.kitCancelled")
                } else {
                    AppError::with("error.zipWrite", json!({ "detail": e.to_string() }))
                }
            })?;
            let file = out.into_inner().map_err(|e| AppError::file(&part, e.into_error()))?;
            file.sync_all().map_err(|e| AppError::file(&part, e))?;
            drop(file);
            std::fs::rename(&part, &path).map_err(|e| AppError::file(&path, e))?;
            Ok(s)
        })();
        let s = match written {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        Ok(dto::KitDto {
            pieces: s.pieces,
            plates: s.plates,
            oversize: s.oversize,
            review: s.review,
            dropped: s.dropped,
            pins: s.pins,
            unpinned: s.unpinned,
            bytes: std::fs::metadata(&path).map_or(0, |m| m.len()),
        })
    })
    .await
}

/// The open model as a Bambu Studio 3MF. `objects` picks which printed objects
/// go in (all when `None`); pieces are named in `lang`. `thumbnails` are the
/// plate's PNG pictures, large and small, drawn by the preview; without them
/// the file goes out without pictures.
#[tauri::command]
pub async fn export_3mf(
    app: AppHandle,
    path: String,
    lang: String,
    objects: Option<Vec<u32>>,
    thumbnails: Option<(Vec<u8>, Vec<u8>)>,
) -> CmdResult<()> {
    with_doc(app, move |d, prefs| write_selected_3mf(d, prefs, &path, &lang, objects, thumbnails.as_ref())).await
}

#[cfg(feature = "bench")]
pub(crate) fn write_3mf(d: &Document, prefs: &Prefs, path: &str) -> CmdResult<()> {
    write_selected_3mf(d, prefs, path, "es", None, None)
}

fn write_selected_3mf(
    d: &Document,
    prefs: &Prefs,
    path: &str,
    lang: &str,
    objects: Option<Vec<u32>>,
    thumbnails: Option<&(Vec<u8>, Vec<u8>)>,
) -> CmdResult<()> {
    let mut parts = d.project.export_parts();
    if let Some(keep) = &objects {
        parts.retain(|p| keep.contains(&p.object));
    }
    if parts.is_empty() {
        return Err(AppError::new("error.noObjects"));
    }
    let filaments = d.slot_colors().iter().map(|c| bp_core::color::to_hex(*c)).collect();
    let slots = d.slot_filaments().iter().map(|f| f.map(|f| bp_3mf::filament::for_slot(&f.kind, &d.printer))).collect();
    let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (volumes_mm3, _) = dto::flush_matrix(d);
    let model = bp_3mf::Model {
        plate_count: d.project.plate_count(),
        name: &name,
        piece: bp_3mf::ZipText::for_lang(lang).piece,
        parts: parts
            .iter()
            .map(|p| bp_3mf::Part {
                name: &p.name,
                positions: &p.positions,
                tris: &p.tris,
                paint: &p.paint,
                object: p.object,
                plate: p.plate,
            })
            .collect(),
        filaments,
        slots,
        printer: d.printer.clone(),
        flush: bp_3mf::Flush {
            volumes_mm3,
            multiplier: prefs.waste.flush_multiplier,
            into_infill: prefs.waste.flush_reuse > 0.0,
            prime_mm3: prefs.waste.prime_mm3,
        },
        layer_mm: prefs.waste.layer_mm,
        offset_mm: d.offset_mm,
        thumbnail: thumbnails.map(|(large, small)| bp_3mf::Thumbnail { large, small }),
    };
    let file = std::fs::File::create(path).map_err(|e| AppError::file(path, e))?;
    bp_3mf::write(std::io::BufWriter::new(file), &model)
        .map_err(|e| AppError::with("error.3mfWrite", json!({ "detail": e.to_string() })))
}

/// One STL per colour in a ZIP, its colour list in `lang`. Returns how many colour files were written.
#[tauri::command]
pub async fn export_color_shells_zip(
    app: AppHandle,
    path: String,
    thickness_mm: f32,
    main_solid: bool,
    lang: String,
) -> CmdResult<usize> {
    in_range(thickness_mm, 0.2..=20.0, "error.shellRange")?;
    with_doc(app, move |d, _| {
        let pieces = d.project.color_pieces(thickness_mm, main_solid);
        if pieces.is_empty() {
            return Err(AppError::new("error.platesEmpty"));
        }
        let hexes: Vec<String> =
            pieces.iter().map(|p| bp_core::color::to_hex(d.project.palette()[p.slot as usize])).collect();
        let files: Vec<bp_3mf::ColorFile> = pieces
            .iter()
            .zip(&hexes)
            .map(|(p, hex)| bp_3mf::ColorFile { hex, positions: &p.positions, tris: &p.tris, solid: p.solid })
            .collect();
        let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let file = std::fs::File::create(&path).map_err(|e| AppError::file(&path, e))?;
        bp_3mf::write_color_zip(std::io::BufWriter::new(file), &name, &files, bp_3mf::ZipText::for_lang(&lang))
            .map_err(|e| AppError::with("error.zipWrite", json!({ "detail": e.to_string() })))?;
        Ok(files.len())
    })
    .await
}

#[tauri::command]
pub async fn export_color_assembly(
    app: AppHandle,
    path: String,
    thickness_mm: f32,
    main_solid: bool,
) -> CmdResult<usize> {
    in_range(thickness_mm, 0.2..=20.0, "error.shellRange")?;
    with_doc(app, move |d, prefs| {
        let pieces = d.project.color_pieces(thickness_mm, main_solid);
        let names: Vec<_> =
            pieces.iter().map(|p| bp_core::color::to_hex(d.project.palette()[p.slot as usize])).collect();
        let filaments: Vec<_> = pieces.iter().map(|p| p.slot).collect();
        let parts = pieces
            .iter()
            .zip(&names)
            .map(|(p, name)| bp_3mf::Part {
                name,
                positions: &p.positions,
                tris: &p.tris,
                paint: &[],
                object: 0,
                plate: 0,
            })
            .collect();
        write_color_assembly(d, prefs, &path, parts, &filaments)?;
        Ok(pieces.len())
    })
    .await
}

fn write_color_assembly(
    d: &Document,
    prefs: &Prefs,
    path: &str,
    parts: Vec<bp_3mf::Part<'_>>,
    part_filaments: &[u8],
) -> CmdResult<()> {
    let filaments = d.slot_colors().iter().map(|c| bp_core::color::to_hex(*c)).collect();
    let slots = d.slot_filaments().iter().map(|f| f.map(|f| bp_3mf::filament::for_slot(&f.kind, &d.printer))).collect();
    let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let (volumes_mm3, _) = dto::flush_matrix(d);
    let model = bp_3mf::Model {
        plate_count: 1,
        name: &name,
        piece: bp_3mf::ZipText::ES.piece,
        parts,
        filaments,
        slots,
        thumbnail: None,
        printer: d.printer.clone(),
        flush: bp_3mf::Flush {
            volumes_mm3,
            multiplier: prefs.waste.flush_multiplier,
            into_infill: prefs.waste.flush_reuse > 0.0,
            prime_mm3: prefs.waste.prime_mm3,
        },
        layer_mm: prefs.waste.layer_mm,
        offset_mm: d.offset_mm,
    };
    let file = std::fs::File::create(path).map_err(|e| AppError::file(path, e))?;
    let out = std::io::BufWriter::new(file);
    let result = bp_3mf::write_filament_parts(out, &model, part_filaments);
    result.map_err(|e| AppError::with("error.3mfWrite", json!({ "detail": e.to_string() })))
}

pub(crate) fn printability_for_doc(
    d: &Document,
    prefs: &Prefs,
    params: bp_core::printability::Params,
) -> bp_core::printability::Report {
    let project = &d.project;
    let mut report = project.printability(params);
    report.printer = Some(d.printer.profile.id.clone());
    let stats = project.stats();
    let size = dto::printed_size(project, &stats);
    let (_, plate) = dto::waste_and_plate(d, prefs, &stats.swaps, size);
    let fits = plate.fits && size[2] <= d.printer.max_height_mm();
    report.checks.push(bp_core::printability::Check::new(
        "buildVolume",
        "checked",
        usize::from(!fits),
        "error",
        // Footprint, margin, tower and height all decide this; one number would mislead.
        None,
        None,
        vec![],
    ));
    report
}

/// Runs only on request; large reports never slow down every painting summary.
#[tauri::command]
pub async fn analyze_printability(
    app: AppHandle,
    params: bp_core::printability::Params,
) -> CmdResult<bp_core::printability::Report> {
    if !params.valid() {
        return Err(AppError::new("error.printabilityParams"));
    }
    with_doc(app, move |d, prefs| Ok(printability_for_doc(d, prefs, params))).await
}

/// The mesh operations a printability fix (or its revert) changes; the rest stay.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshFix {
    repair: Option<bool>,
    orient: Option<bool>,
    fill_holes: Option<bool>,
    split_vertices: Option<bool>,
    deep_clean: Option<bool>,
    drop_floating: Option<bool>,
    min_piece_mm: Option<f32>,
    flatten_mm: Option<f32>,
    thicken_mm: Option<f32>,
}

/// One undoable step, so Ctrl+Z restores geometry, colours and hand painting.
#[tauri::command]
pub async fn fix_mesh(app: AppHandle, fix: MeshFix) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        let now = d.project.mesh_ops();
        d.project.fix_mesh(MeshOps {
            repair: fix.repair.unwrap_or(now.repair),
            orient: fix.orient.unwrap_or(now.orient),
            fill_holes: fix.fill_holes.unwrap_or(now.fill_holes),
            split_vertices: fix.split_vertices.unwrap_or(now.split_vertices),
            deep_clean: fix.deep_clean.unwrap_or(now.deep_clean),
            drop_floating: fix.drop_floating.unwrap_or(now.drop_floating),
            min_piece_mm: fix.min_piece_mm.map_or(now.min_piece_mm, |v| v.clamp(0.0, 100.0)),
            flatten_mm: fix.flatten_mm.map_or(now.flatten_mm, |v| v.clamp(0.0, 20.0)),
            // NaN goes to 0: nothing thickened.
            thicken_mm: fix.thicken_mm.map_or(now.thicken_mm, |v| if v.is_finite() { v.clamp(0.0, 10.0) } else { 0.0 }),
            ..now
        });
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Hollows the model to a wall of `wall_mm` with `drains` holes of `drain_mm`
/// through the base, as one undoable step; a wall of 0 fills it again. The wall
/// is at least two lines of the nozzle.
#[tauri::command]
pub async fn hollow_mesh(app: AppHandle, wall_mm: f32, drains: u8, drain_mm: f32) -> CmdResult<Summary> {
    if !wall_mm.is_finite()
        || !(0.0..=50.0).contains(&wall_mm)
        || drains > bp_core::mesh::MAX_DRAINS
        || !(1.0..=20.0).contains(&drain_mm)
    {
        return Err(AppError::new("error.hollowParams"));
    }
    with_doc(app, move |d, prefs| {
        let min = 2.0 * d.printer.nozzle_mm;
        if wall_mm > 0.0 && wall_mm < min - 1e-4 {
            return Err(AppError::with("error.hollowWall", json!({ "min": min })));
        }
        let now = d.project.mesh_ops();
        d.project.fix_mesh(MeshOps { hollow_mm: wall_mm, drains, drain_mm, ..now });
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Subdivides and smooths the model as one undoable step. Refused when the
/// result would pass `max_triangles` (the panel's cap).
#[tauri::command]
pub async fn smooth_mesh(
    app: AppHandle,
    subdivide: u8,
    smooth: u8,
    crease_deg: f32,
    max_triangles: usize,
) -> CmdResult<Summary> {
    if subdivide > bp_core::mesh::MAX_SUBDIVIDE || smooth > 20 || !(0.0..=180.0).contains(&crease_deg) {
        return Err(AppError::new("error.smoothParams"));
    }
    let cap = max_triangles.clamp(1, bp_core::mesh::SUBDIVIDE_LIMIT);
    with_doc(app, move |d, prefs| {
        let now = d.project.mesh_ops();
        let base = d.project.stats().triangles / 4usize.pow(now.subdivide as u32);
        let result = base * 4usize.pow(subdivide as u32);
        if result > cap {
            return Err(AppError::with("error.subdivideCap", json!({ "n": result, "cap": cap })));
        }
        d.project.fix_mesh(MeshOps { subdivide, smooth, crease_deg, ..now });
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// What each advanced repair finds before and after; on request, since it
/// builds the mesh again without them.
#[tauri::command]
pub async fn mesh_issues(app: AppHandle) -> CmdResult<dto::MeshIssuesDto> {
    with_doc(app, |d, _| {
        let (before, after) = d.project.mesh_issues();
        Ok(dto::MeshIssuesDto { before: before.into(), after: after.into() })
    })
    .await
}

/// The turn with the least overhang and what it gains, without turning; the
/// panel shows it and `set_rotation` applies it.
#[tauri::command]
pub async fn plan_orientation(app: AppHandle, overhang_deg: f32) -> CmdResult<dto::OrientationDto> {
    if !overhang_deg.is_finite() {
        return Err(AppError::new("error.printabilityParams"));
    }
    with_doc(app, move |d, _| {
        let p = d.project.orientation_plan(overhang_deg.clamp(0.0, 90.0));
        Ok(dto::OrientationDto {
            changes: p.rotation != d.project.rotation(),
            rotation: p.rotation,
            down: p.down,
            overhang_mm2: [p.now.0, p.after.0],
            contact_mm2: [p.now.1, p.after.1],
        })
    })
    .await
}

/// Turns the model to the orientation with the least overhang; one undoable step.
#[tauri::command]
pub async fn orient_for_print(app: AppHandle, overhang_deg: f32) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        let r = d.project.best_rotation(overhang_deg.clamp(0.0, 90.0));
        d.project.set_rotation(r);
        Ok(dto::summary(d, prefs))
    })
    .await
}

/// Sets the model's rotation from load (rows of a 3×3 matrix); reverts a turn.
#[tauri::command]
pub async fn set_rotation(app: AppHandle, rotation: bp_core::Mat3) -> CmdResult<Summary> {
    // Only a turn: rows of unit length at right angles, not a mirror. Anything else would warp the model.
    let dot = |a: usize, b: usize| (0..3).map(|k| rotation[a][k] * rotation[b][k]).sum::<f32>();
    let det = bp_core::geom::dot(rotation[0], bp_core::geom::cross(rotation[1], rotation[2]));
    let turn = (0..3).all(|a| (0..3).all(|b| (dot(a, b) - if a == b { 1.0 } else { 0.0 }).abs() < 1e-3));
    if rotation.iter().flatten().any(|v| !v.is_finite()) || !turn || (det - 1.0).abs() > 1e-3 {
        return Err(AppError::new("error.printabilityParams"));
    }
    with_doc(app, move |d, prefs| {
        d.project.set_rotation(rotation);
        Ok(dto::summary(d, prefs))
    })
    .await
}
