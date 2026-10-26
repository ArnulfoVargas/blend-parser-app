use std::io::{Cursor, Read};

use bp_core::connect::{Connectors, Kind, Shape};
use bp_core::{ColorSource, Part, Project, Scene};

/// A 20 mm cube, painted red with a blue top.
fn cube() -> Scene {
    let positions =
        (0..8).map(|i| [(i & 1) as f32 * 20.0, ((i >> 1) & 1) as f32 * 20.0, ((i >> 2) & 1) as f32 * 20.0]).collect();
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let tris: Vec<_> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    Scene {
        parts: vec![Part {
            name: "box".into(),
            positions,
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
fn a_cut_with_dowels_exports_halves_and_dowels_as_named_objects() {
    let mut project = Project::new(cube(), 2);
    let c = Connectors {
        kind: Kind::Dowel,
        shape: Shape::Square,
        size_mm: 4.0,
        depth_mm: 5.0,
        clearance_mm: 0.15,
        angle_deg: 15.0,
        at: vec![[-4.0, -4.0, 10.0], [4.0, 4.0, 10.0]],
    };
    assert_eq!(project.cut_connected([0.0, 0.0, 10.0], [0.0, 0.0, 1.0], true, &[], &c, "dowel"), Ok(4));
    let pieces = project.export_parts();
    let names: Vec<&str> = pieces.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["box A", "box B", "box dowel 1", "box dowel 2"]);
    let model = bp_3mf::Model {
        name: "dowels",
        parts: pieces
            .iter()
            .map(|p| bp_3mf::Part {
                name: &p.name,
                positions: &p.positions,
                tris: &p.tris,
                paint: &p.paint,
                object: p.object,
            })
            .collect(),
        filaments: project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect(),
        printer: bp_3mf::Printer::default(),
        flush: bp_3mf::Flush::default(),
        layer_mm: None,
        offset_mm: [0.0; 2],
    };
    let mut bytes = Cursor::new(Vec::new());
    bp_3mf::write(&mut bytes, &model).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.into_inner())).unwrap();
    let mut main = String::new();
    archive.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut main).unwrap();
    assert_eq!(main.matches("<item ").count(), 4);
    let mut settings = String::new();
    archive.by_name("Metadata/model_settings.config").unwrap().read_to_string(&mut settings).unwrap();
    for name in names {
        assert!(settings.contains(&format!("value=\"{name}\"")), "{name}");
    }
    assert!(project.undo());
    assert_eq!(project.object_count(), 1);
}
