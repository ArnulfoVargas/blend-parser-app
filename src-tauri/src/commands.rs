//! Tauri commands: thin adapters from the UI to the core use cases. Anything
//! that rebuilds the painting runs off the main thread. Errors go out as keys
//! (see `error.rs`) so the UI words them in the user's language.

use std::path::{Path, PathBuf};

use bp_core::{ColorOps, MeshOps, PrintOps};
use serde_json::json;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{self, HitDto, PaintDto, PrefsDto, PrintersDto, ProgressDto, Summary, TurnDto};
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
fn print_ops(p: &bp_3mf::Printer, prefs: &Prefs) -> PrintOps {
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

/// A supported model passed on the command line ("Open with…"), if any.
#[tauri::command]
pub fn initial_file() -> Option<String> {
    std::env::args().skip(1).find(|a| bp_blend::supports(Path::new(a)))
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
        let doc = Document {
            project,
            file_name: file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone()),
            file_size: size,
            import_warnings: imported.warnings,
            printer,
            flush: Default::default(),
            offset_mm: [0.0; 2],
        };
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

#[tauri::command]
pub async fn suggest_palette(app: AppHandle, count: usize) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        let pal = d.project.suggest_palette(count);
        d.project.set_palette(pal);
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
        d.project.cut_plane(point, normal, keep_low, keep_other, lay_on_cut, &selected).map_err(|e| {
            AppError::new(match e {
                bp_core::cut::CutError::Misses => "error.cutMisses",
                bp_core::cut::CutError::InvalidPlane => "error.cutPlane",
                bp_core::cut::CutError::InvalidGeometry => "error.cutGeometry",
            })
        })?;
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
pub async fn set_mesh_ops(
    app: AppHandle,
    keep_ratio: f32,
    flatten_mm: f32,
    repair: bool,
    min_piece_mm: f32,
) -> CmdResult<Summary> {
    with_doc(app, move |d, prefs| {
        // The settings panel has no say over the printability fixes; keep them.
        let ops = MeshOps {
            keep_ratio: keep_ratio.clamp(0.05, 1.0),
            flatten_mm: flatten_mm.max(0.0),
            repair,
            min_piece_mm: min_piece_mm.max(0.0),
            ..d.project.mesh_ops()
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
    let bytes = with_doc(app, |d, _| {
        let pv = d.project.preview();
        let leaves = pv.labels.len();
        let tris = pv.offsets.len() - 1;
        let mut out = Vec::with_capacity(8 + leaves * (36 + 4) + tris * 4);
        out.extend((leaves as u32).to_le_bytes());
        out.extend((tris as u32).to_le_bytes());
        for v in &pv.positions {
            out.extend(v.to_le_bytes());
        }
        for o in &pv.offsets {
            out.extend(o.to_le_bytes());
        }
        out.extend(&pv.labels);
        out.extend(&pv.source_rgb);
        Ok(out)
    })
    .await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn pick(app: AppHandle, origin: [f32; 3], dir: [f32; 3]) -> CmdResult<Option<HitDto>> {
    with_doc(app, move |d, _| Ok(d.project.pick(origin, dir).map(|h| HitDto { point: h.point, normal: h.normal })))
        .await
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

/// One STL per colour in a ZIP, its colour list in `lang`. Returns how many colour files were written.
#[tauri::command]
pub async fn export_color_zip(
    app: AppHandle,
    path: String,
    thickness_mm: f32,
    main_solid: bool,
    lang: String,
) -> CmdResult<usize> {
    in_range(thickness_mm, 0.2..=20.0, "error.shellRange")?;
    with_doc(app, move |d, _| {
        let pieces = d.project.color_pieces(thickness_mm, main_solid);
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
pub async fn export_3mf(app: AppHandle, path: String) -> CmdResult<()> {
    with_doc(app, move |d, prefs| {
        let parts = d.project.export_parts();
        let filaments = d.project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect();
        let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (volumes_mm3, _) = dto::flush_matrix(d);
        let model = bp_3mf::Model {
            name: &name,
            parts: parts
                .iter()
                .map(|p| bp_3mf::Part {
                    name: &p.name,
                    positions: &p.positions,
                    tris: &p.tris,
                    paint: &p.paint,
                    object: p.object,
                })
                .collect(),
            filaments,
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
        let file = std::fs::File::create(&path).map_err(|e| AppError::file(&path, e))?;
        bp_3mf::write(std::io::BufWriter::new(file), &model)
            .map_err(|e| AppError::with("error.3mfWrite", json!({ "detail": e.to_string() })))
    })
    .await
}

fn printability_for_doc(
    d: &Document,
    prefs: &Prefs,
    params: bp_core::printability::Params,
) -> bp_core::printability::Report {
    let project = &d.project;
    let mut report = project.printability(params);
    report.printer = Some(d.printer.profile.id.clone());
    let stats = project.stats();
    let (_, plate) = dto::waste_and_plate(d, prefs, &stats.swaps, stats.size_mm);
    let fits = plate.fits && stats.size_mm[2] <= d.printer.max_height_mm();
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
            ..now
        });
        Ok(dto::summary(d, prefs))
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
    if rotation.iter().flatten().any(|v| !v.is_finite()) {
        return Err(AppError::new("error.printabilityParams"));
    }
    with_doc(app, move |d, prefs| {
        d.project.set_rotation(rotation);
        Ok(dto::summary(d, prefs))
    })
    .await
}
