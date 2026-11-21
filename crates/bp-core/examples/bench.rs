//! Times the steps a large model goes through, on a synthetic textured torus:
//!
//! ```sh
//! cargo run --release -p bp-core --example bench -- 2000000
//! ```
//!
//! The argument is the number of triangles (default 1 000 000). The texture has
//! stripes in four colours, so the painting subdivides along their borders as
//! it does on a real model.

use std::time::Instant;

use bp_core::{ColorSource, Image, MeshOps, Part, Project, Scene};

pub fn torus(tris: usize) -> Scene {
    // tris = 2 * rings * sides, with rings = 2 * sides.
    let sides = ((tris as f64 / 4.0).sqrt().round() as usize).max(3);
    let rings = sides * 2;
    let (big, small) = (40.0f32, 15.0f32);
    let mut positions = Vec::with_capacity(rings * sides);
    let mut uv = Vec::with_capacity(rings * sides);
    for r in 0..rings {
        let a = r as f32 / rings as f32 * std::f32::consts::TAU;
        for s in 0..sides {
            let b = s as f32 / sides as f32 * std::f32::consts::TAU;
            let d = big + small * b.cos();
            positions.push([d * a.cos(), d * a.sin(), small * b.sin()]);
            uv.push([r as f32 / rings as f32, s as f32 / sides as f32]);
        }
    }
    let at = |r: usize, s: usize| ((r % rings) * sides + s % sides) as u32;
    let mut out = Vec::with_capacity(rings * sides * 2);
    for r in 0..rings {
        for s in 0..sides {
            out.push([at(r, s), at(r + 1, s), at(r + 1, s + 1)]);
            out.push([at(r, s), at(r + 1, s + 1), at(r, s + 1)]);
        }
    }
    // One loop per corner, as Blender writes them.
    let tri_loops: Vec<[u32; 3]> = (0..out.len() as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
    let loops_uv: Vec<[f32; 2]> = out.iter().flat_map(|t| t.map(|v| uv[v as usize])).collect();
    let size = 1024u32;
    let palette = [[200u8, 30, 30], [30, 160, 60], [30, 60, 200], [230, 210, 40]];
    let mut rgb = Vec::with_capacity((size * size * 3) as usize);
    for y in 0..size {
        for x in 0..size {
            let band = ((x / 37) + (y / 53) * 3) as usize % palette.len();
            rgb.extend(palette[band]);
        }
    }
    let n = out.len();
    let part = Part {
        name: "torus".into(),
        positions,
        tris: out,
        tri_loops,
        tri_source: vec![0; n],
        sources: vec![ColorSource::Image { image: 0, uv: 0 }],
        uv_layers: vec![loops_uv],
        color_layers: vec![],
        object: 0,
        plate: 0,
    };
    Scene {
        parts: vec![part],
        images: vec![Image { name: "stripes".into(), width: size, height: size, rgb }],
        materials: vec![],
    }
}

fn time<R>(label: &str, f: impl FnOnce() -> R) -> R {
    let t = Instant::now();
    let r = f();
    eprintln!("{label:<34} {:>9.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    r
}

fn main() {
    let tris: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1_000_000);
    let scene = time("synthetic scene", || torus(tris));
    eprintln!("triangles: {}", scene.triangle_count());
    if std::env::var_os("BREAKDOWN").is_some() {
        use bp_core::{bvh::Bvh, color::Classifier, field::ColorField, mesh::Mesh, optimize, paint, palette};
        let mut sc = scene.clone();
        sc.place_on_plate();
        let (shape, work) = time("  Mesh::build", || Mesh::build(&sc, MeshOps::default(), 1.0));
        time("  adjacency", || work.parts.iter().map(optimize::adjacency).collect::<Vec<_>>());
        let soup = time("  soup", || work.soup());
        time("  Bvh::build", || Bvh::build(&soup));
        let field = time("  ColorField::new", || ColorField::new(&sc, &shape));
        let samples = time("  surface_samples", || palette::surface_samples(&field, &shape, 300_000, 1));
        let pal = time("  auto_palette", || palette::auto_palette(&samples, 4));
        let targets: Vec<_> = pal.iter().enumerate().map(|(i, &c)| (c, i as u8)).collect();
        let cls = Classifier::with_slots(&targets);
        time("  paint::build", || {
            paint::build(&field, &shape, &cls, paint::PaintParams { max_depth: 5, min_edge: 0.2 })
        });
        let (shape, work) =
            time("  Mesh::build keep 50%", || Mesh::build(&sc, MeshOps { keep_ratio: 0.5, ..MeshOps::default() }, 1.0));
        time("  adjacency keep 50%", || work.parts.iter().map(optimize::adjacency).collect::<Vec<_>>());
        let field = time("  ColorField::new keep 50%", || ColorField::new(&sc, &shape));
        time("  paint::build keep 50%", || {
            paint::build(&field, &shape, &cls, paint::PaintParams { max_depth: 5, min_edge: 0.2 })
        });
    }
    let mut p = time("Project::new", || Project::new(scene, 4));
    let pv = time("preview_bytes", || p.preview_bytes());
    eprintln!("leaves: {}", u32::from_le_bytes(pv[..4].try_into().unwrap()));
    time("stats", || p.stats());
    time("small_islands", || p.small_islands(1.0));
    let hit = time("pick", || p.pick([0.0, -200.0, 15.0], [0.0, 1.0, 0.0]));
    assert!(hit.is_some());
    time("paint (3 mm)", || p.paint([0.0, -200.0, 15.0], [0.0, 1.0, 0.0], 3.0, 1));
    time("end_stroke", || p.end_stroke());
    time("rotate 90° (rigid)", || p.rotate(2, 90.0));
    time("set_height_mm", || p.set_height_mm(60.0));
    let pal = p.palette().to_vec();
    time("set_palette", || p.set_palette(pal));
    time("set_mesh_ops keep 50%", || p.set_mesh_ops(MeshOps { keep_ratio: 0.5, ..p.mesh_ops() }));
    time("preview_bytes (simplified)", || p.preview_bytes());
}
