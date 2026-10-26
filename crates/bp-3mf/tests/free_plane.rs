use std::io::{Cursor, Read};

use bp_core::{ColorSource, Part, Project, Scene};

fn cube() -> Scene {
    let positions =
        (0..8).map(|i| [(i & 1) as f32 * 10.0, ((i >> 1) & 1) as f32 * 10.0, ((i >> 2) & 1) as f32 * 10.0]).collect();
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let tris: Vec<_> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    Scene {
        parts: vec![Part {
            name: "painted cube".into(),
            positions,
            tri_loops: (0..tris.len() as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: (0..tris.len()).map(|i| u16::from(i < 2)).collect(),
            tris,
            sources: vec![ColorSource::Flat([0.9, 0.1, 0.1]), ColorSource::Flat([0.1, 0.1, 0.9])],
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn painted_free_cut_exports_both_objects_and_can_be_undone() {
    for lay in [false, true] {
        let mut project = Project::new(cube(), 2);
        project.paint([0.0, -20.0, 2.0], [0.0, 1.0, 0.0], 0.1, 1).unwrap();
        project.end_stroke();
        let before = project.preview();
        project.cut_plane([0.0, 0.0, 5.0], [1.0, 0.0, 1.0], true, true, lay, &[]).unwrap();
        assert!(project.painted_count() > 0);
        let pieces = project.export_parts();
        assert_eq!(pieces.len(), 2);
        assert_ne!(pieces[0].object, pieces[1].object);
        let model = bp_3mf::Model {
            name: "free plane",
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
        assert_eq!(main.matches("<item ").count(), 2);
        let mut geometry = String::new();
        archive.by_name("3D/Objects/object_1.model").unwrap().read_to_string(&mut geometry).unwrap();
        assert_eq!(geometry.matches("<triangle ").count(), pieces.iter().map(|p| p.tris.len()).sum::<usize>());
        assert_eq!(geometry.matches("paint_color=").count(), pieces.iter().map(|p| p.paint.len()).sum::<usize>());
        for p in &pieces {
            for color in &p.paint {
                assert!(geometry.contains(&format!("paint_color=\"{color}\"")));
            }
        }
        assert!(project.undo());
        assert_eq!(project.preview().positions, before.positions);
        assert_eq!(project.preview().labels, before.labels);
    }
}
