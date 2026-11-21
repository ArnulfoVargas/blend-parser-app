//! Small projects for reviewing the combined PRs without needing Blender.
use std::path::Path;

use bp_3mf::project_file::{self, Settings};
use bp_core::{ColorSource, Part, Project, Scene, SplitPlan};

fn block(name: &str, size: [f32; 3], offset: [f32; 3]) -> Part {
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let tris: Vec<_> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    Part {
        name: name.into(),
        positions: (0..8).map(|i| std::array::from_fn(|a| offset[a] + ((i >> a) & 1) as f32 * size[a])).collect(),
        tri_loops: (0..tris.len() as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
        tri_source: (0..tris.len()).map(|i| u16::from((2..4).contains(&i))).collect(),
        tris,
        sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
        ..Default::default()
    }
}

fn save(dir: &Path, name: &str, project: &Project) {
    let printer = bp_3mf::Printer::default();
    let settings = Settings {
        format: project_file::FORMAT,
        app_version: env!("CARGO_PKG_VERSION").into(),
        file_name: name.into(),
        file_size: 0,
        import_warnings: vec![],
        printer: printer.profile,
        nozzle_mm: printer.nozzle_mm,
        flush: vec![],
        offset_mm: [0.0; 2],
        filaments: vec![],
        kit: None,
    };
    let file = std::fs::File::create(dir.join(format!("{name}.bpproj"))).unwrap();
    project_file::write(file, &serde_json::to_string(&settings).unwrap(), &project.to_bytes()).unwrap();
}

fn main() {
    let out = std::env::args().nth(1).expect("output directory");
    let dir = Path::new(&out);
    std::fs::create_dir_all(dir).unwrap();
    let scene = Scene { parts: vec![block("Cube", [40.0; 3], [0.0; 3])], ..Default::default() };
    save(dir, "01-cube-cut-connect-region-kit", &Project::new(scene, 2));
    let scene = Scene {
        parts: vec![block("Left", [30.0; 3], [0.0; 3]), block("Right", [20.0; 3], [40.0, 0.0, 0.0])],
        ..Default::default()
    };
    save(dir, "02-disconnected-pieces", &Project::new(scene, 2));
    let scene = Scene { parts: vec![block("Long bar", [600.0, 150.0, 40.0], [0.0; 3])], ..Default::default() };
    let mut project = Project::new(scene, 2);
    save(dir, "03-too-large-for-printer", &project);
    let printer = bp_3mf::Printer::default();
    let room = bp_core::fit::Room {
        area_mm: bp_3mf::centred_room(&printer, &bp_3mf::Reserve { tower: false, ..Default::default() }),
        height_mm: printer.max_height_mm(),
        gap_mm: 5.0,
    };
    let plan = SplitPlan { cuts: project.propose_split(&room), ..Default::default() };
    project.split_for_printer(&plan, &room, "dowel").unwrap();
    save(dir, "04-split-across-plates", &project);
    println!("Four review projects written to {}", dir.display());
}
