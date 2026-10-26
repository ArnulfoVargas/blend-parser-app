use std::io::{Cursor, Read};

use bp_core::fit::Room;
use bp_core::{ColorSource, Part, Project, Scene, SplitPlan};

/// A 600 × 150 × 40 mm slab: its pieces only fit one to a plate.
fn bar() -> Scene {
    let c = |i: usize| [(i & 1) as f32 * 600.0, ((i >> 1) & 1) as f32 * 150.0, ((i >> 2) & 1) as f32 * 40.0];
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let tris: Vec<_> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    Scene {
        parts: vec![Part {
            name: "bar".into(),
            positions: (0..8).map(c).collect(),
            tri_loops: (0..tris.len() as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: (0..tris.len()).map(|i| u16::from((2..4).contains(&i))).collect(),
            tris,
            sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn a_split_model_exports_one_plate_per_group_with_every_piece_on_its_plate() {
    let printer = bp_3mf::Printer::default();
    let reserve = bp_3mf::Reserve { tower: false, ..Default::default() };
    let room =
        Room { area_mm: bp_3mf::centred_room(&printer, &reserve), height_mm: printer.max_height_mm(), gap_mm: 5.0 };
    let mut project = Project::new(bar(), 2);
    let plan = SplitPlan { cuts: project.propose_split(&room), ..Default::default() };
    let report = project.split_for_printer(&plan, &room, "dowel").unwrap();
    assert!(report.plates >= 2, "{report:?}");
    let pieces = project.export_parts();
    let model = bp_3mf::Model {
        name: "bar",
        parts: pieces
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
        filaments: project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect(),
        printer: printer.clone(),
        flush: bp_3mf::Flush::default(),
        layer_mm: None,
        offset_mm: [0.0; 2],
    };
    // Every plate's pieces fit centred on it, as the 3MF puts them there.
    for plate in 0..report.plates as u32 {
        let (lo, hi) = pieces
            .iter()
            .filter(|p| p.plate == plate)
            .flat_map(|p| p.positions.iter())
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                (std::array::from_fn(|i| lo[i].min(v[i])), std::array::from_fn(|i| hi[i].max(v[i])))
            });
        let check = bp_3mf::plate::check(&printer, [hi[0] - lo[0], hi[1] - lo[1]], &reserve);
        assert!(check.fits(), "plate {plate}: {check:?}");
        assert!((lo[0] + hi[0]).abs() < 1e-2 && (lo[1] + hi[1]).abs() < 1e-2, "plate {plate} is centred");
    }
    let mut bytes = Cursor::new(Vec::new());
    bp_3mf::write(&mut bytes, &model).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.into_inner())).unwrap();
    let mut settings = String::new();
    archive.by_name("Metadata/model_settings.config").unwrap().read_to_string(&mut settings).unwrap();
    assert_eq!(settings.matches("<plate>").count(), report.plates);
    assert_eq!(settings.matches("<model_instance>").count(), report.pieces.len());
    for piece in &report.pieces {
        assert!(settings.contains(&format!("value=\"{}\"", piece.name)), "{}", piece.name);
    }
    let mut main = String::new();
    archive.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut main).unwrap();
    // The second plate sits a plate and a fifth to the right of the first, as Bambu Studio lays them out.
    let (x, _) = bp_3mf::plate_origin(&printer, 1, report.plates);
    let (cx, cy) = printer.plate_centre();
    assert!(main.contains(&format!("0 0 1 {} {} 0", x + cx, cy)), "{main}");
}
