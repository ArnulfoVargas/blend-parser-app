//! Turns split pieces into a print kit: every piece turned to rest on its
//! broadest flat face, packed onto plates that hold a single colour, and an
//! assembly order that always adds the piece touching the built part most.

use std::collections::HashMap;

use crate::geom::{self, Vec3};
use crate::split::{Piece, Split};

pub type Mat3 = [[f32; 3]; 3];

pub struct Placed {
    /// Print orientation: `rot * p` for a model-space point.
    pub rot: Mat3,
    /// Positions in print orientation, centred on XY with the base on Z = 0.
    pub positions: Vec<Vec3>,
    pub size: Vec3,
    pub plate: usize,
    /// Centre of the piece on its plate, in plate millimetres.
    pub at: [f32; 2],
    /// False when the piece is larger than the plate.
    pub fits: bool,
}

pub struct Plate {
    pub slot: u8,
    pub pieces: Vec<usize>,
}

pub struct Step {
    pub piece: usize,
    /// Pieces already in place that this one is glued to, most contact first.
    pub joins: Vec<usize>,
}

pub struct Kit {
    pub pieces: Vec<Piece>,
    pub placed: Vec<Placed>,
    pub plates: Vec<Plate>,
    pub steps: Vec<Step>,
    pub bed: (f32, f32),
    pub voxel_mm: f32,
}

const MARGIN: f32 = 8.0;
const GAP: f32 = 5.0;
/// Contact below this many mm² is a touch, not a joint worth gluing.
const MIN_JOINT_MM2: f32 = 2.0;

pub fn kit(split: Split, bed: (f32, f32)) -> Kit {
    let mut placed: Vec<Placed> = split.pieces.iter().map(orient).collect();
    let plates = pack(&split.pieces, &mut placed, bed);
    let steps = assembly(&split.pieces);
    Kit { pieces: split.pieces, placed, plates, steps, bed, voxel_mm: split.voxel_mm }
}

pub fn apply(m: &Mat3, p: Vec3) -> Vec3 {
    [geom::dot(m[0], p), geom::dot(m[1], p), geom::dot(m[2], p)]
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// Rotation taking unit vector `d` to -Z.
fn to_down(d: Vec3) -> Mat3 {
    let t = [0.0, 0.0, -1.0];
    let c = geom::dot(d, t);
    if c > 0.9999 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    if c < -0.9999 {
        return [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
    }
    // Rodrigues around d × t.
    let k = geom::normalize(geom::cross(d, t));
    let s = (1.0 - c * c).sqrt();
    let v = 1.0 - c;
    [
        [c + k[0] * k[0] * v, k[0] * k[1] * v - k[2] * s, k[0] * k[2] * v + k[1] * s],
        [k[1] * k[0] * v + k[2] * s, c + k[1] * k[1] * v, k[1] * k[2] * v - k[0] * s],
        [k[2] * k[0] * v - k[1] * s, k[2] * k[1] * v + k[0] * s, c + k[2] * k[2] * v],
    ]
}

fn yaw(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Flat area that would touch the plate resting on direction `d`, minus a
/// penalty for the area that would need support.
fn rest_score(p: &Piece, normals: &[(Vec3, f32)], d: Vec3) -> f32 {
    let far = p.positions.iter().map(|&v| geom::dot(v, d)).fold(f32::MIN, f32::max);
    let (mut contact, mut overhang) = (0.0, 0.0);
    for (t, &(n, area)) in p.tris.iter().zip(normals) {
        let facing = geom::dot(n, d);
        if facing > 0.985 && t.iter().all(|&v| geom::dot(p.positions[v as usize], d) > far - 0.3) {
            contact += area;
        } else if facing > 0.707 {
            overhang += area;
        }
    }
    contact - 0.5 * overhang
}

fn orient(p: &Piece) -> Placed {
    let normals: Vec<(Vec3, f32)> = p
        .tris
        .iter()
        .map(|t| {
            let c = t.map(|v| p.positions[v as usize]);
            let n = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
            (geom::normalize(n), 0.5 * geom::len(n))
        })
        .collect();
    // Candidates: the six axes and the directions of the largest flat areas.
    let mut bins: HashMap<[i32; 3], (Vec3, f32)> = HashMap::new();
    for &(n, a) in &normals {
        let key = n.map(|x| (x * 8.0).round() as i32);
        let e = bins.entry(key).or_insert(([0.0; 3], 0.0));
        e.0 = geom::add(e.0, geom::scale(n, a));
        e.1 += a;
    }
    let mut flats: Vec<(Vec3, f32)> = bins.into_values().collect();
    flats.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut candidates: Vec<Vec3> =
        vec![[0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
    candidates.extend(flats.iter().take(8).map(|f| geom::normalize(f.0)));
    let down = candidates
        .into_iter()
        .map(|d| (rest_score(p, &normals, d), d))
        .fold((f32::MIN, [0.0, 0.0, -1.0]), |best, c| if c.0 > best.0 + 1e-3 { c } else { best });
    let tilt = to_down(down.1);
    let tilted: Vec<Vec3> = p.positions.iter().map(|&v| apply(&tilt, v)).collect();

    // Turn about Z for the smallest footprint, wider than deep.
    let footprint = |a: f32| {
        let (s, c) = a.sin_cos();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for v in &tilted {
            let q = [c * v[0] - s * v[1], s * v[0] + c * v[1]];
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        (hi[0] - lo[0], hi[1] - lo[1])
    };
    let mut best = (f32::MAX, 0.0f32);
    for k in 0..18 {
        let a = (k as f32 * 5.0).to_radians();
        let (w, d) = footprint(a);
        if w * d < best.0 - 1e-3 {
            best = (w * d, a);
        }
    }
    let (w, d) = footprint(best.1);
    let angle = if w < d { best.1 + std::f32::consts::FRAC_PI_2 } else { best.1 };
    let rot = mul(&yaw(angle), &tilt);

    let mut positions: Vec<Vec3> = p.positions.iter().map(|&v| apply(&rot, v)).collect();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &positions {
        lo = std::array::from_fn(|a| lo[a].min(v[a]));
        hi = std::array::from_fn(|a| hi[a].max(v[a]));
    }
    let shift = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, lo[2]];
    for v in &mut positions {
        *v = geom::sub(*v, shift);
    }
    Placed { rot, positions, size: geom::sub(hi, lo), plate: 0, at: [0.0; 2], fits: true }
}

/// Shelf packing, one colour per plate, each plate's load centred.
fn pack(pieces: &[Piece], placed: &mut [Placed], bed: (f32, f32)) -> Vec<Plate> {
    let (w, d) = (bed.0 - 2.0 * MARGIN, bed.1 - 2.0 * MARGIN);
    let mut plates: Vec<Plate> = vec![];
    let mut slots: Vec<u8> = pieces.iter().map(|p| p.slot).collect();
    slots.sort_unstable();
    slots.dedup();
    for slot in slots {
        let mut ids: Vec<usize> = (0..pieces.len()).filter(|&i| pieces[i].slot == slot).collect();
        ids.sort_by(|&a, &b| placed[b].size[1].total_cmp(&placed[a].size[1]));
        let first = plates.len();
        let (mut x, mut y, mut shelf) = (0.0f32, 0.0f32, 0.0f32);
        let mut current: Option<usize> = None;
        for i in ids {
            let s = placed[i].size;
            if s[0] > w || s[1] > d {
                placed[i].fits = false;
                placed[i].plate = plates.len();
                placed[i].at = [bed.0 * 0.5, bed.1 * 0.5];
                plates.push(Plate { slot, pieces: vec![i] });
                continue;
            }
            if current.is_some() && x + s[0] > w {
                (x, y, shelf) = (0.0, y + shelf + GAP, 0.0);
            }
            if current.is_none() || y + s[1] > d {
                current = Some(plates.len());
                plates.push(Plate { slot, pieces: vec![] });
                (x, y, shelf) = (0.0, 0.0, 0.0);
            }
            let k = current.unwrap_or_default();
            placed[i].plate = k;
            placed[i].at = [MARGIN + x + s[0] * 0.5, MARGIN + y + s[1] * 0.5];
            plates[k].pieces.push(i);
            x += s[0] + GAP;
            shelf = shelf.max(s[1]);
        }
        for plate in &plates[first..] {
            let ids = &plate.pieces;
            if ids.iter().any(|&i| !placed[i].fits) {
                continue;
            }
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for &i in ids {
                let (c, s) = (placed[i].at, placed[i].size);
                lo = [lo[0].min(c[0] - s[0] * 0.5), lo[1].min(c[1] - s[1] * 0.5)];
                hi = [hi[0].max(c[0] + s[0] * 0.5), hi[1].max(c[1] + s[1] * 0.5)];
            }
            let shift = [bed.0 * 0.5 - (lo[0] + hi[0]) * 0.5, bed.1 * 0.5 - (lo[1] + hi[1]) * 0.5];
            for &i in ids {
                placed[i].at = [placed[i].at[0] + shift[0], placed[i].at[1] + shift[1]];
            }
        }
    }
    plates
}

/// Starts from the largest piece and keeps adding the piece with the most
/// contact with what is already built; a piece touching nothing built comes
/// last, starting its own group.
fn assembly(pieces: &[Piece]) -> Vec<Step> {
    let n = pieces.len();
    let mut done = vec![false; n];
    let mut touch = vec![0.0f32; n];
    let mut steps = Vec::with_capacity(n);
    while steps.len() < n {
        let next = (0..n)
            .filter(|&i| !done[i])
            .max_by(|&a, &b| touch[a].total_cmp(&touch[b]).then(pieces[a].volume_mm3.total_cmp(&pieces[b].volume_mm3)))
            .unwrap_or_default();
        let joins =
            pieces[next].contacts.iter().filter(|&&(j, a)| done[j] && a >= MIN_JOINT_MM2).map(|&(j, _)| j).collect();
        done[next] = true;
        for &(j, a) in &pieces[next].contacts {
            touch[j] += a;
        }
        steps.push(Step { piece: next, joins });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::tests::cuboid;

    fn piece(slot: u8, lo: Vec3, hi: Vec3, contacts: Vec<(usize, f32)>) -> Piece {
        let (positions, tris) = cuboid(lo, hi);
        let s = geom::sub(hi, lo);
        Piece { slot, positions, tris, volume_mm3: s[0] * s[1] * s[2], contacts }
    }

    #[test]
    fn a_tall_slab_lies_down_on_its_broad_face() {
        let p = piece(0, [0.0; 3], [2.0, 30.0, 20.0], vec![]);
        let placed = orient(&p);
        // 2 mm thick, so it prints lying flat; wider than deep.
        assert!((placed.size[2] - 2.0).abs() < 1e-3, "{:?}", placed.size);
        assert!(placed.size[0] >= placed.size[1]);
        assert!(placed.positions.iter().all(|v| v[2] >= -1e-4));
    }

    #[test]
    fn plates_hold_one_colour_and_stay_on_the_bed() {
        let pieces: Vec<Piece> = (0..12).map(|i| piece((i % 2) as u8, [0.0; 3], [40.0, 30.0, 5.0], vec![])).collect();
        let k = kit(Split { pieces, voxel_mm: 0.5 }, (120.0, 120.0));
        assert!(k.plates.len() >= 2);
        for plate in &k.plates {
            assert!(plate.pieces.iter().all(|&i| k.pieces[i].slot == plate.slot));
            for &i in &plate.pieces {
                let (c, s) = (k.placed[i].at, k.placed[i].size);
                assert!(c[0] - s[0] / 2.0 >= -1e-3 && c[0] + s[0] / 2.0 <= 120.0 + 1e-3);
                assert!(c[1] - s[1] / 2.0 >= -1e-3 && c[1] + s[1] / 2.0 <= 120.0 + 1e-3);
            }
        }
        // No two pieces on a plate overlap.
        for plate in &k.plates {
            for (n, &a) in plate.pieces.iter().enumerate() {
                for &b in &plate.pieces[n + 1..] {
                    let (pa, pb) = (&k.placed[a], &k.placed[b]);
                    let apart = (0..2).any(|x| (pa.at[x] - pb.at[x]).abs() >= (pa.size[x] + pb.size[x]) / 2.0 - 1e-3);
                    assert!(apart);
                }
            }
        }
    }

    #[test]
    fn assembly_grows_from_the_largest_piece_through_contacts() {
        // Chain 1 - 0 - 2, with 0 the largest.
        let pieces = vec![
            piece(0, [0.0; 3], [10.0, 10.0, 10.0], vec![(1, 50.0), (2, 20.0)]),
            piece(1, [0.0; 3], [2.0, 2.0, 2.0], vec![(0, 50.0)]),
            piece(2, [0.0; 3], [3.0, 3.0, 3.0], vec![(0, 20.0)]),
        ];
        let steps = assembly(&pieces);
        assert_eq!(steps.iter().map(|s| s.piece).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(steps[0].joins.is_empty());
        assert_eq!(steps[2].joins, vec![0]);
    }
}
