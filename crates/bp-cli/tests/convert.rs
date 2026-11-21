//! The command line end to end on projects, which need no Blender.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use bp_core::{ColorSource, Part, Project, Scene};

fn project() -> Project {
    let part = Part {
        name: "q".into(),
        positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 20.0], [0.0, 0.0, 20.0]],
        tris: vec![[0, 1, 2], [0, 2, 3]],
        tri_loops: vec![[0, 1, 2], [3, 4, 5]],
        tri_source: vec![0, 1],
        sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
        uv_layers: vec![],
        color_layers: vec![],
        object: 0,
    };
    Project::new(Scene { parts: vec![part], ..Default::default() }, 2)
}

fn save(p: &Project, path: &Path) {
    save_with(p, path, vec![]);
}

fn save_with(p: &Project, path: &Path, filaments: Vec<(String, bp_3mf::filament::Filament)>) {
    let settings = bp_3mf::project_file::Settings {
        format: bp_3mf::project_file::FORMAT,
        app_version: "test".into(),
        file_name: "quad.blend".into(),
        file_size: 1,
        import_warnings: vec![],
        printer: bp_3mf::Printer::default().profile,
        nozzle_mm: 0.4,
        flush: vec![],
        offset_mm: [0.0; 2],
        filaments,
        kit: None,
    };
    let file = std::fs::File::create(path).unwrap();
    bp_3mf::project_file::write(file, &serde_json::to_string(&settings).unwrap(), &p.to_bytes()).unwrap();
}

fn read(zip: &Path, name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn converts_a_folder_of_projects() {
    let dir = std::env::temp_dir().join(format!("bp-cli-e2e-{}", std::process::id()));
    let (inputs, out, config) = (dir.join("in"), dir.join("out"), dir.join("config"));
    std::fs::create_dir_all(&inputs).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let p = project();
    save(&p, &inputs.join("a.bpproj"));
    // Slot 1 of b carries a PETG spool.
    let spool = bp_3mf::filament::catalog().into_iter().find(|f| f.kind == "PETG HF").unwrap();
    save_with(&p, &inputs.join("b.bpproj"), vec![(bp_core::color::to_hex(p.palette()[0]), spool.clone())]);
    std::fs::write(inputs.join("notes.txt"), "not a model").unwrap();

    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_blendparser-cli"))
            .args(["convert"])
            .arg(&inputs)
            .args(["--out-dir"])
            .arg(&out)
            .args(["--config"])
            .arg(&config)
            .args(extra)
            .output()
            .unwrap()
    };
    let o = run(&["--json", "--size", "40"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report.as_array().unwrap().len(), 2, "the text file is skipped");
    assert_eq!(report[0]["colors"].as_array().unwrap().len(), 2);
    assert!((report[0]["sizeMm"][2].as_f64().unwrap() - 40.0).abs() < 0.01);

    // The painting is the project's.
    let objects = read(&out.join("a.3mf"), "3D/Objects/object_1.model");
    for paint in &p.export_parts()[0].paint {
        assert!(objects.contains(&format!("paint_color=\"{paint}\"")));
    }
    let cfg = read(&out.join("b.3mf"), "Metadata/project_settings.config");
    assert!(cfg.contains("Bambu Lab A1 0.4 nozzle"), "the project's printer");
    let cfg: serde_json::Value = serde_json::from_str(&cfg).unwrap();
    assert_eq!(cfg["filament_type"][0], "PETG");
    assert_eq!(cfg["filament_settings_id"][0], "Bambu PETG HF @BBL A1");
    assert_eq!(cfg["filament_colour"][0], format!("{}FF", spool.hex));
    assert_eq!(cfg["filament_type"][1], "PLA");

    // A damaged project does not stop the others; the exit code says one failed.
    std::fs::write(inputs.join("c.bpproj"), "broken").unwrap();
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("c.bpproj"));
    assert!(String::from_utf8_lossy(&o.stdout).contains("2 converted, 1 failed"));

    // Bad usage.
    let o = Command::new(env!("CARGO_BIN_EXE_blendparser-cli")).args(["convert", "--colors", "99"]).output().unwrap();
    assert_eq!(o.status.code(), Some(64));
    let _ = std::fs::remove_dir_all(&dir);
}
