//! Benchmark of what each UI command costs on the backend, split into the
//! operation itself and the summary every command sends back. Built only with
//! `--features bench`; run with `cargo run --release -p blend-parser-app
//! --features bench --example bench -- <model> [runs]`.

use std::path::Path;
use std::time::Instant;

use bp_core::{ColorOps, MeshOps};

use crate::dto;
use crate::prefs::Prefs;
use crate::profiles::Store;
use crate::state::Document;

pub struct Row {
    pub op: &'static str,
    pub op_ms: f64,
    pub summary_ms: f64,
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Median of `runs` timings of `f`, each followed by the summary.
fn time(doc: &mut Document, prefs: &Prefs, runs: usize, mut f: impl FnMut(&mut Document)) -> (f64, f64) {
    let mut ops = Vec::with_capacity(runs);
    let mut sums = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        f(doc);
        ops.push(ms(t));
        let t = Instant::now();
        std::hint::black_box(dto::summary(doc, prefs));
        sums.push(ms(t));
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    (med(&mut ops), med(&mut sums))
}

/// Imports `model` with Blender and times every command's backend work.
pub fn run(model: &Path, runs: usize) -> Vec<Row> {
    let blender = bp_blend::find_blender().expect("Blender is needed to import the model");
    let t = Instant::now();
    let imported = bp_blend::extract_with_report(&blender, model, &mut |_| {}).expect("import");
    let import_ms = ms(t);
    let store = Store::default();
    let prefs = Prefs::default();
    let printer = store.printer();
    let t = Instant::now();
    let mut project = bp_core::Project::new(imported.scene, 4);
    project.set_print_ops(crate::commands::print_ops(&printer, &prefs));
    let new_ms = ms(t);
    let mut doc = Document {
        project,
        file_name: "bench".into(),
        file_size: 0,
        import_warnings: vec![],
        printer,
        flush: Default::default(),
        offset_mm: [0.0; 2],
    };
    let mut rows = vec![Row { op: "import (Blender)", op_ms: import_ms, summary_ms: 0.0 }];
    let t = Instant::now();
    std::hint::black_box(dto::summary(&doc, &prefs));
    rows.push(Row { op: "open: Project::new", op_ms: new_ms, summary_ms: ms(t) });

    let mut push = |op, (o, s)| rows.push(Row { op, op_ms: o, summary_ms: s });
    let size = doc.project.size_mm();
    let mut grow = false;
    push(
        "set_size",
        time(&mut doc, &prefs, runs, |d| {
            grow = !grow;
            let k = if grow { 1.5 } else { 1.0 };
            d.project.set_size_mm([size[0] * k, size[1] * k, size[2] * k]);
        }),
    );
    push(
        "rotate 90°",
        time(&mut doc, &prefs, runs, |d| {
            d.project.rotate(0, 90.0);
        }),
    );
    doc.project.reset_rotation();
    push(
        "rotate 15°",
        time(&mut doc, &prefs, runs, |d| {
            d.project.rotate(2, 15.0);
        }),
    );
    doc.project.reset_rotation();
    push(
        "cut + undo",
        time(&mut doc, &prefs, runs, |d| {
            d.project.cut(2, 0.4, true, true);
            d.project.undo_cut();
        }),
    );
    let base = doc.project.mesh_ops();
    let mut half = false;
    push(
        "simplify 50 % ↔ 100 %",
        time(&mut doc, &prefs, runs, |d| {
            half = !half;
            d.project.set_mesh_ops(MeshOps { keep_ratio: if half { 0.5 } else { 1.0 }, ..base });
        }),
    );
    doc.project.set_mesh_ops(base);
    let colors = doc.project.color_ops();
    let mut low = false;
    push(
        "border detail 2 ↔ 5",
        time(&mut doc, &prefs, runs, |d| {
            low = !low;
            d.project.set_color_ops(ColorOps { border_detail: if low { 2 } else { colors.border_detail }, ..colors });
        }),
    );
    doc.project.set_color_ops(colors);
    push("suggest palette (6)", time(&mut doc, &prefs, runs, |d| drop(d.project.suggest_palette(6))));
    push(
        "preview mesh",
        time(&mut doc, &prefs, runs, |d| {
            std::hint::black_box(d.project.preview());
        }),
    );
    // A brush dab straight down onto the top of the model.
    let s = doc.project.size_mm();
    let pv = doc.project.preview();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in pv.positions.chunks(3) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let origin = [mid[0], mid[1], hi[2] + (hi[2] - lo[2])];
    let down = [0.0, 0.0, -1.0];
    push(
        "paint dab + end stroke",
        time(&mut doc, &prefs, runs, |d| {
            d.project.paint(origin, down, s[0].max(s[2]) * 0.05, 1);
            d.project.end_stroke();
        }),
    );
    push(
        "pick",
        time(&mut doc, &prefs, runs, |d| {
            std::hint::black_box(d.project.pick(origin, down));
        }),
    );
    let params = bp_core::printability::Params::for_nozzle(0.4);
    push(
        "printability",
        time(&mut doc, &prefs, runs.min(3), |d| {
            std::hint::black_box(crate::commands::printability_for_doc(d, &prefs, params));
        }),
    );
    let out = std::env::temp_dir().join("bp-bench.3mf");
    push(
        "export 3MF",
        time(&mut doc, &prefs, runs.min(3), |d| {
            crate::commands::write_3mf(d, &prefs, &out.display().to_string()).expect("export");
        }),
    );
    let only = time(&mut doc, &prefs, runs, |_| {});
    push("summary alone", only);
    for (name, ms) in doc.project.profile_phases() {
        rows.push(Row { op: name, op_ms: ms, summary_ms: 0.0 });
    }
    let t = Instant::now();
    let stats = doc.project.stats();
    let stats_ms = ms(t);
    let t = Instant::now();
    std::hint::black_box(dto::waste_and_plate(&doc, &prefs, &stats.swaps, stats.size_mm));
    rows.push(Row { op: "summary: stats()", op_ms: stats_ms, summary_ms: 0.0 });
    rows.push(Row { op: "summary: waste_and_plate", op_ms: ms(t), summary_ms: 0.0 });
    let t = Instant::now();
    std::hint::black_box(doc.project.small_islands(doc.printer.min_detail_mm2()));
    rows.push(Row { op: "summary: small_islands", op_ms: ms(t), summary_ms: 0.0 });
    rows
}
