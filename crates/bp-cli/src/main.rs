//! BlendParser without the window: converts models (through Blender) and saved
//! projects to Bambu Studio 3MF files, one or a whole folder at a time, with the
//! same core, settings and writer as the app, so the same model and settings
//! give the same file.

mod args;
mod config;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use args::{Args, Command, Size};
use bp_core::{ColorOps, MeshOps, PrintOps, Project};
use config::Config;
use serde_json::json;

/// Exit codes: every file converted cleanly, converted with warnings, or something failed.
const OK: u8 = 0;
const WARNINGS: u8 = 1;
const FAILED: u8 = 2;
const USAGE: u8 = 64;

fn main() -> ExitCode {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{}", args::USAGE);
            return ExitCode::from(USAGE);
        }
    };
    match &args.command {
        Command::Help => {
            println!("{}", args::USAGE);
            ExitCode::from(OK)
        }
        Command::Printers => {
            let config = Config::load(args.config.as_deref());
            for p in config.profiles() {
                let nozzles: Vec<String> = p.nozzles.iter().map(|n| n.to_string()).collect();
                println!("{}\t{} mm\t{}", p.id, nozzles.join(", "), p.name);
            }
            ExitCode::from(OK)
        }
        Command::Convert => convert_all(&args),
    }
}

/// What one input gave: where it went, what it estimated, what to look at.
struct Report {
    input: PathBuf,
    output: Option<PathBuf>,
    colors: Vec<String>,
    swaps: u32,
    waste_g: f32,
    size_mm: [f32; 3],
    warnings: Vec<String>,
    error: Option<String>,
}

fn convert_all(args: &Args) -> ExitCode {
    let inputs = match args::expand(&args.inputs) {
        Ok(i) if !i.is_empty() => i,
        Ok(_) => {
            eprintln!("no models or projects to convert\n\n{}", args::USAGE);
            return ExitCode::from(USAGE);
        }
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(USAGE);
        }
    };
    if args.output.is_some() && inputs.len() > 1 {
        eprintln!("-o names one file; use --out-dir for several inputs");
        return ExitCode::from(USAGE);
    }
    let config = Config::load(args.config.as_deref());
    let mut reports = Vec::new();
    for input in &inputs {
        // A file that fails does not stop the batch.
        let report = match convert(input, args, &config) {
            Ok(r) => r,
            Err(e) => Report {
                input: input.clone(),
                output: None,
                colors: vec![],
                swaps: 0,
                waste_g: 0.0,
                size_mm: [0.0; 3],
                warnings: vec![],
                error: Some(e),
            },
        };
        if !args.json {
            print_report(&report);
        }
        reports.push(report);
    }
    let failed = reports.iter().filter(|r| r.error.is_some()).count();
    if args.json {
        let list: Vec<_> = reports
            .iter()
            .map(|r| {
                json!({
                    "input": r.input,
                    "output": r.output,
                    "colors": r.colors,
                    "swaps": r.swaps,
                    "wasteG": (r.waste_g * 10.0).round() / 10.0,
                    "sizeMm": r.size_mm.map(|v| (v * 100.0).round() / 100.0),
                    "warnings": r.warnings,
                    "error": r.error,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
    } else if reports.len() > 1 {
        println!("\n{} converted, {} failed", reports.len() - failed, failed);
    }
    let warned = reports.iter().any(|r| !r.warnings.is_empty());
    ExitCode::from(if failed > 0 {
        FAILED
    } else if warned {
        WARNINGS
    } else {
        OK
    })
}

fn print_report(r: &Report) {
    match (&r.error, &r.output) {
        (Some(e), _) => eprintln!("✗ {}: {e}", r.input.display()),
        (None, Some(out)) => {
            println!("✓ {} → {}", r.input.display(), out.display());
            println!(
                "  {} × {} × {} mm · {} · {} swaps · {:.1} g waste",
                (r.size_mm[0] * 10.0).round() / 10.0,
                (r.size_mm[1] * 10.0).round() / 10.0,
                (r.size_mm[2] * 10.0).round() / 10.0,
                r.colors.join(" "),
                r.swaps,
                r.waste_g
            );
            for w in &r.warnings {
                println!("  ! {w}");
            }
        }
        (None, None) => {}
    }
}

/// Where the 3MF for `input` goes.
fn output_for(input: &Path, args: &Args) -> PathBuf {
    if let Some(o) = &args.output {
        return o.clone();
    }
    let name = input.file_stem().map_or_else(|| "model".into(), |s| s.to_os_string());
    let dir = args.out_dir.clone().or_else(|| input.parent().map(Path::to_path_buf)).unwrap_or_default();
    dir.join(name).with_extension("3mf")
}

/// The open document, as the app keeps it, before the options are applied.
struct Doc {
    project: Project,
    name: String,
    printer: bp_3mf::Printer,
    flush: Vec<(String, String, f32)>,
    offset_mm: [f32; 2],
    /// The filament on each slot that has one, by the slot's colour.
    filaments: Vec<(String, bp_3mf::filament::Filament)>,
    warnings: Vec<String>,
}

fn open(input: &Path, args: &Args, config: &Config) -> Result<Doc, String> {
    let stem = |p: &str| Path::new(p).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if input.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpproj")) {
        let file = std::fs::File::open(input).map_err(|e| e.to_string())?;
        let (json, model) = bp_3mf::project_file::read(std::io::BufReader::new(file))
            .map_err(|_| "not a BlendParser project".to_string())?;
        let s = bp_3mf::project_file::parse_settings(&json).map_err(|e| match e {
            bp_3mf::project_file::ProjectError::Version => "saved by a newer BlendParser".to_string(),
            bp_3mf::project_file::ProjectError::Corrupt => "not a BlendParser project, or damaged".to_string(),
        })?;
        let project = Project::from_bytes(&model).map_err(|_| "the project's model is damaged".to_string())?;
        let known = config.profiles().into_iter().find(|p| p.id == s.printer.id);
        let printer = bp_3mf::Printer::new(known.unwrap_or(s.printer), s.nozzle_mm);
        return Ok(Doc {
            project,
            name: stem(&s.file_name),
            printer,
            flush: s.flush,
            offset_mm: s.offset_mm,
            filaments: s.filaments,
            warnings: s.import_warnings.into_iter().map(|w| format!("{}: {}", w.code, w.detail)).collect(),
        });
    }
    let blender = match &args.blender {
        Some(b) => b.clone(),
        None => bp_blend::find_blender().ok_or("Blender not found: install it or pass --blender PATH")?,
    };
    let imported = bp_blend::extract_with_report(&blender, input, &mut |_| {}).map_err(|e| e.to_string())?;
    Ok(Doc {
        project: Project::new(imported.scene, 4),
        name: stem(&input.display().to_string()),
        printer: config.printer(),
        flush: vec![],
        offset_mm: [0.0; 2],
        filaments: vec![],
        warnings: imported.warnings.into_iter().map(|w| format!("{}: {}", w.code, w.detail)).collect(),
    })
}

fn convert(input: &Path, args: &Args, config: &Config) -> Result<Report, String> {
    let mut doc = open(input, args, config)?;
    if let Some(id) = &args.printer {
        let profile = config
            .profiles()
            .into_iter()
            .find(|p| p.id == bp_3mf::canonical_id(id))
            .ok_or_else(|| format!("unknown printer {id:?}: see `blendparser-cli printers`"))?;
        doc.printer = bp_3mf::Printer::new(profile, args.nozzle.unwrap_or(doc.printer.nozzle_mm));
    } else if let Some(n) = args.nozzle {
        doc.printer = bp_3mf::Printer::new(doc.printer.profile.clone(), n);
    }
    if args.nozzle.is_some_and(|n| (n - doc.printer.nozzle_mm).abs() > 1e-4) {
        doc.warnings.push(format!(
            "nozzle {} mm not in this printer's profile: using {} mm",
            args.nozzle.unwrap_or(0.0),
            doc.printer.nozzle_mm
        ));
    }
    let layer_mm = config.waste.layer_mm.unwrap_or_else(|| doc.printer.layer_mm());
    let p = &mut doc.project;
    p.set_print_ops(PrintOps { nozzle_mm: doc.printer.nozzle_mm, layer_mm });

    // In the order the panels apply them: colours, size, mesh, colour clean-up.
    if let Some(colors) = &args.palette {
        p.set_palette(colors.clone());
    } else if let Some(n) = args.colors {
        let pal = p.suggest_palette(n);
        p.set_palette(pal);
    }
    match args.size {
        Some(Size::Longest(mm)) => {
            let now = p.size_mm();
            let k = mm / now.iter().copied().fold(f32::MIN, f32::max);
            p.set_size_mm(now.map(|v| v * k));
        }
        Some(Size::Axes(mm)) => p.set_size_mm(mm),
        None => {}
    }
    if args.keep.is_some() || args.flatten.is_some() || args.min_piece.is_some() {
        let now = p.mesh_ops();
        p.set_mesh_ops(MeshOps {
            keep_ratio: args.keep.unwrap_or(now.keep_ratio),
            flatten_mm: args.flatten.unwrap_or(now.flatten_mm),
            min_piece_mm: args.min_piece.unwrap_or(now.min_piece_mm),
            ..now
        });
    }
    if args.detail.is_some() || args.islands.is_some() {
        let now = p.color_ops();
        p.set_color_ops(ColorOps {
            border_detail: args.detail.unwrap_or(now.border_detail),
            min_island_mm2: args.islands.unwrap_or(now.min_island_mm2),
        });
    }

    let output = output_for(input, args);
    let stats = p.stats();
    let detected = p.palette().to_vec();
    // Each slot prints its filament's colour when the project picked one, as in the app.
    let slot_filament: Vec<Option<&bp_3mf::filament::Filament>> = detected
        .iter()
        .map(|c| doc.filaments.iter().find(|f| f.0 == bp_core::color::to_hex(*c)).map(|f| &f.1))
        .collect();
    let palette: Vec<_> = detected
        .iter()
        .zip(&slot_filament)
        .map(|(&c, f)| f.and_then(|f| bp_core::color::parse_hex(&f.hex)).unwrap_or(c))
        .collect();
    let keys: Vec<String> = detected.iter().map(|c| bp_core::color::to_hex(*c)).collect();
    let hex: Vec<String> = palette.iter().map(|c| bp_core::color::to_hex(*c)).collect();
    // Flush per swap: what the user typed in the project, the colours' estimate otherwise.
    let n = palette.len();
    let volumes: Vec<f32> = (0..n * n)
        .map(|i| {
            let (a, b) = (i / n, i % n);
            let typed = doc.flush.iter().find(|f| f.0 == keys[a] && f.1 == keys[b]).map(|f| f.2).filter(|_| a != b);
            // The colours' flush on this printer, then the materials', as Bambu Studio works it out.
            typed.unwrap_or_else(|| {
                match bp_core::waste::default_flush(palette[a], palette[b], doc.printer.flush_min_mm3()) {
                    0.0 => 0.0,
                    v => bp_3mf::filament::flush(
                        slot_filament[a].map(|f| f.kind.as_str()),
                        slot_filament[b].map(|f| f.kind.as_str()),
                        v,
                    ),
                }
            })
        })
        .collect();
    let w = &config.waste;
    let params = bp_core::WasteParams {
        layer_mm,
        nozzle_mm: doc.printer.nozzle_mm,
        filament_diameter_mm: w.filament_diameter_mm,
        density_g_cm3: w.density_g_cm3,
        flush_multiplier: w.flush_multiplier,
        start_purge_mm3: w.start_purge_mm3,
        prime_mm3: w.prime_mm3,
        flush_reuse: w.flush_reuse,
        tower: n > 1,
        tower_depth_mm: config.tower_depth_mm,
        purge_chute: doc.printer.purges_into_chute(),
        ..Default::default()
    };
    let waste = bp_core::waste::waste(&stats.swaps, &params, |a, b| volumes[a as usize * n + b as usize]);

    let parts = p.export_parts();
    let model = bp_3mf::Model {
        name: &doc.name,
        piece: bp_3mf::ZipText::for_lang(&args.lang).piece,
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
        filaments: hex.clone(),
        slots: slot_filament.iter().map(|f| f.map(|f| bp_3mf::filament::for_slot(&f.kind, &doc.printer))).collect(),
        printer: doc.printer.clone(),
        flush: bp_3mf::Flush {
            volumes_mm3: volumes,
            multiplier: w.flush_multiplier,
            into_infill: w.flush_reuse > 0.0,
            prime_mm3: w.prime_mm3,
        },
        layer_mm: w.layer_mm,
        offset_mm: doc.offset_mm,
        thumbnail: None,
    };
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let file = std::fs::File::create(&output).map_err(|e| format!("{}: {e}", output.display()))?;
    bp_3mf::write(std::io::BufWriter::new(file), &model).map_err(|e| format!("{}: {e}", output.display()))?;

    let mut warnings = doc.warnings;
    let size = stats.size_mm;
    if size[2] > doc.printer.max_height_mm() {
        warnings.push(format!("taller than the printer: {:.1} mm of {:.0} mm", size[2], doc.printer.max_height_mm()));
    }
    Ok(Report {
        input: input.to_path_buf(),
        output: Some(output),
        colors: hex,
        swaps: waste.swaps,
        waste_g: waste.total_g,
        size_mm: size,
        warnings,
        error: None,
    })
}
