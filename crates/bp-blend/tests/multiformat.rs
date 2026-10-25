use std::path::Path;
use std::process::Command;

#[test]
fn rejects_unsupported_inputs_before_launching_blender() {
    assert!(bp_blend::supports(Path::new("MODEL.OBJ")));
    assert!(!bp_blend::supports(Path::new("model.blend1")));
    assert!(!bp_blend::supports(Path::new("model.3mf")));
    let result = bp_blend::extract(Path::new("nonexistent-blender"), Path::new("file.exe"), &mut |_| {});
    assert!(matches!(result, Err(bp_blend::BlendError::Unsupported(_))));
}

/// Requires Blender 3.6+. Generates OBJ+MTL+PNG, binary FBX, glTF/GLB and vertex-colour PLY,
/// then exercises the exact extraction/palette/3MF path used by the desktop app.
/// cargo test -p bp-blend --test multiformat -- --ignored --nocapture
#[test]
#[ignore = "requires an installed Blender"]
fn real_multicolor_imports_and_missing_resources() {
    let blender = bp_blend::find_blender().expect("Blender must be installed");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("bp-import-test-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("generate.py");
    std::fs::write(&script, include_str!("generate_models.py")).unwrap();
    let output = Command::new(&blender)
        .args(["-b", "--factory-startup", "--disable-autoexec", "--python-exit-code", "1", "--python"])
        .arg(&script)
        .arg("--")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for file in ["colors.blend", "colors.obj", "colors.fbx", "colors.gltf", "colors.glb", "colors.ply", "unlit.gltf"] {
        let ext = Path::new(file).extension().unwrap().to_str().unwrap();
        let imported = bp_blend::extract_with_report(&blender, &dir.join(file), &mut |_| {}).unwrap();
        let scene = imported.scene;
        assert_eq!(scene.triangle_count(), 24, "{ext}: geometry");
        let (lo, hi) = scene.bounds().unwrap();
        let mut size: Vec<_> = (0..3).map(|i| hi[i] - lo[i]).collect();
        size.sort_by(f32::total_cmp);
        assert!((size[0] - 10.0).abs() < 0.02 && (size[2] - 40.0).abs() < 0.02, "{ext}: units {size:?}");
        let samples: Vec<_> = scene
            .parts
            .iter()
            .flat_map(|p| (0..p.tris.len()).map(|t| p.sample(&scene.images, t, [1.0 / 3.0; 3]).rgb()))
            .collect();
        assert!(samples.iter().any(|c| c[0] > 0.9 && c[2] < 0.1), "{ext}: red missing {samples:?}");
        assert!(samples.iter().any(|c| c[2] > 0.9 && c[0] < 0.1), "{ext}: blue missing {samples:?}");
        if ["obj", "fbx", "gltf", "glb"].contains(&ext) {
            assert!(!scene.images.is_empty(), "{ext}: texture lost");
        }
        if ext != "ply" {
            assert_eq!(scene.parts.len(), 2, "{ext}: object structure");
            for part in &scene.parts {
                for tri in 0..part.tris.len() {
                    let top = part.corners(tri).iter().all(|p| (p[2] - 10.0).abs() < 0.02);
                    let color = part.sample(&scene.images, tri, [1.0 / 3.0; 3]).rgb();
                    assert!(
                        if top { color[2] > 0.9 && color[0] < 0.1 } else { color[0] > 0.9 && color[2] < 0.1 },
                        "{ext}: per-face material lost at {tri}: {color:?}"
                    );
                }
            }
        }
        let project = bp_core::Project::new(scene, 2);
        let parts = project.export_parts();
        let model = bp_3mf::Model {
            name: "fixture",
            parts: parts
                .iter()
                .map(|p| bp_3mf::Part { name: &p.name, positions: &p.positions, tris: &p.tris, paint: &p.paint })
                .collect(),
            filaments: project.palette().iter().map(|c| bp_core::color::to_hex(*c)).collect(),
            printer: bp_3mf::Printer::default(),
            flush: Default::default(),
            layer_mm: None,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        bp_3mf::write(&mut out, &model).unwrap();
        assert!(out.into_inner().len() > 100);
        eprintln!("{ext}: 24 triangles, 40 × 10 × 10 mm, red + blue, 3MF exported");
    }
    for (file, code) in [
        ("missing.obj", "missingMaterial"),
        ("texture.obj", "missingTexture"),
        ("tabs.obj", "missingMaterial"),
        ("tabs-texture.obj", "missingTexture"),
    ] {
        let result = bp_blend::extract_with_report(&blender, &dir.join(file), &mut |_| {}).unwrap();
        assert!(result.warnings.iter().any(|w| w.code == code), "{file}: missing warning");
    }
    let gltf: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("colors.gltf")).unwrap()).unwrap();
    let buffer = dir.join(gltf["buffers"][0]["uri"].as_str().unwrap());
    std::fs::remove_file(buffer).unwrap();
    let result = bp_blend::extract_with_report(&blender, &dir.join("colors.gltf"), &mut |_| {});
    assert!(matches!(result, Err(bp_blend::BlendError::Blender(_))), "missing glTF geometry must fail explicitly");
    std::fs::remove_dir_all(&dir).unwrap();
}
