//! Tauri commands: thin adapters from the UI to the core use cases. Anything
//! that rebuilds the painting runs off the main thread.

use std::path::{Path, PathBuf};

use bp_core::{ColorOps, MeshOps, PrintOps};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{self, HitDto, PaintDto, PrintersDto, Summary};
use crate::profiles::Store;
use crate::state::{AppState, Document};

type CmdResult<T> = Result<T, String>;

/// Runs `f` on the open document in a blocking worker.
async fn with_doc<R: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&mut Document) -> CmdResult<R> + Send + 'static,
) -> CmdResult<R> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut guard = state.doc.lock().map_err(|_| "El estado quedó bloqueado por un error anterior.".to_string())?;
        let doc = guard.as_mut().ok_or("No hay ningún archivo abierto.")?;
        f(doc)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What the core prepares for: the nozzle and the layer height it prints at.
fn print_ops(p: &bp_3mf::Printer) -> PrintOps {
    PrintOps { nozzle_mm: p.nozzle_mm, layer_mm: p.layer_mm() }
}

fn with_store<R>(app: &AppHandle, f: impl FnOnce(&mut Store) -> CmdResult<R>) -> CmdResult<R> {
    let state = app.state::<AppState>();
    let mut s = state.printers.lock().map_err(|e| e.to_string())?;
    f(&mut s)
}

/// Applies the store's pick to the open document, if there is one.
async fn apply_printer(app: AppHandle) -> CmdResult<Option<Summary>> {
    let printer = with_store(&app, |s| Ok(s.printer()))?;
    if app.state::<AppState>().doc.lock().map_err(|e| e.to_string())?.is_none() {
        return Ok(None);
    }
    with_doc(app, move |d| {
        d.project.set_print_ops(print_ops(&printer));
        d.printer = printer;
        Ok(Some(dto::summary(d)))
    })
    .await
}

fn blender_path(app: &AppHandle) -> CmdResult<PathBuf> {
    let state = app.state::<AppState>();
    let mut b = state.blender.lock().map_err(|e| e.to_string())?;
    if b.is_none() {
        *b = bp_blend::find_blender();
    }
    b.clone().ok_or_else(|| bp_blend::BlendError::BlenderNotFound.to_string())
}

#[tauri::command]
pub fn blender_status(app: AppHandle) -> Option<String> {
    blender_path(&app).ok().map(|p| p.display().to_string())
}

/// A `.blend` passed on the command line ("Open with…"), if any.
#[tauri::command]
pub fn initial_file() -> Option<String> {
    std::env::args().skip(1).find(|a| a.to_lowercase().ends_with(".blend"))
}

#[tauri::command]
pub fn set_blender_path(app: AppHandle, path: String) -> CmdResult<String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("{path} no es un archivo."));
    }
    *app.state::<AppState>().blender.lock().map_err(|e| e.to_string())? = Some(p);
    Ok(path)
}

#[tauri::command]
pub async fn open_blend(app: AppHandle, path: String) -> CmdResult<Summary> {
    let blender = blender_path(&app)?;
    let printer = with_store(&app, |s| Ok(s.printer()))?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let file = Path::new(&path);
        let size = std::fs::metadata(file).map_err(|e| format!("{path}: {e}"))?.len();
        let emit = |msg: &str| {
            let _ = handle.emit("progress", msg.to_string());
        };
        emit("Abriendo Blender");
        let scene = bp_blend::extract(&blender, file, &mut |m| emit(m)).map_err(|e| e.to_string())?;
        emit("Agrupando colores en filamentos");
        let mut project = bp_core::Project::new(scene, 4);
        project.set_print_ops(print_ops(&printer));
        let doc = Document {
            project,
            file_name: file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone()),
            file_size: size,
            printer,
        };
        let summary = dto::summary(&doc);
        *handle.state::<AppState>().doc.lock().map_err(|e| e.to_string())? = Some(doc);
        Ok(summary)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_palette(app: AppHandle, colors: Vec<String>) -> CmdResult<Summary> {
    let parsed: Vec<_> = colors
        .iter()
        .map(|c| bp_core::color::parse_hex(c).ok_or(format!("{c} no es un color #RRGGBB.")))
        .collect::<Result<_, _>>()?;
    with_doc(app, move |d| {
        d.project.set_palette(parsed);
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn suggest_palette(app: AppHandle, count: usize) -> CmdResult<Summary> {
    with_doc(app, move |d| {
        let pal = d.project.suggest_palette(count);
        d.project.set_palette(pal);
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn set_height(app: AppHandle, mm: f32) -> CmdResult<Summary> {
    if !(1.0..=1000.0).contains(&mm) {
        return Err("La altura debe estar entre 1 y 1000 mm.".into());
    }
    with_doc(app, move |d| {
        d.project.set_height_mm(mm);
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn reduce_palette(app: AppHandle, target: usize) -> CmdResult<Summary> {
    with_doc(app, move |d| {
        d.project.reduce_palette(target);
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn merge_slots(app: AppHandle, from: usize, into: usize) -> CmdResult<Summary> {
    with_doc(app, move |d| {
        d.project.merge_slots(from, into);
        Ok(dto::summary(d))
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
    let ops = MeshOps {
        keep_ratio: keep_ratio.clamp(0.05, 1.0),
        flatten_mm: flatten_mm.max(0.0),
        repair,
        min_piece_mm: min_piece_mm.max(0.0),
    };
    with_doc(app, move |d| {
        d.project.set_mesh_ops(ops);
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn set_color_ops(app: AppHandle, border_detail: u8, min_island_mm2: f32) -> CmdResult<Summary> {
    let ops = ColorOps { border_detail, min_island_mm2: min_island_mm2.max(0.0) };
    with_doc(app, move |d| {
        d.project.set_color_ops(ops);
        Ok(dto::summary(d))
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
    with_store(&app, |s| {
        s.select(&id, nozzle_mm)?;
        s.save()
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

/// The paintable mesh as one binary blob:
/// `u32 leaves, u32 tris | f32 positions[9 * leaves] | u32 offsets[tris + 1] | u8 labels[leaves] | u8 rgb[3 * leaves]`.
#[tauri::command]
pub async fn preview_mesh(app: AppHandle) -> CmdResult<Response> {
    let bytes = with_doc(app, |d| {
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
    with_doc(app, move |d| Ok(d.project.pick(origin, dir).map(|h| HitDto { point: h.point, normal: h.normal }))).await
}

#[tauri::command]
pub async fn paint(
    app: AppHandle,
    origin: [f32; 3],
    dir: [f32; 3],
    radius_mm: f32,
    slot: u8,
) -> CmdResult<Option<PaintDto>> {
    with_doc(app, move |d| {
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
    with_doc(app, |d| {
        d.project.end_stroke();
        Ok(dto::summary(d))
    })
    .await
}

#[tauri::command]
pub async fn undo(app: AppHandle) -> CmdResult<Summary> {
    with_doc(app, |d| {
        d.project.undo();
        Ok(dto::summary(d))
    })
    .await
}

/// One STL per colour in a ZIP. Returns how many colour files were written.
#[tauri::command]
pub async fn export_color_zip(app: AppHandle, path: String, thickness_mm: f32, main_solid: bool) -> CmdResult<usize> {
    if !(0.2..=20.0).contains(&thickness_mm) {
        return Err("El grosor debe estar entre 0,2 y 20 mm.".into());
    }
    with_doc(app, move |d| {
        let pieces = d.project.color_pieces(thickness_mm, main_solid);
        let hexes: Vec<String> =
            pieces.iter().map(|p| bp_core::color::to_hex(d.project.palette()[p.slot as usize])).collect();
        let files: Vec<bp_3mf::ColorFile> = pieces
            .iter()
            .zip(&hexes)
            .map(|(p, hex)| bp_3mf::ColorFile { hex, positions: &p.positions, tris: &p.tris, solid: p.solid })
            .collect();
        let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let file = std::fs::File::create(&path).map_err(|e| format!("{path}: {e}"))?;
        bp_3mf::write_color_zip(std::io::BufWriter::new(file), &name, &files)
            .map_err(|e| format!("No se pudo escribir el ZIP: {e}"))?;
        Ok(files.len())
    })
    .await
}

#[tauri::command]
pub async fn export_3mf(app: AppHandle, path: String) -> CmdResult<()> {
    with_doc(app, move |d| {
        let parts = d.project.export_parts();
        let filaments = d.project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect();
        let name = Path::new(&d.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let model = bp_3mf::Model {
            name: &name,
            parts: parts
                .iter()
                .map(|p| bp_3mf::Part { name: &p.name, positions: &p.positions, tris: &p.tris, paint: &p.paint })
                .collect(),
            filaments,
            printer: d.printer.clone(),
        };
        let file = std::fs::File::create(&path).map_err(|e| format!("{path}: {e}"))?;
        bp_3mf::write(std::io::BufWriter::new(file), &model).map_err(|e| format!("No se pudo escribir el 3MF: {e}"))
    })
    .await
}
