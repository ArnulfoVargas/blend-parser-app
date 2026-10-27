//! Local diagnostics for the colour split (not shipped): synthetic painted
//! models, quality metrics and pictures of the pieces.
use bp_core::bvh::{Bvh, TriSoup};
use bp_core::geom::{self, Bary, Vec3};
use bp_core::render::{self, Camera, Item, Style};
use bp_core::split::{self, Piece, SplitParams};
use std::collections::HashMap;
use std::io::Write;

type Mesh = (Vec<Vec3>, Vec<[u32; 3]>);

fn icosphere(r: f32, c: Vec3, level: usize) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut p: Vec<Vec3> = vec![
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut nf = vec![];
        for t in &f {
            let mut m = [0u32; 3];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                m[k] = *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    p.push(geom::scale(geom::add(p[a as usize], p[b as usize]), 0.5));
                    p.len() as u32 - 1
                });
            }
            nf.push([t[0], m[0], m[2]]);
            nf.push([t[1], m[1], m[0]]);
            nf.push([t[2], m[2], m[1]]);
            nf.push(m);
        }
        f = nf;
    }
    let p = p.into_iter().map(|v| geom::add(c, geom::scale(geom::normalize(v), r))).collect();
    (p, f)
}

/// Closed cylinder along Z from z0 to z1.
fn cylinder(r: f32, c: [f32; 2], z0: f32, z1: f32, n: usize) -> Mesh {
    let mut p = vec![[c[0], c[1], z0], [c[0], c[1], z1]];
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        p.push([c[0] + r * a.cos(), c[1] + r * a.sin(), z0]);
        p.push([c[0] + r * a.cos(), c[1] + r * a.sin(), z1]);
    }
    let mut f = vec![];
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        let (b0, t0, b1, t1) = (2 + 2 * i, 3 + 2 * i, 2 + 2 * j, 3 + 2 * j);
        f.push([0, b1, b0]);
        f.push([1, t0, t1]);
        f.push([b0, b1, t1]);
        f.push([b0, t1, t0]);
    }
    (p, f)
}

fn cuboid(lo: Vec3, hi: Vec3, n: usize) -> Mesh {
    // Subdivided faces so paint can follow.
    let mut p = vec![];
    let mut f = vec![];
    let mut idx: HashMap<[i64; 3], u32> = HashMap::new();
    let mut vid = |q: Vec3, p: &mut Vec<Vec3>| -> u32 {
        let k = q.map(|x| (x * 1000.0).round() as i64);
        *idx.entry(k).or_insert_with(|| {
            p.push(q);
            p.len() as u32 - 1
        })
    };
    for axis in 0..3 {
        for side in 0..2 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for i in 0..n {
                for j in 0..n {
                    let at = |a: usize, b: usize| {
                        let mut q = [0.0; 3];
                        q[axis] = if side == 0 { lo[axis] } else { hi[axis] };
                        q[u] = lo[u] + (hi[u] - lo[u]) * a as f32 / n as f32;
                        q[v] = lo[v] + (hi[v] - lo[v]) * b as f32 / n as f32;
                        q
                    };
                    let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)].map(|q| vid(q, &mut p));
                    if side == 1 {
                        f.push([q[0], q[1], q[2]]);
                        f.push([q[0], q[2], q[3]]);
                    } else {
                        f.push([q[0], q[2], q[1]]);
                        f.push([q[0], q[3], q[2]]);
                    }
                }
            }
        }
    }
    (p, f)
}

fn join(ms: &[Mesh]) -> Mesh {
    let (mut p, mut f) = (vec![], vec![]);
    for (mp, mf) in ms {
        let b = p.len() as u32;
        p.extend_from_slice(mp);
        f.extend(mf.iter().map(|t| t.map(|v| v + b)));
    }
    (p, f)
}

fn soup(m: &Mesh) -> TriSoup {
    let mut s = TriSoup::default();
    for (i, t) in m.1.iter().enumerate() {
        s.push(t.map(|v| m.0[v as usize]), 0, i as u32);
    }
    s
}

const PAL: [[u8; 3]; 5] =
    [[0xE8, 0xD8, 0xB0], [0x30, 0x30, 0x38], [0xC8, 0x30, 0x30], [0x30, 0x80, 0xD0], [0x40, 0xA0, 0x50]];

fn volume(p: &Piece) -> f32 {
    p.tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| p.positions[v as usize]);
            geom::dot(a, geom::cross(b, c)) / 6.0
        })
        .sum()
}

fn metrics(name: &str, model: &Mesh, s: &split::Split) {
    let src = soup(model);
    let bvh = Bvh::build(&src);
    let h = s.voxel_mm;
    let mut total_vol = 0.0;
    let (mut nonmanifold, mut not_tight) = (0, 0);
    let (mut ang_sum, mut ang_n, mut ang_bad) = (0.0f64, 0usize, 0usize);
    let mut slivers = 0usize;
    let mut tris = 0usize;
    for p in &s.pieces {
        total_vol += volume(p);
        tris += p.tris.len();
        if !split::watertight(&p.tris) {
            not_tight += 1;
        }
        let mut e: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &p.tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *e.entry((a.min(b), a.max(b))).or_default() += 1;
            }
            let c = t.map(|v| p.positions[v as usize]);
            let area = geom::area(&c);
            let le = geom::longest_edge(&c);
            if area < 0.02 * le * le {
                slivers += 1;
            }
            // Outer faces: compare with the model's normal there.
            let m = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
            if let Some((ti, b)) = bvh.nearest(&src, m) {
                let q = geom::at(&src.corners[ti as usize], b);
                if geom::len(geom::sub(q, m)) < 0.1 * h && area > 1e-6 {
                    let sc = &src.corners[ti as usize];
                    let ns = geom::normalize(geom::cross(geom::sub(sc[1], sc[0]), geom::sub(sc[2], sc[0])));
                    let nt = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
                    let a = geom::dot(ns, nt).clamp(-1.0, 1.0).acos().to_degrees();
                    ang_sum += a as f64 * area as f64;
                    ang_n += 1;
                    ang_bad += (a > 20.0) as usize;
                }
            }
        }
        nonmanifold += e.values().filter(|&&n| n > 2).count();
    }
    let model_vol: f32 = model
        .1
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| model.0[v as usize]);
            geom::dot(a, geom::cross(b, c)) / 6.0
        })
        .sum();
    // Groove depth: how far below the model surface the colour seams sink.
    // Sample the model surface densely, look for points with no piece nearby.
    let mut all = TriSoup::default();
    for p in &s.pieces {
        for t in &p.tris {
            all.push(t.map(|v| p.positions[v as usize]), 0, 0);
        }
    }
    let pb = Bvh::build(&all);
    // Corners and edges: how far each model vertex is from the pieces.
    let (mut vmax, mut vsum) = (0.0f32, 0.0f64);
    for q in &model.0 {
        if let Some((t, bb)) = pb.nearest(&all, *q) {
            let d = geom::len(geom::sub(geom::at(&all.corners[t as usize], bb), *q));
            vmax = vmax.max(d);
            vsum += d as f64;
        }
    }
    print!("vertex gap mean {:.3} max {:.3}; skin {} | ", vsum / model.0.len() as f64, vmax, s.exact_skin.unwrap_or(0));
    let (mut gmax, mut gsum, mut gn) = (0.0f32, 0.0f64, 0usize);
    for (ti, c) in src.corners.iter().enumerate() {
        for b in [[1.0 / 3.0; 3], [0.6, 0.2, 0.2], [0.2, 0.6, 0.2], [0.2, 0.2, 0.6]] {
            let q = geom::at(c, b);
            if let Some((t, bb)) = pb.nearest(&all, q) {
                let d = geom::len(geom::sub(geom::at(&all.corners[t as usize], bb), q));
                gmax = gmax.max(d);
                gsum += d as f64;
                gn += 1;
            }
        }
        let _ = ti;
    }
    println!(
        "{name}: {} pieces, {} tris, h {:.2}, vol {:.0}/{:.0} ({:+.1}%), not tight {}, non-manifold edges {}, slivers {:.1}%, outer normal err {:.1}° (>20°: {:.1}%), surface gap mean {:.3} max {:.3} mm, merged {} dropped {}",
        s.pieces.len(), tris, h, total_vol, model_vol, 100.0 * (total_vol / model_vol - 1.0), not_tight, nonmanifold,
        100.0 * slivers as f64 / tris.max(1) as f64, ang_sum / ang_n.max(1) as f64,
        100.0 * ang_bad as f64 / ang_n.max(1) as f64, gsum / gn.max(1) as f64, gmax, s.merged, s.dropped
    );
}

fn picture(path: &str, s: &split::Split, explode: f32, az: f32, el: f32) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &s.pieces {
        for v in &p.positions {
            lo = std::array::from_fn(|a| lo[a].min(v[a]));
            hi = std::array::from_fn(|a| hi[a].max(v[a]));
        }
    }
    let centre = geom::scale(geom::add(lo, hi), 0.5);
    let moved: Vec<Vec<Vec3>> = s
        .pieces
        .iter()
        .map(|p| {
            let n = p.positions.len().max(1) as f32;
            let c = geom::scale(p.positions.iter().fold([0.0; 3], |a, &v| geom::add(a, v)), 1.0 / n);
            let d = geom::sub(c, centre);
            let off = if geom::len(d) > 1e-3 { geom::scale(geom::normalize(d), explode) } else { [0.0; 3] };
            p.positions.iter().map(|&v| geom::add(v, off)).collect()
        })
        .collect();
    let items: Vec<Item> = s
        .pieces
        .iter()
        .zip(&moved)
        .enumerate()
        .map(|(i, (p, m))| Item {
            positions: m,
            tris: &p.tris,
            rgb: PAL[p.slot as usize % PAL.len()],
            id: i as u32,
            style: Style::Normal,
        })
        .collect();
    let e = explode * 1.5;
    let cam = Camera::new(az, el, geom::sub(lo, [e; 3]), geom::add(hi, [e; 3]), 900, 900);
    let f = render::render(&cam, &items, 2);
    let mut out = std::fs::File::create(path).unwrap();
    write!(out, "P6\n{} {}\n255\n", f.width, f.height).unwrap();
    out.write_all(&f.rgb).unwrap();
}

fn run(name: &str, model: &Mesh, label: &(dyn Fn(Vec3) -> u8 + Sync), params: SplitParams, dir: &str) {
    let s = soup(model);
    let lab = |t: u32, b: Bary| label(geom::at(&s.corners[t as usize], b));
    let t0 = std::time::Instant::now();
    let r = split::split(&s, lab, params);
    print!("[{:>6.2}s] pins {:?} unpinned {} ", t0.elapsed().as_secs_f32(), r.pins, r.unpinned);
    for (i, p) in r.pieces.iter().enumerate() {
        let mut f = std::fs::File::create(format!("{dir}/{name}_{i}.stl")).unwrap();
        writeln!(f, "solid p").unwrap();
        for t in &p.tris {
            let c = t.map(|v| p.positions[v as usize]);
            writeln!(f, "facet normal 0 0 0\nouter loop").unwrap();
            for v in c {
                writeln!(f, "vertex {} {} {}", v[0], v[1], v[2]).unwrap();
            }
            writeln!(f, "endloop\nendfacet").unwrap();
        }
        writeln!(f, "endsolid p").unwrap();
    }
    metrics(name, model, &r);
    if name == "halves" && r.pieces.len() == 2 {
        let mut other = TriSoup::default();
        for t in &r.pieces[1].tris {
            other.push(t.map(|v| r.pieces[1].positions[v as usize]), 0, 0);
        }
        let ob = Bvh::build(&other);
        let d: Vec<f32> = r.pieces[0]
            .positions
            .iter()
            .filter(|v| v[2] > 15.2)
            .map(|&q| {
                let (t, b) = ob.nearest(&other, q).unwrap();
                geom::len(geom::sub(geom::at(&other.corners[t as usize], b), q))
            })
            .collect();
        for (q, dd) in r.pieces[0].positions.iter().filter(|v| v[2] > 15.2).zip(&d) {
            if *dd < 0.12 {
                println!("    close {q:?} {dd:.3}");
            }
        }
        let mean = d.iter().sum::<f32>() / d.len().max(1) as f32;
        println!(
            "  pin clearance over {} vertices: min {:.3} mean {:.3} max {:.3}",
            d.len(),
            d.iter().cloned().fold(f32::MAX, f32::min),
            mean,
            d.iter().cloned().fold(0.0, f32::max)
        );
    }
    picture(&format!("{dir}/{name}_a.ppm"), &r, 0.0, 30.0, 25.0);
    picture(&format!("{dir}/{name}_x.ppm"), &r, 8.0, 30.0, 25.0);
    picture(&format!("{dir}/{name}_y.ppm"), &r, 8.0, 30.0, -35.0);
    for (i, p) in r.pieces.iter().enumerate() {
        let one = split::Split {
            pieces: vec![Piece {
                slot: p.slot,
                positions: p.positions.clone(),
                tris: p.tris.clone(),
                volume_mm3: 0.0,
                contacts: vec![],
            }],
            voxel_mm: r.voxel_mm,
            ..Default::default()
        };
        picture(&format!("{dir}/{name}_p{i}.ppm"), &one, 0.0, 200.0, 50.0);
    }
}

fn main() {
    if std::env::var("BENCH").is_ok() {
        bench();
        return;
    }
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let voxel: f32 = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(0.5);
    let pins = std::env::var("PINS").is_ok().then(split::PinParams::default);
    let exact_skin = std::env::var("SKIN").is_ok();
    let p = SplitParams { voxel_mm: voxel, min_piece_mm3: 30.0, pins, exact_skin, ..Default::default() };
    let halves = cuboid([0.0; 3], [30.0, 30.0, 30.0], 10);
    run("halves", &halves, &|q: Vec3| u8::from(q[2] > 15.0), p, &dir);
    // A ball with a band, a spot and a diagonal stripe.
    let ball = icosphere(15.0, [0.0, 0.0, 15.0], 5);
    let ball_label = |q: Vec3| -> u8 {
        if (q[2] - 15.0).abs() < 3.0 {
            1
        } else if geom::len(geom::sub(q, [8.0, -10.0, 22.0])) < 5.0 {
            2
        } else if (q[0] + q[1] - 2.0 * (q[2] - 15.0)).abs() < 2.5 {
            3
        } else {
            0
        }
    };
    if std::env::var("BOXONLY").is_err() {
        run("ball", &ball, &ball_label, p, &dir);
    }
    if std::env::var("BIG").is_ok() {
        let big = icosphere(60.0, [0.0, 0.0, 60.0], 6);
        let big_label = |q: Vec3| -> u8 { ball_label([q[0] / 4.0, q[1] / 4.0, q[2] / 4.0]) };
        run("big", &big, &big_label, p, &dir);
    }
    // A box with a painted top and a stripe down one side: sharp edges.
    let bx = cuboid([0.0; 3], [20.0, 20.0, 20.0], 20);
    let box_label = |q: Vec3| -> u8 {
        if q[2] > 19.99 {
            1
        } else if (q[0] - 10.0).abs() < 2.0 && q[1] < 0.01 {
            2
        } else {
            0
        }
    };
    run("box", &bx, &box_label, p, &dir);
    // A body on thin legs, legs another colour: thin features.
    let mut parts = vec![icosphere(10.0, [0.0, 0.0, 14.0], 4)];
    for i in 0..6 {
        let a = i as f32 / 6.0 * std::f32::consts::TAU;
        parts.push(cylinder(1.2, [8.0 * a.cos(), 8.0 * a.sin()], 0.0, 10.0, 24));
    }
    let bug = join(&parts);
    let bug_label = |q: Vec3| -> u8 {
        if q[2] < 7.0 {
            1
        } else if q[2] > 21.0 {
            2
        } else {
            0
        }
    };
    if std::env::var("BOXONLY").is_err() {
        run("bug", &bug, &bug_label, p, &dir);
    }
}

#[allow(dead_code)]
pub fn bench() {
    let bx = cuboid([0.0; 3], [20.0, 20.0, 20.0], 20);
    let s = soup(&bx);
    let b = Bvh::build(&s);
    let t = std::time::Instant::now();
    let mut acc = 0.0;
    for i in 0..100_000 {
        let q = [(i % 97) as f32 * 0.2, 20.0 - 0.1, (i % 89) as f32 * 0.2];
        if let Some((tri, bb)) = b.nearest(&s, q) {
            acc += bb[0] + tri as f32 * 0.0;
        }
    }
    println!("100k nearest: {:?} {acc}", t.elapsed());
    let t = std::time::Instant::now();
    for i in 0..100_000 {
        let q = [(i % 97) as f32 * 0.2, 20.0 - 0.1, (i % 89) as f32 * 0.2];
        if let Some((tri, bb)) = b.nearest(&s, q) {
            acc += bb[0] + tri as f32 * 0.0;
        }
    }
    println!("100k nearest unbounded: {:?} {acc}", t.elapsed());
}
