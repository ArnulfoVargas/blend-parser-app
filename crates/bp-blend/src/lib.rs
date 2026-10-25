//! Reads multicolour model formats by asking Blender itself: Blender evaluates modifiers and
//! material node trees, `extract.py` dumps the result, and this crate turns the
//! dump into a [`bp_core::Scene`]. Everything after that happens in Rust.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bp_core::{ColorSource, Image, MaterialInfo, Part, Scene};
use serde::{Deserialize, Serialize};

pub mod companions;

const SCRIPT: &str = include_str!("extract.py");

#[derive(Debug, thiserror::Error)]
pub enum BlendError {
    #[error("Formato de entrada no compatible: {0}")]
    Unsupported(String),
    #[error("No encontré Blender. Instálalo o indica dónde está blender.exe.")]
    BlenderNotFound,
    #[error("No se pudo leer o escribir un archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("Blender no pudo abrir el archivo:\n{0}")]
    Blender(String),
    #[error("El volcado de Blender no tiene el formato esperado: {0}")]
    Format(String),
    #[error("El .blend no tiene mallas visibles.")]
    Empty,
}

/// Looks for Blender: `BLENDER_PATH`, then the usual install locations, then `PATH`.
pub fn find_blender() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BLENDER_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for root in [r"C:\Program Files\Blender Foundation", r"C:\Program Files (x86)\Blender Foundation"] {
            if let Ok(dirs) = std::fs::read_dir(root) {
                let mut versions: Vec<PathBuf> =
                    dirs.flatten().map(|d| d.path().join("blender.exe")).filter(|p| p.is_file()).collect();
                // Newest version last, so it wins.
                versions.sort_by_key(|p| version_key(p));
                candidates.extend(versions.into_iter().rev());
            }
        }
        candidates.push(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Blender\blender.exe"));
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/Blender.app/Contents/MacOS/Blender"));
    } else {
        candidates.extend(["/usr/bin/blender", "/usr/local/bin/blender", "/snap/bin/blender"].map(PathBuf::from));
    }
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    let exe = if cfg!(windows) { "blender.exe" } else { "blender" };
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()))
}

fn version_key(p: &Path) -> Vec<u32> {
    let dir = p.parent().and_then(|d| d.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    dir.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect()
}

#[derive(Deserialize)]
struct Dump {
    #[serde(default)]
    warnings: Vec<ImportWarning>,
    images: Vec<DumpImage>,
    materials: Vec<DumpMaterial>,
    parts: Vec<DumpPart>,
}

/// Stable keys translated by the desktop client; resource names remain literal.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportWarning {
    pub code: String,
    pub detail: String,
}

pub struct ImportResult {
    pub scene: Scene,
    pub warnings: Vec<ImportWarning>,
}

pub const INPUT_EXTENSIONS: &[&str] = &["blend", "obj", "fbx", "gltf", "glb", "ply"];

pub fn supports(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| INPUT_EXTENSIONS.contains(&s.to_ascii_lowercase().as_str()))
}

#[derive(Deserialize)]
struct DumpImage {
    name: String,
    width: u32,
    height: u32,
    file: String,
}

#[derive(Deserialize)]
struct DumpMaterial {
    name: String,
    kind: String,
    detail: String,
    faces: u32,
}

#[derive(Deserialize)]
struct DumpPart {
    name: String,
    vertices: usize,
    triangles: usize,
    loops: usize,
    positions: String,
    tris: String,
    tri_loops: String,
    tri_slot: String,
    uv_layers: Vec<String>,
    color_layers: Vec<String>,
    sources: Vec<DumpSource>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum DumpSource {
    Flat { rgb: [f32; 3] },
    Image { image: u32, uv: u32 },
    Vcol { attr: u32 },
}

/// Runs Blender on `blend` and returns its scene. `progress` receives each step's name.
pub fn extract(blender: &Path, blend: &Path, progress: &mut dyn FnMut(&str)) -> Result<Scene, BlendError> {
    extract_with_report(blender, blend, progress).map(|r| r.scene)
}

/// Imports a supported model and reports lossy colour approximations and missing resources.
pub fn extract_with_report(
    blender: &Path,
    source: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<ImportResult, BlendError> {
    extract_with_files(blender, source, &Additions::default(), progress)
}

/// What the user added before converting (see [`companions`]).
#[derive(Clone, Debug, Default)]
pub struct Additions {
    /// Files for what the model references; folders stand for the files inside them.
    pub files: Vec<PathBuf>,
    /// An image for each named material whose texture the model does not name.
    pub textures: Vec<(String, PathBuf)>,
    /// An OBJ material library to use instead of the ones the OBJ names.
    pub library: Option<PathBuf>,
    /// A .blend whose packed textures go to materials of the same name.
    pub blend: Option<PathBuf>,
}

/// As [`extract_with_report`], with what the user added.
pub fn extract_with_files(
    blender: &Path,
    source: &Path,
    additions: &Additions,
    progress: &mut dyn FnMut(&str),
) -> Result<ImportResult, BlendError> {
    if !supports(source) {
        return Err(BlendError::Unsupported(source.display().to_string()));
    }
    let source = source.canonicalize()?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("blendparser-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let extras = companions::expand(&additions.files);
    let textures: serde_json::Map<String, serde_json::Value> =
        additions.textures.iter().map(|(m, p)| (m.clone(), p.display().to_string().into())).collect();
    let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string());
    let added = serde_json::json!({ "textures": textures, "library": path(&additions.library), "blend": path(&additions.blend) });
    let result = std::fs::write(dir.join("additions.json"), added.to_string())
        .map_err(BlendError::from)
        .and_then(|_| run(blender, &source, &extras, &dir, progress))
        .and_then(|_| read_dump(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run(
    blender: &Path,
    blend: &Path,
    extras: &[PathBuf],
    dir: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<(), BlendError> {
    let script = dir.join("extract.py");
    std::fs::write(&script, SCRIPT)?;
    let mut cmd = Command::new(blender);
    cmd.arg("-b")
        .arg("--factory-startup")
        .arg("--disable-autoexec")
        .arg("--python-exit-code")
        .arg("1")
        .arg("--python")
        .arg(&script)
        .arg("--")
        .arg(dir)
        .arg(blend)
        .args(extras)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| BlendError::Blender(format!("{}: {e}", blender.display())))?;
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                s.push_str(&line);
                s.push('\n');
            }
        }
        s
    });
    let mut tail: Vec<String> = Vec::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(msg) = line.strip_prefix("BP_PROGRESS ") {
                progress(msg);
            } else {
                tail.push(line);
                if tail.len() > 20 {
                    tail.remove(0);
                }
            }
        }
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() || !dir.join("scene.json").is_file() {
        let log = format!("{}\n{}", tail.join("\n"), err);
        return Err(BlendError::Blender(
            log.trim().chars().rev().take(2000).collect::<String>().chars().rev().collect(),
        ));
    }
    Ok(())
}

fn read_vec<T: Copy, const N: usize>(path: &Path, count: usize, conv: fn([u8; N]) -> T) -> Result<Vec<T>, BlendError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() != count * N {
        return Err(BlendError::Format(format!(
            "{} mide {} bytes, esperaba {}",
            path.display(),
            bytes.len(),
            count * N
        )));
    }
    Ok(bytes.as_chunks::<N>().0.iter().map(|c| conv(*c)).collect())
}

fn f32s<const K: usize>(path: &Path, count: usize) -> Result<Vec<[f32; K]>, BlendError> {
    let flat = read_vec::<f32, 4>(path, count * K, f32::from_le_bytes)?;
    Ok(flat.as_chunks::<K>().0.to_vec())
}

fn u32x3(path: &Path, count: usize) -> Result<Vec<[u32; 3]>, BlendError> {
    let flat = read_vec::<u32, 4>(path, count * 3, u32::from_le_bytes)?;
    Ok(flat.as_chunks::<3>().0.to_vec())
}

fn read_dump(dir: &Path) -> Result<ImportResult, BlendError> {
    let json = std::fs::read_to_string(dir.join("scene.json"))?;
    let dump: Dump = serde_json::from_str(&json).map_err(|e| BlendError::Format(e.to_string()))?;
    let images = dump
        .images
        .into_iter()
        .map(|i| {
            let rgb = std::fs::read(dir.join(&i.file))?;
            if rgb.len() != (i.width * i.height * 3) as usize {
                return Err(BlendError::Format(format!("textura {} incompleta", i.name)));
            }
            Ok(Image { name: i.name, width: i.width, height: i.height, rgb })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut parts = Vec::with_capacity(dump.parts.len());
    for p in dump.parts {
        let tris = u32x3(&dir.join(&p.tris), p.triangles)?;
        let tri_loops = u32x3(&dir.join(&p.tri_loops), p.triangles)?;
        if tris.iter().flatten().any(|&v| v as usize >= p.vertices)
            || tri_loops.iter().flatten().any(|&l| l as usize >= p.loops)
        {
            return Err(BlendError::Format(format!("{}: índices fuera de rango", p.name)));
        }
        let sources: Vec<ColorSource> = p
            .sources
            .into_iter()
            .map(|s| match s {
                DumpSource::Flat { rgb } => ColorSource::Flat(rgb),
                DumpSource::Image { image, uv } => ColorSource::Image { image, uv },
                DumpSource::Vcol { attr } => ColorSource::VertexColor { attr },
            })
            .collect();
        let uv_layers = p.uv_layers.iter().map(|f| f32s::<2>(&dir.join(f), p.loops)).collect::<Result<Vec<_>, _>>()?;
        let color_layers =
            p.color_layers.iter().map(|f| f32s::<3>(&dir.join(f), p.loops)).collect::<Result<Vec<_>, _>>()?;
        for s in &sources {
            let ok = match *s {
                ColorSource::Image { image, uv } => (image as usize) < images.len() && (uv as usize) < uv_layers.len(),
                ColorSource::VertexColor { attr } => (attr as usize) < color_layers.len(),
                ColorSource::Flat(_) => true,
            };
            if !ok {
                return Err(BlendError::Format(format!("{}: material con referencias rotas", p.name)));
            }
        }
        parts.push(Part {
            name: p.name,
            positions: f32s::<3>(&dir.join(&p.positions), p.vertices)?,
            tris,
            tri_loops,
            tri_source: read_vec::<u16, 2>(&dir.join(&p.tri_slot), p.triangles, u16::from_le_bytes)?,
            sources,
            uv_layers,
            color_layers,
            object: 0,
        });
    }
    if parts.is_empty() {
        return Err(BlendError::Empty);
    }
    let materials = dump
        .materials
        .into_iter()
        .map(|m| MaterialInfo { name: m.name, kind: m.kind, detail: m.detail, faces: m.faces })
        .collect();
    Ok(ImportResult { scene: Scene { parts, images, materials }, warnings: dump.warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against a real model with Blender installed:
    /// `BP_SAMPLE_BLEND=ruta/al/modelo.blend cargo test --release -p bp-blend -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn extracts_the_sample_blend() {
        let Some(blend) = std::env::var_os("BP_SAMPLE_BLEND").map(std::path::PathBuf::from) else {
            eprintln!("BP_SAMPLE_BLEND no está definido; se omite.");
            return;
        };
        let blender = find_blender().expect("Blender");
        let mut steps = vec![];
        let scene = extract(&blender, &blend, &mut |s| steps.push(s.to_string())).unwrap();
        assert!(!scene.parts.is_empty() && scene.triangle_count() > 0);
        assert!(!steps.is_empty());
        // The spider model used during development, when that is the sample.
        if blend.to_string_lossy().contains("spider_hito2v5") {
            assert_eq!(scene.parts.len(), 2);
            assert_eq!(scene.triangle_count(), 145_054 + 181_702);
            assert_eq!(scene.images.len(), 9);
        }

        let t = std::time::Instant::now();
        let mut project = bp_core::Project::new(scene, 4);
        eprintln!("project in {:?}", t.elapsed());
        assert_eq!(project.palette().len(), 4);

        let report = |label: &str, p: &bp_core::Project| {
            let s = p.stats();
            eprintln!(
                "{label:<28} tris {:>7}  size {:>5.1}x{:>5.1}x{:>5.1}  contact {:>7.0} mm2  swaps {:>4}  multicolor layers {:>3}/{:<3}",
                s.triangles, s.size_mm[0], s.size_mm[1], s.size_mm[2], s.contact_mm2, s.swaps.swaps, s.swaps.multicolor_layers, s.swaps.layers
            );
            s
        };
        let t = std::time::Instant::now();
        project.set_height_mm(75.0);
        let base = report(&format!("75 mm ({:?})", t.elapsed()), &project);
        assert!((base.size_mm[2] - 75.0).abs() < 0.01);

        let t = std::time::Instant::now();
        project.set_color_ops(bp_core::ColorOps { border_detail: 5, min_island_mm2: 4.0 });
        let clean = report(&format!("+ islands < 4 mm2 ({:?})", t.elapsed()), &project);
        assert!(clean.swaps.swaps <= base.swaps.swaps);

        let t = std::time::Instant::now();
        project.set_mesh_ops(bp_core::MeshOps { keep_ratio: 1.0, flatten_mm: 1.0, ..Default::default() });
        let flat = report(&format!("+ flatten 1 mm ({:?})", t.elapsed()), &project);
        assert!((flat.size_mm[2] - 74.0).abs() < 0.01);

        let t = std::time::Instant::now();
        project.set_mesh_ops(bp_core::MeshOps { keep_ratio: 0.4, flatten_mm: 1.0, ..Default::default() });
        let lite = report(&format!("+ keep 40% ({:?})", t.elapsed()), &project);
        assert!(lite.triangles < flat.triangles / 2);

        // The waste estimate, with the flush per pair that also goes into the 3MF.
        let pal = project.palette().to_vec();
        let n = pal.len();
        let matrix: Vec<f32> =
            pal.iter().flat_map(|&a| pal.iter().map(move |&b| bp_core::waste::default_flush(a, b, 107.0))).collect();
        let params = bp_core::WasteParams { tower: n > 1, ..Default::default() };
        let w = bp_core::waste::waste(&lite.swaps, &params, |a, b| matrix[a as usize * n + b as usize]);
        eprintln!(
            "waste: flush {:.1} g, start {:.1} g, tower {:.1} g ({:.0}x{:.0} mm, {:.1} mm tall), total {:.1} g / {:.2} m",
            w.flush_mm3 * 1.24e-3,
            w.start_mm3 * 1.24e-3,
            w.tower_mm3 * 1.24e-3,
            w.tower_footprint_mm[0],
            w.tower_footprint_mm[1],
            w.tower_height_mm,
            w.total_g,
            w.total_m
        );
        assert!(w.total_g > 0.0 && w.flush_mm3 > w.tower_mm3);

        // Export and check every paint_color decodes.
        let parts = project.export_parts();
        let filaments: Vec<String> = project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect();
        let model = bp_3mf::Model {
            name: "spider",
            piece: "piece",
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
            slots: vec![],
            printer: bp_3mf::Printer::default(),
            flush: bp_3mf::Flush { volumes_mm3: matrix, ..Default::default() },
            layer_mm: None,
            offset_mm: [0.0; 2],
            thumbnail: None,
        };
        let out = std::env::temp_dir().join("blendparser-integration.3mf");
        bp_3mf::write(std::fs::File::create(&out).unwrap(), &model).unwrap();
        for p in &parts {
            assert!(p.paint.iter().all(|c| bp_core::encode::decode(c).is_ok()));
        }
        eprintln!("3MF: {} ({} KB)", out.display(), std::fs::metadata(&out).unwrap().len() / 1024);

        // 10 detected colours reduced to 5 by closeness.
        let pal10 = project.suggest_palette(10);
        project.set_palette(pal10);
        let ten = report("10 colours", &project);
        let t = std::time::Instant::now();
        project.reduce_palette(5);
        let five = report(&format!("reduced to 5 ({:?})", t.elapsed()), &project);
        assert_eq!(project.palette().len(), 5);
        assert!(five.swaps.swaps <= ten.swaps.swaps);
        eprintln!("palette 5: {:?}", project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect::<Vec<_>>());

        // Solid pieces per colour.
        for (voxel_mm, depth_mm) in [(0.5, 0.0), (0.5, 3.0)] {
            let t = std::time::Instant::now();
            let params = bp_core::SplitParams { voxel_mm, depth_mm, ..Default::default() };
            let split = project.split_by_color(params);
            let closed = split.pieces.iter().filter(|p| bp_core::split::watertight(&p.tris)).count();
            eprintln!(
                "split {voxel_mm} mm, depth {depth_mm} in {:?}: voxel {:.2} mm, {} pieces ({closed} sealed), {} tris, per slot {:?}",
                t.elapsed(),
                split.voxel_mm,
                split.pieces.len(),
                split.pieces.iter().map(|p| p.tris.len()).sum::<usize>(),
                (0..5).map(|s| split.pieces.iter().filter(|p| p.slot == s).count()).collect::<Vec<_>>()
            );
            assert_eq!(closed, split.pieces.len());

            let t = std::time::Instant::now();
            let kit = bp_core::kit::kit(split, bp_3mf::Printer::default().bed_mm());
            let palette: Vec<String> = project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect();
            let info =
                bp_3mf::KitInfo { name: "spider", palette: &palette, slots: &[], printer: &bp_3mf::Printer::default() };
            let zip = std::env::temp_dir().join(format!("blendparser-piezas-{depth_mm}.zip"));
            let summary = bp_3mf::write_kit_zip(std::fs::File::create(&zip).unwrap(), &kit, &info).unwrap();
            eprintln!(
                "kit zip in {:?}: {summary:?}, {} KB, {}",
                t.elapsed(),
                std::fs::metadata(&zip).unwrap().len() / 1024,
                zip.display()
            );
            assert_eq!(summary.pieces, kit.pieces.len());
        }
    }
}
