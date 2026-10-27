//! Pins that key neighbouring pieces together, like the studs of a toy brick:
//! a round pin stands out of one piece into a socket in the other, a little
//! wider and deeper so it slides in. Each join big enough gets its pins where
//! the pieces are thickest; the socket keeps a wall all round.
//!
//! Pins are made on the voxel grid before meshing: voxels change sides so the
//! pin belongs to its piece and the gap round it to neither, and the surfaces
//! of the pin and of its socket are placed exactly on their cylinders rather
//! than half way between voxels, so the clearance holds even when it is much
//! finer than a voxel.

use std::collections::HashMap;

use super::{step, Grid, N6, NONE, OUT};
use crate::geom::{self, Vec3};

/// The gap left round a pin; neither piece's.
pub(super) const GAP: u8 = OUT - 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinParams {
    pub diameter_mm: f32,
    /// How far a pin stands out of its piece.
    pub depth_mm: f32,
    /// Gap between a pin and its socket, at the side and at the bottom.
    pub clearance_mm: f32,
    /// Least material left round a socket.
    pub wall_mm: f32,
    /// Most pins on one join.
    pub per_join: usize,
}

impl Default for PinParams {
    fn default() -> Self {
        Self { diameter_mm: 5.0, depth_mm: 4.0, clearance_mm: 0.15, wall_mm: 1.2, per_join: 2 }
    }
}

/// One placed pin.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pin {
    /// On the join, where the pin leaves its piece.
    site: Vec3,
    /// Unit, from the pin's piece into the socket's.
    axis: Vec3,
    /// Colours of the pin's piece and of the socket's.
    pub pin_lab: u8,
    pub socket_lab: u8,
    r: f32,
    depth: f32,
    clearance: f32,
}

/// Places tried on one join before giving up on it.
const MAX_TRIES: usize = 400;

/// Bevel at the tip of a pin, so it finds its socket.
const BEVEL_MM: f32 = 0.4;

impl Pin {
    /// Height above the join and distance from the axis.
    fn coords(&self, x: Vec3) -> (f32, f32) {
        let d = geom::sub(x, self.site);
        let s = geom::dot(d, self.axis);
        (s, geom::len(geom::sub(d, geom::scale(self.axis, s))))
    }

    /// Below zero inside the pin piece: under the join or in the pin.
    fn pin_side(&self, x: Vec3) -> f32 {
        let (s, rad) = self.coords(x);
        let bevel = BEVEL_MM.min(self.r * 0.5);
        let pin = (rad - self.r)
            .max(s - self.depth)
            .max((rad + s - (self.r + self.depth - bevel)) * std::f32::consts::FRAC_1_SQRT_2);
        s.min(pin)
    }

    /// Below zero inside the socket piece: over the join and out of the socket.
    fn socket_side(&self, x: Vec3) -> f32 {
        let (s, rad) = self.coords(x);
        let socket = (rad - (self.r + self.clearance)).max(s - (self.depth + self.clearance));
        (-s).max(-socket)
    }

    fn side(&self, lab: u8, x: Vec3) -> Option<f32> {
        if lab == self.pin_lab {
            Some(self.pin_side(x))
        } else if lab == self.socket_lab {
            Some(self.socket_side(x))
        } else {
            None
        }
    }

    pub fn axis(&self) -> Vec3 {
        self.axis
    }

    /// A point in the pin's piece, under the pin.
    pub fn base(&self, h: f32) -> Vec3 {
        geom::sub(self.site, geom::scale(self.axis, 1.5 * h))
    }

    /// A point in the socket's piece, past the end of the socket.
    pub fn tip(&self, h: f32) -> Vec3 {
        geom::add(self.site, geom::scale(self.axis, self.depth + self.clearance + 1.5 * h))
    }

    /// Radius of the socket with its wall.
    fn reach(&self, wall: f32) -> f32 {
        self.r + self.clearance + wall
    }
}

/// The pins and, for every voxel near one, which.
pub(super) struct Pins {
    pub pins: Vec<Pin>,
    zone: Vec<u32>,
    /// Joins wide enough for a pin where none fitted.
    pub missed: usize,
}

impl Pins {
    /// Where the surface of the region coloured `lab` crosses the edge from
    /// voxel `a` (in it) to `b`, when a pin decides it; and whether the region
    /// across has a surface of its own there (a pin in its socket) instead of
    /// sharing this one.
    pub fn crossing(&self, grid: &Grid, lab: &[u8], a: usize, b: usize) -> Option<(Vec3, bool)> {
        let z = if self.zone[a] != NONE { self.zone[a] } else { self.zone[b] };
        let pin = self.pins.get(z as usize)?;
        let (pa, pb) = (grid.center(a), grid.center(b));
        // The surfaces are cylinders, not planes: bisect for the crossing.
        let at = |la: u8, pa: Vec3, pb: Vec3| -> Option<Vec3> {
            let f = |t: f32| pin.side(la, geom::add(pa, geom::scale(geom::sub(pb, pa), t)));
            if !(f(0.0)? < 0.0 && f(1.0)? >= 0.0) {
                return None;
            }
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..16 {
                let m = 0.5 * (lo + hi);
                if f(m)? < 0.0 {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            Some(geom::add(pa, geom::scale(geom::sub(pb, pa), (0.5 * (lo + hi)).clamp(0.02, 0.98))))
        };
        let mine = at(lab[a], pa, pb)?;
        let own = match lab[b] {
            GAP => true,
            lb => at(lb, pb, pa).is_none_or(|theirs| geom::len(geom::sub(theirs, mine)) > 1e-4),
        };
        Some((mine, own))
    }
}

/// Places up to `per_join` pins on every join between two regions, the pin
/// on the smaller one, and carves them into `lab`. `comp`, `labels` and
/// `sizes` describe the regions before; they are stale afterwards.
pub(super) fn place(
    grid: &Grid,
    lab: &mut [u8],
    comp: &[u32],
    labels: &[u8],
    sizes: &[u32],
    params: &PinParams,
) -> Pins {
    let h = grid.h;
    let off6 = grid.offsets(&N6);
    // Layers in from the outside: where to look first.
    let mut depth = vec![u16::MAX; lab.len()];
    let mut front: Vec<usize> =
        (0..lab.len()).filter(|&v| comp[v] != NONE && off6.iter().any(|&o| comp[step(v, o)] == NONE)).collect();
    for &v in &front {
        depth[v] = 0;
    }
    let mut layer = 0u16;
    while !front.is_empty() {
        layer += 1;
        let mut next = vec![];
        for &v in &front {
            for &o in &off6 {
                let n = step(v, o);
                if comp[n] != NONE && depth[n] == u16::MAX {
                    depth[n] = layer;
                    next.push(n);
                }
            }
        }
        front = next;
    }

    // Faces between regions, from the side of the smaller one (the pin's).
    let mut joins: HashMap<(u32, u32), Vec<(usize, usize)>> = HashMap::new();
    for v in 0..lab.len() {
        let c = comp[v];
        if c == NONE {
            continue;
        }
        for &o in &off6 {
            let n = step(v, o);
            let d = comp[n];
            if d != NONE && d != c && (sizes[c as usize], c) < (sizes[d as usize], d) {
                joins.entry((c, d)).or_default().push((v, n));
            }
        }
    }
    let r = params.diameter_mm * 0.5;
    let probe = Pin {
        site: [0.0; 3],
        axis: [0.0, 0.0, 1.0],
        pin_lab: 0,
        socket_lab: 0,
        r,
        depth: params.depth_mm,
        clearance: params.clearance_mm,
    };
    let reach = probe.reach(params.wall_mm);
    let footprint = std::f32::consts::PI * reach * reach;
    let mut joins: Vec<_> = joins.into_iter().filter(|(_, f)| f.len() as f32 * h * h >= 1.5 * footprint).collect();
    // Biggest joins first, so they get their pins before smaller ones crowd them.
    joins.sort_by_key(|(k, f)| (std::cmp::Reverse(f.len()), *k));

    let cell = |p: Vec3| -> Option<usize> {
        let c: [isize; 3] = std::array::from_fn(|a| ((p[a] - grid.origin[a]) / h).floor() as isize);
        (0..3).all(|a| c[a] >= 0 && (c[a] as usize) < grid.n[a]).then(|| grid.index(c.map(|x| x as usize)))
    };
    let mut pins: Vec<Pin> = vec![];
    let mut missed = 0;
    for ((small, big), faces) in joins {
        // The pin on the smaller piece; failing that, on the bigger one.
        let mut placed = 0;
        for (pc, sc) in [(small, big), (big, small)] {
            if placed > 0 {
                break;
            }
            let faces: Vec<(usize, usize)> =
                if pc == small { faces.clone() } else { faces.iter().map(|&(v, n)| (n, v)).collect() };
            let face_of: HashMap<usize, Vec<usize>> = faces.iter().fold(HashMap::new(), |mut m, &(v, n)| {
                m.entry(v).or_default().push(n);
                m
            });
            let mut order: Vec<usize> = face_of.keys().copied().collect();
            order.sort_by_key(|&v| (std::cmp::Reverse(depth[v].min(depth[face_of[&v][0]])), v));
            let want = ((faces.len() as f32 * h * h / (6.0 * footprint)) as usize).clamp(1, params.per_join.max(1));
            // Pins on one join spread over it.
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &v in &order {
                let p = grid.center(v);
                lo = std::array::from_fn(|a| lo[a].min(p[a]));
                hi = std::array::from_fn(|a| hi[a].max(p[a]));
            }
            let apart = (2.0 * reach).max(0.35 * geom::len(geom::sub(hi, lo)));
            // One try per patch of the join, deepest first.
            let patch = (reach * 0.5).max(h);
            let mut tried = std::collections::HashSet::new();
            let mut tries = 0;
            let rings = 12;
            for &v in &order {
                if placed == want || tries == MAX_TRIES {
                    break;
                }
                // Too shallow for the socket's wall: the rest are shallower still.
                if (depth[v] as f32 + 1.0) * h < reach * 0.5 {
                    break;
                }
                if !tried.insert(grid.center(v).map(|x| (x / patch).floor() as i32)) {
                    continue;
                }
                tries += 1;
                // The join's plane here: mean of the faces nearby and their normal.
                let centre = grid.center(v);
                let k = (reach / h).ceil() as isize;
                let (mut at, mut normal, mut n) = ([0.0f32; 3], [0.0f32; 3], 0);
                let c = grid.coords(v);
                for dz in -k..=k {
                    for dy in -k..=k {
                        for dx in -k..=k {
                            let q = [c[0] as isize + dx, c[1] as isize + dy, c[2] as isize + dz];
                            if (0..3).any(|a| q[a] < 0 || q[a] as usize >= grid.n[a]) {
                                continue;
                            }
                            let u = grid.index(q.map(|x| x as usize));
                            let Some(ns) = face_of.get(&u) else { continue };
                            if geom::len(geom::sub(grid.center(u), centre)) > reach {
                                continue;
                            }
                            for &w in ns {
                                let (pu, pw) = (grid.center(u), grid.center(w));
                                at = geom::add(at, geom::scale(geom::add(pu, pw), 0.5));
                                normal = geom::add(normal, geom::sub(pw, pu));
                                n += 1;
                            }
                        }
                    }
                }
                if n == 0 || geom::len(normal) < 1e-6 {
                    continue;
                }
                let pin = Pin {
                    site: geom::scale(at, 1.0 / n as f32),
                    axis: geom::normalize(normal),
                    pin_lab: labels[pc as usize],
                    socket_lab: labels[sc as usize],
                    ..probe
                };
                let near = |o: &Pin| {
                    let d = geom::len(geom::sub(o.site, pin.site));
                    d < o.reach(params.wall_mm) + reach
                        || (o.pin_lab, o.socket_lab) == (pin.pin_lab, pin.socket_lab) && d < apart
                };
                if pins.iter().any(near) {
                    continue;
                }
                // The socket and its wall all in the socket's piece, and the pin's
                // piece under the whole mouth.
                let (u, w) = basis(pin.axis);
                let region_at = |p: Vec3| cell(p).map_or(NONE, |i| comp[i]);
                let point = |s: f32, rad: f32, k: usize| {
                    let a = k as f32 / rings as f32 * std::f32::consts::TAU;
                    geom::add(
                        pin.site,
                        geom::add(
                            geom::scale(pin.axis, s),
                            geom::scale(geom::add(geom::scale(u, a.cos()), geom::scale(w, a.sin())), rad),
                        ),
                    )
                };
                let top = pin.depth + pin.clearance + params.wall_mm;
                let mut fits = true;
                // The join is a voxel or so rough: the socket proper starts past that.
                let mut s = 2.0 * h;
                'socket: while s <= top {
                    for rad in [0.0, r, reach] {
                        for k in 0..rings {
                            if region_at(point(s, rad, k)) != sc {
                                fits = false;
                                break 'socket;
                            }
                        }
                    }
                    s += h * 0.5;
                }
                for rad in [0.0, r, r + pin.clearance + h] {
                    for k in 0..rings {
                        fits &= region_at(point(-1.5 * h, rad, k)) == pc;
                    }
                }
                if fits {
                    pins.push(pin);
                    placed += 1;
                }
            }
        }
        if placed == 0 {
            missed += 1;
        }
    }

    // Carve: near each pin, voxels take the side the pin puts them on.
    let mut zone = vec![NONE; lab.len()];
    for (i, pin) in pins.iter().enumerate() {
        let edge = pin.r + pin.clearance + 2.0 * h;
        let (lo_s, hi_s) = (-3.0 * h, pin.depth + pin.clearance + 2.0 * h);
        let ext = edge.max(hi_s);
        let lo = cell(geom::sub(pin.site, [ext; 3]));
        let hi = cell(geom::add(pin.site, [ext; 3]));
        let (Some(lo), Some(hi)) = (lo, hi) else { continue };
        let (lo, hi) = (grid.coords(lo), grid.coords(hi));
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let v = grid.index([x, y, z]);
                    if lab[v] != pin.pin_lab && lab[v] != pin.socket_lab && lab[v] != GAP {
                        continue;
                    }
                    let p = grid.center(v);
                    let (s, rad) = pin.coords(p);
                    if s < lo_s || s > hi_s || rad > edge {
                        continue;
                    }
                    zone[v] = i as u32;
                    if rad > pin.r + pin.clearance + h {
                        continue;
                    }
                    lab[v] = if pin.pin_side(p) < 0.0 {
                        pin.pin_lab
                    } else if pin.socket_side(p) < 0.0 {
                        pin.socket_lab
                    } else {
                        GAP
                    };
                }
            }
        }
    }
    Pins { pins, zone, missed }
}

/// Two unit vectors square to `n` and to each other.
fn basis(n: Vec3) -> (Vec3, Vec3) {
    let t = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = geom::normalize(geom::cross(n, t));
    (u, geom::cross(n, u))
}
