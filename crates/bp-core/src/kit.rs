//! Turns split pieces into a print kit: every piece turned to rest on its
//! broadest flat face and to its smallest footprint, packed onto plates
//! (MaxRects, either way round, clear of the margin, the printer's excluded
//! zones and, where colours mix, the prime tower), and an assembly order that
//! always adds the piece touching the built part most.

use std::collections::HashMap;

use crate::geom::{self, Vec3};
use crate::split::{Piece, PinJoin, Split};

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
    /// Area the piece covers on the plate (its footprint's convex hull), mm².
    pub footprint_mm2: f32,
}

pub struct Plate {
    /// The colours on the plate, most pieces first; one unless colours mix.
    pub slots: Vec<u8>,
    pub pieces: Vec<usize>,
    /// Share of the plate's free room the pieces cover, 0–1.
    pub occupancy: f32,
}

impl Plate {
    /// The plate's main colour.
    pub fn slot(&self) -> u8 {
        self.slots[0]
    }

    /// More than one colour: the plate swaps filament and needs the tower.
    pub fn mixed(&self) -> bool {
        self.slots.len() > 1
    }
}

/// Where pieces may go, in plate millimetres (x0, y0, x1, y1 rectangles).
#[derive(Clone, Debug, PartialEq)]
pub struct Bed {
    pub area: [f32; 4],
    /// Zones the printer keeps free.
    pub excluded: Vec<[f32; 4]>,
    /// The prime tower, its gap included; kept free where colours mix.
    pub tower: Option<[f32; 4]>,
    /// Room kept to the edges, and between pieces.
    pub margin_mm: f32,
    pub gap_mm: f32,
    /// Let colours share a plate: fewer plates, but each swaps filament.
    pub mix: bool,
}

impl Bed {
    /// A `w` × `d` bed with nothing excluded and the default margin and gap.
    pub fn plain(w: f32, d: f32) -> Self {
        Self { area: [0.0, 0.0, w, d], excluded: vec![], tower: None, margin_mm: MARGIN, gap_mm: GAP, mix: false }
    }

    pub fn size(&self) -> (f32, f32) {
        (self.area[2] - self.area[0], self.area[3] - self.area[1])
    }
}

pub struct Step {
    pub piece: usize,
    /// Pieces of its sub-assembly already in place that this one is glued to,
    /// most contact first.
    pub joins: Vec<usize>,
    pub group: usize,
}

/// A sub-assembly: pieces that touch, built on their own and then glued to
/// the groups before it. Small kits are a single group.
pub struct Group {
    /// Its steps, in `Kit::steps`.
    pub steps: std::ops::Range<usize>,
    /// Where it is glued to the groups before it: (its piece, a piece already
    /// in place), most contact first. Empty for the first group.
    pub joins: Vec<(usize, usize)>,
}

pub struct Kit {
    pub pieces: Vec<Piece>,
    pub placed: Vec<Placed>,
    pub plates: Vec<Plate>,
    pub steps: Vec<Step>,
    /// Sub-assemblies, in the order they are built and joined.
    pub groups: Vec<Group>,
    pub bed: Bed,
    pub voxel_mm: f32,
    /// Crumbs that went to a neighbour, and regions too thin to make a piece.
    pub merged: usize,
    pub dropped: usize,
    /// Pins keying pieces together, and joins with no room for one.
    pub pins: Vec<PinJoin>,
    pub unpinned: usize,
}

/// Why a piece needs a look before printing it.
#[derive(Clone, Debug, PartialEq)]
pub enum Issue {
    /// Its surface does not close: the slicer may fill it wrongly.
    Open,
    /// Triangles with no area, by count.
    Degenerate(usize),
    /// Larger than the plate in any orientation tried.
    Oversize,
    /// Its thinnest side, in millimetres, under two lines of the nozzle.
    Thin(f32),
    /// Its largest side, in millimetres, under ten nozzle widths: hard to print and to handle.
    Tiny(f32),
}

/// What every piece of `kit` needs looked at, printed with a `nozzle_mm` nozzle.
pub fn check(kit: &Kit, nozzle_mm: f32) -> Vec<Vec<Issue>> {
    let line = nozzle_mm * 1.125;
    kit.pieces
        .iter()
        .zip(&kit.placed)
        .map(|(p, pl)| {
            let mut issues = vec![];
            if !crate::split::watertight(&p.tris) {
                issues.push(Issue::Open);
            }
            let flat = p
                .tris
                .iter()
                .filter(|t| {
                    geom::area(&[p.positions[t[0] as usize], p.positions[t[1] as usize], p.positions[t[2] as usize]])
                        < 1e-6
                })
                .count();
            if flat > 0 {
                issues.push(Issue::Degenerate(flat));
            }
            if !pl.fits {
                issues.push(Issue::Oversize);
            }
            let (least, most) = pl.size.iter().fold((f32::MAX, 0.0f32), |(a, b), &s| (a.min(s), b.max(s)));
            if most < 10.0 * nozzle_mm {
                issues.push(Issue::Tiny(most));
            } else if least < 2.0 * line {
                issues.push(Issue::Thin(least));
            }
            issues
        })
        .collect()
}

const MARGIN: f32 = 8.0;
const GAP: f32 = 5.0;
/// Contact below this many mm² is a touch, not a joint worth gluing.
const MIN_JOINT_MM2: f32 = 2.0;

pub fn kit(split: Split, bed: Bed) -> Kit {
    let mut placed: Vec<Placed> = split.pieces.iter().map(orient).collect();
    let plates = pack(&split.pieces, &mut placed, &bed);
    let (steps, groups) = assembly(&split.pieces);
    Kit {
        pieces: split.pieces,
        placed,
        plates,
        steps,
        groups,
        bed,
        voxel_mm: split.voxel_mm,
        merged: split.merged,
        dropped: split.dropped,
        pins: split.pins,
        unpinned: split.unpinned,
    }
}

impl Kit {
    /// The pins between pieces `a` and `b`, either way round.
    pub fn pins_between(&self, a: usize, b: usize) -> impl Iterator<Item = &PinJoin> {
        self.pins.iter().filter(move |p| (p.pin, p.socket) == (a, b) || (p.pin, p.socket) == (b, a))
    }

    /// For a step adding `piece` onto `joins`: the joined pieces whose pins
    /// point another way than the first one's, so it cannot go onto them all at once.
    pub fn crossed_pins(&self, piece: usize, joins: &[usize]) -> Vec<usize> {
        let mut first: Option<Vec3> = None;
        let mut crossed = vec![];
        for &j in joins {
            for p in self.pins_between(piece, j) {
                // The way the new piece moves to go on.
                let along = if p.socket == piece { p.axis } else { geom::scale(p.axis, -1.0) };
                match first {
                    None => first = Some(along),
                    Some(f) if geom::dot(f, along) < 0.9 && !crossed.contains(&j) => crossed.push(j),
                    _ => {}
                }
            }
        }
        crossed
    }
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

    // Turn about Z for the smallest footprint, wider than deep: the smallest
    // rectangle around the footprint has a side along one of its hull's edges.
    let hull = hull2(tilted.iter().map(|v| [v[0], v[1]]).collect());
    let footprint = |a: f32| {
        let (s, c) = a.sin_cos();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for v in &hull {
            let q = [c * v[0] - s * v[1], s * v[0] + c * v[1]];
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        (hi[0] - lo[0], hi[1] - lo[1])
    };
    let mut best = (footprint(0.0), 0.0f32);
    for (k, a) in hull.iter().enumerate() {
        let b = hull[(k + 1) % hull.len()];
        // Turning by minus the edge's angle lays it along X.
        let angle = -(b[1] - a[1]).atan2(b[0] - a[0]);
        let (w, d) = footprint(angle);
        if w * d < best.0 .0 * best.0 .1 - 1e-3 {
            best = ((w, d), angle);
        }
    }
    let ((w, d), angle) = best;
    let angle = if w < d { angle + std::f32::consts::FRAC_PI_2 } else { angle };
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
    let footprint_mm2 = area2(&hull);
    Placed { rot, positions, size: geom::sub(hi, lo), plate: 0, at: [0.0; 2], fits: true, footprint_mm2 }
}

/// Convex hull, counter-clockwise (Andrew's monotone chain).
fn hull2(mut p: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let turn = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut h: Vec<[f32; 2]> = Vec::with_capacity(2 * p.len());
    for pass in 0..2 {
        let start = h.len();
        let points: Box<dyn Iterator<Item = &[f32; 2]>> =
            if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in points {
            while h.len() >= start + 2 && turn(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

fn area2(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    0.5 * (0..n).map(|i| poly[i][0] * poly[(i + 1) % n][1] - poly[(i + 1) % n][0] * poly[i][1]).sum::<f32>().abs()
}

type Rect = [f32; 4];

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a[0] < b[2] - 1e-4 && b[0] < a[2] - 1e-4 && a[1] < b[3] - 1e-4 && b[1] < a[3] - 1e-4
}

fn inside(a: &Rect, b: &Rect) -> bool {
    a[0] >= b[0] - 1e-4 && a[1] >= b[1] - 1e-4 && a[2] <= b[2] + 1e-4 && a[3] <= b[3] + 1e-4
}

/// One plate being filled, MaxRects style: the free room is every largest
/// rectangle left, and a piece goes where the load stays most compact (the
/// smallest square around it), then where it leaves the least short side over.
/// Each piece takes its size plus the gap on its far sides, so the room runs a
/// gap past the margin and obstacles grow by a gap on their far sides too.
/// A piece's front left corner, whether it turned, and how well it fits.
type Spot = ([f32; 2], bool, (f32, f32, f32));

struct Sheet {
    free: Vec<Rect>,
    gap: f32,
    /// Around what is on the plate so far, gaps included.
    load: Option<Rect>,
}

impl Sheet {
    fn new(bed: &Bed, tower: bool) -> Self {
        let (m, g) = (bed.margin_mm.max(0.0), bed.gap_mm.max(0.0));
        let a = bed.area;
        let mut sheet = Sheet { free: vec![[a[0] + m, a[1] + m, a[2] - m + g, a[3] - m + g]], gap: g, load: None };
        let tower = bed.tower.filter(|_| tower);
        for o in bed.excluded.iter().chain(&tower) {
            sheet.take(&[o[0], o[1], o[2] + g, o[3] + g]);
        }
        sheet
    }

    /// The best spot for a `w` × `d` piece, either way round: its front left
    /// corner, whether it turned, and how well it fits (less is better).
    fn spot(&self, w: f32, d: f32) -> Option<Spot> {
        let mut best: Option<Spot> = None;
        for f in &self.free {
            let (fw, fd) = (f[2] - f[0], f[3] - f[1]);
            for turned in [false, true] {
                let (pw, pd) = if turned { (d + self.gap, w + self.gap) } else { (w + self.gap, d + self.gap) };
                if pw > fw + 1e-4 || pd > fd + 1e-4 {
                    continue;
                }
                let (a, b) = (fw - pw, fd - pd);
                // The free rectangle's corner, or lined up with the load's front or
                // left edge: anywhere inside it is free.
                let xs = [f[0], self.load.map_or(f[0], |l| l[0].clamp(f[0], f[2] - pw))];
                let ys = [f[1], self.load.map_or(f[1], |l| l[1].clamp(f[1], f[3] - pd))];
                for x in xs {
                    for y in ys {
                        let r = [x, y, x + pw, y + pd];
                        let box_ =
                            self.load.map_or(r, |l| [l[0].min(r[0]), l[1].min(r[1]), l[2].max(r[2]), l[3].max(r[3])]);
                        // The load's longest side, to the millimetre; then the short
                        // side left over, then the long side; then nearer the front left.
                        let side = (box_[2] - box_[0]).max(box_[3] - box_[1]).round();
                        let score = (side, a.min(b), a.max(b) + 1e-3 * (x + y));
                        if best.is_none_or(|b| score < b.2) {
                            best = Some(([x, y], turned, score));
                        }
                    }
                }
            }
        }
        best
    }

    /// Puts a piece's room on the plate.
    fn place(&mut self, r: &Rect) {
        self.load = Some(self.load.map_or(*r, |l| [l[0].min(r[0]), l[1].min(r[1]), l[2].max(r[2]), l[3].max(r[3])]));
        self.take(r);
    }

    /// Takes `r` out of the free room.
    fn take(&mut self, r: &Rect) {
        let mut next = Vec::with_capacity(self.free.len() + 4);
        for f in self.free.drain(..) {
            if !overlaps(&f, r) {
                next.push(f);
                continue;
            }
            for part in
                [[f[0], f[1], r[0], f[3]], [r[2], f[1], f[2], f[3]], [f[0], f[1], f[2], r[1]], [f[0], r[3], f[2], f[3]]]
            {
                if part[2] - part[0] > 1e-3 && part[3] - part[1] > 1e-3 {
                    next.push(part);
                }
            }
        }
        // Drop rectangles inside another.
        let mut keep = vec![true; next.len()];
        for i in 0..next.len() {
            for j in 0..next.len() {
                if i != j && keep[j] && inside(&next[i], &next[j]) && (next[i] != next[j] || i > j) {
                    keep[i] = false;
                    break;
                }
            }
        }
        self.free = next.into_iter().zip(keep).filter_map(|(r, k)| k.then_some(r)).collect();
    }
}

/// Packs each colour onto its own plates (or all together when colours mix),
/// largest pieces first, then centres each plate's load as far as it can go.
fn pack(pieces: &[Piece], placed: &mut [Placed], bed: &Bed) -> Vec<Plate> {
    let mut slots: Vec<u8> = pieces.iter().map(|p| p.slot).collect();
    slots.sort_unstable();
    slots.dedup();
    let groups: Vec<Vec<usize>> = if bed.mix {
        vec![(0..pieces.len()).collect()]
    } else {
        slots.iter().map(|&s| (0..pieces.len()).filter(|&i| pieces[i].slot == s).collect()).collect()
    };
    // A tower only matters where colours can meet.
    let tower = bed.mix && slots.len() > 1;
    let mut plates: Vec<Plate> = vec![];
    for mut left in groups {
        left.sort_by(|&a, &b| {
            let (sa, sb) = (placed[a].size, placed[b].size);
            (sb[0].max(sb[1]), sb[0] * sb[1])
                .partial_cmp(&(sa[0].max(sa[1]), sa[0] * sa[1]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        while !left.is_empty() {
            let mut sheet = Sheet::new(bed, tower);
            let k = plates.len();
            let mut on: Vec<usize> = vec![];
            left.retain(|&i| {
                let s = placed[i].size;
                let Some((at, turned, _)) = sheet.spot(s[0], s[1]) else { return true };
                if turned {
                    turn_quarter(&mut placed[i]);
                }
                let s = placed[i].size;
                sheet.place(&[at[0], at[1], at[0] + s[0] + sheet.gap, at[1] + s[1] + sheet.gap]);
                placed[i].at = [at[0] + s[0] * 0.5, at[1] + s[1] * 0.5];
                placed[i].plate = k;
                on.push(i);
                false
            });
            if on.is_empty() {
                // Not even an empty plate holds it: alone, in the middle, flagged.
                let i = left.remove(0);
                let a = bed.area;
                placed[i].fits = false;
                placed[i].plate = k;
                placed[i].at = [(a[0] + a[2]) * 0.5, (a[1] + a[3]) * 0.5];
                on.push(i);
            } else {
                centre(&on, placed, bed, tower);
            }
            plates.push(plate(on, pieces, placed, bed, tower));
        }
    }
    plates
}

/// Turns a placed piece a quarter turn about Z, keeping it centred.
fn turn_quarter(p: &mut Placed) {
    let q = yaw(std::f32::consts::FRAC_PI_2);
    p.rot = mul(&q, &p.rot);
    for v in &mut p.positions {
        *v = [-v[1], v[0], v[2]];
    }
    p.size = [p.size[1], p.size[0], p.size[2]];
}

/// Moves a plate's load toward the middle of its room, as far as it stays
/// clear of the margin, excluded zones and tower.
fn centre(ids: &[usize], placed: &mut [Placed], bed: &Bed, tower: bool) {
    let rect = |p: &Placed, d: [f32; 2]| {
        [
            p.at[0] + d[0] - p.size[0] * 0.5,
            p.at[1] + d[1] - p.size[1] * 0.5,
            p.at[0] + d[0] + p.size[0] * 0.5,
            p.at[1] + d[1] + p.size[1] * 0.5,
        ]
    };
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for &i in ids {
        let r = rect(&placed[i], [0.0; 2]);
        lo = [lo[0].min(r[0]), lo[1].min(r[1])];
        hi = [hi[0].max(r[2]), hi[1].max(r[3])];
    }
    let a = bed.area;
    let want = [(a[0] + a[2] - lo[0] - hi[0]) * 0.5, (a[1] + a[3] - lo[1] - hi[1]) * 0.5];
    let (m, g) = (bed.margin_mm.max(0.0), bed.gap_mm.max(0.0));
    let room = [a[0] + m, a[1] + m, a[2] - m, a[3] - m];
    let tower = bed.tower.filter(|_| tower);
    let clear = |d: [f32; 2]| {
        ids.iter().all(|&i| {
            let r = rect(&placed[i], d);
            inside(&r, &room)
                && bed.excluded.iter().chain(&tower).all(|o| !overlaps(&r, &[o[0] - g, o[1] - g, o[2] + g, o[3] + g]))
        })
    };
    // Each axis on its own, from all the way down to not at all.
    let mut d = [0.0f32; 2];
    for axis in 0..2 {
        for k in (0..=8).rev() {
            let mut t = d;
            t[axis] = want[axis] * k as f32 / 8.0;
            if clear(t) {
                d = t;
                break;
            }
        }
    }
    for &i in ids {
        placed[i].at = [placed[i].at[0] + d[0], placed[i].at[1] + d[1]];
    }
}

fn plate(ids: Vec<usize>, pieces: &[Piece], placed: &[Placed], bed: &Bed, tower: bool) -> Plate {
    let mut count: Vec<(u8, usize)> = vec![];
    for &i in &ids {
        match count.iter_mut().find(|c| c.0 == pieces[i].slot) {
            Some(c) => c.1 += 1,
            None => count.push((pieces[i].slot, 1)),
        }
    }
    count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (w, d) = bed.size();
    let m = bed.margin_mm.max(0.0);
    let room = [bed.area[0] + m, bed.area[1] + m, bed.area[2] - m, bed.area[3] - m];
    let cut = |o: &[f32; 4]| {
        let (x, y) =
            ((o[2].min(room[2]) - o[0].max(room[0])).max(0.0), (o[3].min(room[3]) - o[1].max(room[1])).max(0.0));
        x * y
    };
    let tower = bed.tower.filter(|_| tower);
    let free = ((w - 2.0 * m).max(0.0) * (d - 2.0 * m).max(0.0)
        - bed.excluded.iter().chain(&tower).map(cut).sum::<f32>())
    .max(1.0);
    let used: f32 = ids.iter().map(|&i| placed[i].footprint_mm2).sum();
    Plate { slots: count.into_iter().map(|c| c.0).collect(), pieces: ids, occupancy: (used / free).min(1.0) }
}

/// Kits up to this many pieces are built as one; larger ones in sub-assemblies.
const ONE_GROUP_MAX: usize = 18;
/// Joints between sub-assemblies a step names, at most.
const GROUP_JOINS: usize = 4;

/// Builds the kit in sub-assemblies of pieces that touch (one for small
/// kits), each grown from its largest piece; then joins them, each group to
/// the ones it touches most.
fn assembly(pieces: &[Piece]) -> (Vec<Step>, Vec<Group>) {
    let n = pieces.len();
    let contact = |a: usize, b: usize| pieces[a].contacts.iter().find(|c| c.0 == b).map_or(0.0, |c| c.1);
    let groups = clusters(pieces);
    // Join order: the largest group first, then the one touching the built groups most.
    let mut order: Vec<usize> = vec![];
    let mut in_place = vec![false; groups.len()];
    let group_of: Vec<usize> = {
        let mut g = vec![0; n];
        for (k, ids) in groups.iter().enumerate() {
            for &i in ids {
                g[i] = k;
            }
        }
        g
    };
    while order.len() < groups.len() {
        let touch = |k: usize| -> f32 {
            groups[k]
                .iter()
                .flat_map(|&i| pieces[i].contacts.iter())
                .filter(|c| in_place[group_of[c.0]] && c.1 >= MIN_JOINT_MM2)
                .map(|c| c.1)
                .sum()
        };
        let size = |k: usize| groups[k].iter().map(|&i| pieces[i].volume_mm3).sum::<f32>();
        let next = (0..groups.len())
            .filter(|&k| !in_place[k])
            .max_by(|&a, &b| touch(a).total_cmp(&touch(b)).then(size(a).total_cmp(&size(b))).then(b.cmp(&a)))
            .unwrap_or_default();
        in_place[next] = true;
        order.push(next);
    }

    let mut steps = Vec::with_capacity(n);
    let mut out = Vec::with_capacity(groups.len());
    let mut placed = vec![false; n];
    for (g, &k) in order.iter().enumerate() {
        let ids = &groups[k];
        let mut joins: Vec<(usize, usize, f32)> = ids
            .iter()
            .flat_map(|&i| pieces[i].contacts.iter().map(move |c| (i, c.0, c.1)))
            .filter(|&(_, j, a)| placed[j] && a >= MIN_JOINT_MM2)
            .collect();
        joins.sort_by(|a, b| b.2.total_cmp(&a.2));
        joins.truncate(GROUP_JOINS);
        let start = steps.len();
        let mut done = vec![false; n];
        let mut touch = vec![0.0f32; n];
        for _ in 0..ids.len() {
            let next = ids
                .iter()
                .copied()
                .filter(|&i| !done[i])
                .max_by(|&a, &b| {
                    // Ties go to the lower label, so equal pieces build in order.
                    touch[a]
                        .total_cmp(&touch[b])
                        .then(pieces[a].volume_mm3.total_cmp(&pieces[b].volume_mm3))
                        .then(b.cmp(&a))
                })
                .unwrap_or_default();
            let mut with: Vec<usize> = pieces[next]
                .contacts
                .iter()
                .filter(|&&(j, a)| done[j] && a >= MIN_JOINT_MM2)
                .map(|&(j, _)| j)
                .collect();
            with.sort_by(|&a, &b| contact(next, b).total_cmp(&contact(next, a)));
            done[next] = true;
            for &(j, a) in &pieces[next].contacts {
                touch[j] += a;
            }
            steps.push(Step { piece: next, joins: with, group: g });
        }
        for &i in ids {
            placed[i] = true;
        }
        out.push(Group { steps: start..steps.len(), joins: joins.into_iter().map(|(a, b, _)| (a, b)).collect() });
    }
    (steps, out)
}

/// Pieces in sub-assemblies of about `√n · 1.6` (8 to 16) that touch: each
/// grows from the largest piece left through the strongest contacts; a group
/// too small goes to the neighbour it touches most.
fn clusters(pieces: &[Piece]) -> Vec<Vec<usize>> {
    let n = pieces.len();
    if n <= ONE_GROUP_MAX {
        return vec![(0..n).collect()];
    }
    let target = ((n as f32).sqrt() * 1.6).round().clamp(8.0, 16.0) as usize;
    let mut by_size: Vec<usize> = (0..n).collect();
    by_size.sort_by(|&a, &b| pieces[b].volume_mm3.total_cmp(&pieces[a].volume_mm3));
    let mut group = vec![usize::MAX; n];
    let mut groups: Vec<Vec<usize>> = vec![];
    for &seed in &by_size {
        if group[seed] != usize::MAX {
            continue;
        }
        let k = groups.len();
        let mut ids = vec![seed];
        group[seed] = k;
        let mut touch: HashMap<usize, f32> = HashMap::new();
        loop {
            for &(j, a) in &pieces[*ids.last().unwrap_or(&seed)].contacts {
                if group[j] == usize::MAX && a >= MIN_JOINT_MM2 {
                    *touch.entry(j).or_default() += a;
                }
            }
            if ids.len() >= target {
                break;
            }
            let Some((&next, _)) = touch.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))) else { break };
            touch.remove(&next);
            group[next] = k;
            ids.push(next);
        }
        groups.push(ids);
    }
    // Fold small groups into the neighbour they touch most.
    loop {
        let small =
            (0..groups.len()).filter(|&k| !groups[k].is_empty() && groups[k].len() < target / 3).find_map(|k| {
                let mut best: Option<(usize, f32)> = None;
                for &i in &groups[k] {
                    for &(j, a) in &pieces[i].contacts {
                        let o = group[j];
                        if o != k && a >= MIN_JOINT_MM2 {
                            let total = a + best.filter(|b| b.0 == o).map_or(0.0, |b| b.1);
                            if best.is_none_or(|b| total > b.1) {
                                best = Some((o, total));
                            }
                        }
                    }
                }
                best.map(|b| (k, b.0))
            });
        let Some((from, to)) = small else { break };
        let moved = std::mem::take(&mut groups[from]);
        for &i in &moved {
            group[i] = to;
        }
        groups[to].extend(moved);
    }
    groups.retain(|g| !g.is_empty());
    groups
}

/// The triangles of `a` lying on `b`'s surface, within `tol` mm: where glue goes.
pub fn contact_faces(a: &Piece, b: &Piece, tol: f32) -> Vec<usize> {
    let cell = tol.max(1e-3) * 4.0;
    let key = |p: Vec3| p.map(|x| (x / cell).floor() as i32);
    let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (t, tri) in b.tris.iter().enumerate() {
        let c = tri.map(|v| b.positions[v as usize]);
        let lo = key(std::array::from_fn(|k| c[0][k].min(c[1][k]).min(c[2][k]) - tol));
        let hi = key(std::array::from_fn(|k| c[0][k].max(c[1][k]).max(c[2][k]) + tol));
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    grid.entry([x, y, z]).or_default().push(t);
                }
            }
        }
    }
    let normal = |c: [Vec3; 3]| geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
    a.tris
        .iter()
        .enumerate()
        .filter(|(_, tri)| {
            let c = tri.map(|v| a.positions[v as usize]);
            let mid = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
            let na = normal(c);
            grid.get(&key(mid)).is_some_and(|ts| {
                ts.iter().any(|&t| {
                    let d = b.tris[t].map(|v| b.positions[v as usize]);
                    let nb = normal(d);
                    // Facing each other, in the same plane, inside the triangle.
                    geom::dot(na, nb) < -0.7 && geom::dot(geom::sub(mid, d[0]), nb).abs() <= tol && {
                        let inside = |p: Vec3, q: Vec3| {
                            geom::dot(geom::cross(geom::sub(q, p), geom::sub(mid, p)), nb)
                                >= -tol * geom::len(geom::sub(q, p))
                        };
                        inside(d[0], d[1]) && inside(d[1], d[2]) && inside(d[2], d[0])
                    }
                })
            })
        })
        .map(|(t, _)| t)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossed_pins_are_the_ones_that_point_another_way() {
        let mut k = kit(Split::default(), Bed::plain(100.0, 100.0));
        k.pins = vec![
            // Piece 0 slides down onto 1 and onto 2: the same way.
            PinJoin { pin: 1, socket: 0, axis: [0.0, 0.0, 1.0] },
            PinJoin { pin: 2, socket: 0, axis: [0.0, 0.0, 1.0] },
            // ...but onto 3 sideways.
            PinJoin { pin: 0, socket: 3, axis: [1.0, 0.0, 0.0] },
        ];
        assert!(k.crossed_pins(0, &[1, 2]).is_empty());
        assert_eq!(k.crossed_pins(0, &[1, 3]), vec![3]);
        assert_eq!(k.pins_between(3, 0).count(), 1);
    }

    #[test]
    fn check_names_what_to_look_at() {
        use crate::split::tests::cuboid;
        let piece = |lo, hi| {
            let (positions, tris) = cuboid(lo, hi);
            Piece { slot: 0, positions, tris, volume_mm3: 1.0, contacts: vec![] }
        };
        let mut open = piece([0.0; 3], [20.0; 3]);
        open.tris.pop();
        let mut flat = piece([0.0; 3], [20.0; 3]);
        flat.tris.push([0, 0, 1]);
        let pieces = vec![
            piece([0.0; 3], [20.0; 3]),
            piece([0.0; 3], [0.6, 20.0, 20.0]),
            piece([0.0; 3], [3.0; 3]),
            open,
            flat,
            piece([0.0; 3], [300.0, 300.0, 5.0]),
        ];
        let k =
            kit(Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, Bed::plain(256.0, 256.0));
        let issues = check(&k, 0.4);
        assert!(issues[0].is_empty(), "{:?}", issues[0]);
        assert!(matches!(issues[1].as_slice(), [Issue::Thin(t)] if (*t - 0.6).abs() < 1e-3), "{:?}", issues[1]);
        assert!(matches!(issues[2].as_slice(), [Issue::Tiny(_)]), "{:?}", issues[2]);
        assert!(issues[3].contains(&Issue::Open));
        assert!(issues[4].contains(&Issue::Degenerate(1)) && issues[4].contains(&Issue::Open), "{:?}", issues[4]);
        assert!(issues[5].contains(&Issue::Oversize));
    }
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
        let k =
            kit(Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, Bed::plain(120.0, 120.0));
        assert!(k.plates.len() >= 2);
        for plate in &k.plates {
            assert!(plate.pieces.iter().all(|&i| k.pieces[i].slot == plate.slot()));
            assert!(!plate.mixed());
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

    /// Every piece on its plate clear of the margin, the obstacles and the others.
    fn assert_clear(k: &Kit, obstacles: &[[f32; 4]]) {
        let b = &k.bed;
        let (m, g) = (b.margin_mm, b.gap_mm);
        let rect = |i: usize| {
            let (c, s) = (k.placed[i].at, k.placed[i].size);
            [c[0] - s[0] / 2.0, c[1] - s[1] / 2.0, c[0] + s[0] / 2.0, c[1] + s[1] / 2.0]
        };
        for plate in &k.plates {
            for (n, &a) in plate.pieces.iter().enumerate() {
                let r = rect(a);
                assert!(
                    inside(&r, &[b.area[0] + m, b.area[1] + m, b.area[2] - m, b.area[3] - m]),
                    "{r:?} in the margin"
                );
                for o in obstacles {
                    assert!(!overlaps(&r, &[o[0] - g, o[1] - g, o[2] + g, o[3] + g]), "{r:?} on {o:?}");
                }
                for &c in &plate.pieces[n + 1..] {
                    let q = rect(c);
                    assert!(
                        !overlaps(&[r[0] - g + 1e-3, r[1] - g + 1e-3, r[2] + g - 1e-3, r[3] + g - 1e-3], &q),
                        "{r:?} near {q:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn packs_tighter_than_shelves_and_turns_pieces_to_fit() {
        // Long sticks 150 × 20 on a 160 × 160 bed with a 5 mm margin: lying
        // along X they fit five deep; the 100 × 60 blocks then fill the rest.
        let mut pieces: Vec<Piece> = (0..5).map(|_| piece(0, [0.0; 3], [150.0, 20.0, 4.0], vec![])).collect();
        pieces.extend((0..2).map(|_| piece(0, [0.0; 3], [60.0, 30.0, 4.0], vec![])));
        let bed = Bed { margin_mm: 5.0, gap_mm: 2.0, ..Bed::plain(160.0, 160.0) };
        let k = kit(Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, bed);
        assert_eq!(k.plates.len(), 1, "one plate holds them all");
        assert_clear(&k, &[]);
        assert!(k.plates[0].occupancy > 0.6 && k.plates[0].occupancy <= 1.0, "{}", k.plates[0].occupancy);
        // A piece too deep one way turns the other way round to fit.
        let tall = vec![piece(0, [0.0; 3], [100.0, 40.0, 4.0], vec![])];
        let bed = Bed { margin_mm: 0.0, gap_mm: 0.0, ..Bed::plain(50.0, 120.0) };
        let k = kit(Split { pieces: tall, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, bed);
        assert!(k.placed[0].fits);
        assert!((k.placed[0].size[1] - 100.0).abs() < 1e-3, "{:?}", k.placed[0].size);
        // The STL stays in step with the turn: its extent is the size.
        let xs = k.placed[0].positions.iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        assert!((xs - 20.0).abs() < 1e-3);
    }

    #[test]
    fn keeps_off_excluded_zones_and_the_tower_where_colours_mix() {
        let pieces: Vec<Piece> = (0..16).map(|i| piece((i % 3) as u8, [0.0; 3], [30.0, 25.0, 5.0], vec![])).collect();
        let excluded = [0.0, 0.0, 60.0, 60.0];
        let tower = [150.0, 150.0, 190.0, 190.0];
        let bed = Bed { excluded: vec![excluded], tower: Some(tower), ..Bed::plain(200.0, 200.0) };
        // Apart: one colour per plate, and no tower to keep clear of.
        let apart = kit(
            Split {
                pieces: (0..16).map(|i| piece((i % 3) as u8, [0.0; 3], [30.0, 25.0, 5.0], vec![])).collect(),
                voxel_mm: 0.5,
                ..Default::default()
            },
            bed.clone(),
        );
        assert_eq!(apart.plates.len(), 3);
        assert!(apart.plates.iter().all(|p| !p.mixed()));
        assert_clear(&apart, &[excluded]);
        // Mixed: fewer plates, clear of the tower as well.
        let mixed =
            kit(Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, Bed { mix: true, ..bed });
        assert_eq!(mixed.plates.len(), 1);
        assert!(mixed.plates[0].mixed() && mixed.plates[0].slots.len() == 3);
        assert_clear(&mixed, &[excluded, tower]);
    }

    #[test]
    fn the_smallest_footprint_is_found_at_any_angle() {
        // A 60 × 10 slab turned 30° about Z: its box is 57 × 39 until turned back.
        let mut p = piece(0, [0.0; 3], [60.0, 10.0, 2.0], vec![]);
        let r = yaw(30f32.to_radians());
        p.positions = p.positions.iter().map(|&v| apply(&r, v)).collect();
        let placed = orient(&p);
        assert!((placed.size[0] - 60.0).abs() < 0.05 && (placed.size[1] - 10.0).abs() < 0.05, "{:?}", placed.size);
        assert!((placed.footprint_mm2 - 600.0).abs() < 1.0);
    }

    #[test]
    fn a_piece_larger_than_the_bed_gets_its_own_plate() {
        let pieces =
            vec![piece(0, [0.0; 3], [300.0, 40.0, 5.0], vec![]), piece(0, [0.0; 3], [20.0, 20.0, 5.0], vec![])];
        let k =
            kit(Split { pieces, voxel_mm: 0.5, merged: 0, dropped: 0, ..Default::default() }, Bed::plain(200.0, 200.0));
        assert_eq!(k.plates.len(), 2);
        assert!(!k.placed[0].fits && k.placed[1].fits);
        assert_ne!(k.placed[0].plate, k.placed[1].plate);
    }

    #[test]
    fn assembly_grows_from_the_largest_piece_through_contacts() {
        // Chain 1 - 0 - 2, with 0 the largest.
        let pieces = vec![
            piece(0, [0.0; 3], [10.0, 10.0, 10.0], vec![(1, 50.0), (2, 20.0)]),
            piece(1, [0.0; 3], [2.0, 2.0, 2.0], vec![(0, 50.0)]),
            piece(2, [0.0; 3], [3.0, 3.0, 3.0], vec![(0, 20.0)]),
        ];
        let (steps, groups) = assembly(&pieces);
        assert_eq!(groups.len(), 1);
        assert_eq!(steps.iter().map(|s| s.piece).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(steps[0].joins.is_empty());
        assert_eq!(steps[2].joins, vec![0]);
    }

    #[test]
    fn large_kits_build_in_sub_assemblies_of_pieces_that_touch() {
        // A row of 40 cubes, each glued to the next: groups are runs of the row.
        let pieces: Vec<Piece> = (0..40)
            .map(|i| {
                let x = i as f32 * 10.0;
                let mut c = vec![];
                if i > 0 {
                    c.push((i - 1, 100.0));
                }
                if i < 39 {
                    c.push((i + 1, 100.0));
                }
                piece(0, [x, 0.0, 0.0], [x + 10.0, 10.0, 10.0], c)
            })
            .collect();
        let (steps, groups) = assembly(&pieces);
        assert!(groups.len() >= 3, "{}", groups.len());
        assert_eq!(steps.len(), 40);
        let mut seen: Vec<usize> = steps.iter().map(|s| s.piece).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..40).collect::<Vec<_>>(), "every piece once");
        for (g, group) in groups.iter().enumerate() {
            let ids: Vec<usize> = steps[group.steps.clone()].iter().map(|s| s.piece).collect();
            assert!(steps[group.steps.clone()].iter().all(|s| s.group == g));
            // A run of the row: neighbours only.
            let (lo, hi) = (*ids.iter().min().unwrap(), *ids.iter().max().unwrap());
            assert_eq!(hi - lo + 1, ids.len(), "group {g}: {ids:?}");
            // Every step after the first is glued to something in its group.
            assert!(steps[group.steps.clone()]
                .iter()
                .skip(1)
                .all(|s| !s.joins.is_empty() && s.joins.iter().all(|j| ids.contains(j))));
            // Every group after the first is glued to one before it.
            assert_eq!(group.joins.is_empty(), g == 0);
            for &(mine, theirs) in &group.joins {
                assert!(ids.contains(&mine) && !ids.contains(&theirs));
                assert!(groups[..g].iter().any(|o| steps[o.steps.clone()].iter().any(|s| s.piece == theirs)));
            }
        }
    }

    #[test]
    fn glue_goes_where_two_pieces_meet() {
        let a = piece(0, [0.0; 3], [10.0, 10.0, 10.0], vec![]);
        let b = piece(1, [0.0, 0.0, 10.0], [10.0, 10.0, 14.0], vec![]);
        let faces = contact_faces(&a, &b, 0.25);
        // The cube's top: two triangles, facing +Z.
        assert_eq!(faces.len(), 2, "{faces:?}");
        for t in faces {
            assert!(a.tris[t].iter().all(|&v| (a.positions[v as usize][2] - 10.0).abs() < 1e-4));
        }
        let apart = piece(1, [0.0, 0.0, 11.0], [10.0, 10.0, 14.0], vec![]);
        assert!(contact_faces(&a, &apart, 0.25).is_empty());
    }
}
