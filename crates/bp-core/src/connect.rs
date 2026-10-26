//! Connectors for putting a cut model back together: pins on one half with
//! sockets in the other, or loose dowels with sockets in both. Everything is
//! laid out on the cut plane in final millimetres; the clearance is the gap
//! left all round a connector in its socket, at the sides and at the bottom.

use crate::bvh::{Bvh, TriSoup};
use crate::geom::{self, Vec3};

/// Sides of a round connector: its outline is a regular polygon with its
/// corners on the nominal circle, the same for the pin and its socket.
pub const ROUND_SIDES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Pins stand out of one half (the low one, A, with `on_low`) into sockets in the other.
    Pin { on_low: bool },
    /// Loose dowels, printed as objects of their own, into sockets in both halves.
    Dowel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Round,
    Square,
}

/// The connectors of one cut, in final millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct Connectors {
    pub kind: Kind,
    pub shape: Shape,
    /// Diameter of a round connector, side of a square one.
    pub size_mm: f32,
    /// How far a connector goes into each half.
    pub depth_mm: f32,
    /// Gap between a connector and its socket, on each side and at the bottom.
    pub clearance_mm: f32,
    /// Turn of a square connector about the plane's normal, in degrees.
    pub angle_deg: f32,
    /// Centres on the cut plane, in preview coordinates.
    pub at: Vec<Vec3>,
}

/// Why a connector cannot be made where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    /// Not on the section of any part being cut.
    Outside,
    /// Too close to another connector to leave a wall between their sockets.
    Overlap,
    /// Too close to the section's edge or to the model's surface inside it.
    ThinWall,
    /// The socket would come out of the far side of the model.
    TooDeep,
}

impl Connectors {
    /// Sizes in range: 1–50 mm across, 1–100 mm deep, up to 2 mm of clearance.
    pub fn valid(&self) -> bool {
        let finite = [self.size_mm, self.depth_mm, self.clearance_mm, self.angle_deg].iter().all(|v| v.is_finite())
            && self.at.iter().flatten().all(|v| v.is_finite());
        finite
            && (1.0..=50.0).contains(&self.size_mm)
            && (1.0..=100.0).contains(&self.depth_mm)
            && (0.0..=2.0).contains(&self.clearance_mm)
    }

    /// Half the width of the outline, grown by `grow` on every side.
    fn half(&self, grow: f32) -> f32 {
        self.size_mm / 2.0 + grow
    }

    /// Radius of the circle round the outline grown by `grow`.
    pub fn reach(&self, grow: f32) -> f32 {
        match self.shape {
            Shape::Round => self.half(grow),
            Shape::Square => self.half(grow) * std::f32::consts::SQRT_2,
        }
    }

    /// The outline round `centre` on the plane spanned by unit `u` and `v`,
    /// grown by `grow` on every side; `unit` turns millimetres into those coordinates.
    pub fn outline(&self, centre: Vec3, u: Vec3, v: Vec3, grow: f32, unit: f32) -> Vec<Vec3> {
        let (n, r, turn) = match self.shape {
            Shape::Round => (ROUND_SIDES, self.half(grow), 0.0),
            Shape::Square => (
                4,
                self.half(grow) * std::f32::consts::SQRT_2,
                self.angle_deg.to_radians() + std::f32::consts::FRAC_PI_4,
            ),
        };
        (0..n)
            .map(|k| {
                let a = turn + k as f32 / n as f32 * std::f32::consts::TAU;
                geom::add(centre, geom::add(geom::scale(u, r * a.cos() * unit), geom::scale(v, r * a.sin() * unit)))
            })
            .collect()
    }

    /// Which halves get sockets (0 low, 1 high) and which, if any, gets pins.
    pub fn sides(&self) -> (Vec<usize>, Option<usize>) {
        match self.kind {
            Kind::Pin { on_low: true } => (vec![1], Some(0)),
            Kind::Pin { on_low: false } => (vec![0], Some(1)),
            Kind::Dowel => (vec![0, 1], None),
        }
    }
}

/// The thinnest wall worth printing round a socket: two lines of the nozzle, at least 0.8 mm.
pub fn min_wall(nozzle_mm: f32) -> f32 {
    (2.0 * nozzle_mm).max(0.8)
}

/// A part's section on the cut plane, in plane coordinates (millimetres), and
/// its surface in preview coordinates for distance and ray queries.
pub struct Section {
    pub loops: Vec<Vec<[f32; 2]>>,
    pub surface: (Bvh, TriSoup),
}

impl Section {
    fn inside(&self, p: [f32; 2]) -> bool {
        let mut odd = false;
        for ring in &self.loops {
            for k in 0..ring.len() {
                let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                    odd = !odd;
                }
            }
        }
        odd
    }

    /// Distance from `p` to the nearest edge of the section.
    fn edge_distance(&self, p: [f32; 2]) -> f32 {
        let mut best = f32::MAX;
        for ring in &self.loops {
            for k in 0..ring.len() {
                let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                let ab = [b[0] - a[0], b[1] - a[1]];
                let len2 = ab[0] * ab[0] + ab[1] * ab[1];
                let t = if len2 > 0.0 {
                    (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                best = best.min(((p[0] - a[0] - ab[0] * t).powi(2) + (p[1] - a[1] - ab[1] * t).powi(2)).sqrt());
            }
        }
        best
    }

    fn clearance(&self, q: Vec3) -> f32 {
        let (bvh, soup) = &self.surface;
        bvh.nearest(soup, q).map_or(f32::MAX, |(t, b)| geom::len(geom::sub(geom::at(&soup.corners[t as usize], b), q)))
    }
}

/// The cut plane in preview coordinates with its in-plane axes.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub point: Vec3,
    pub normal: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    /// Millimetres per preview unit.
    pub scale: f32,
}

impl Frame {
    /// Plane coordinates of `p`, in millimetres.
    pub fn flat(&self, p: Vec3) -> [f32; 2] {
        let d = geom::sub(p, self.point);
        [geom::dot(d, self.u) * self.scale, geom::dot(d, self.v) * self.scale]
    }

    /// The preview point at plane coordinates `q` (millimetres).
    pub fn lift(&self, q: [f32; 2]) -> Vec3 {
        geom::add(self.point, geom::add(geom::scale(self.u, q[0] / self.scale), geom::scale(self.v, q[1] / self.scale)))
    }

    /// Into the solid of side 0 (below the plane) or side 1.
    pub fn inward(&self, side: usize) -> Vec3 {
        geom::scale(self.normal, if side == 0 { -1.0 } else { 1.0 })
    }
}

/// For each connector, the section it sits on and what is wrong with it.
pub fn check(c: &Connectors, frame: &Frame, sections: &[Section], wall_mm: f32) -> Vec<(Option<usize>, Option<Issue>)> {
    let reach = c.reach(c.clearance_mm);
    let flats: Vec<[f32; 2]> = c.at.iter().map(|&p| frame.flat(p)).collect();
    let (sockets, _) = c.sides();
    let mut out: Vec<(Option<usize>, Option<Issue>)> = flats
        .iter()
        .map(|&q| {
            let Some(s) = sections.iter().position(|s| s.inside(q)) else { return (None, Some(Issue::Outside)) };
            let section = &sections[s];
            if section.edge_distance(q) < reach + wall_mm {
                return (Some(s), Some(Issue::ThinWall));
            }
            let centre = frame.lift(q);
            let deep = c.depth_mm + c.clearance_mm;
            for &side in &sockets {
                let dir = frame.inward(side);
                // Straight on from the socket's floor, the model has to go on for a wall's width.
                let start = geom::add(centre, geom::scale(dir, 1e-3 / frame.scale));
                let (bvh, soup) = &section.surface;
                let hit = bvh.raycast(soup, start, dir).map_or(f32::MAX, |h| h.t * frame.scale);
                if hit < deep + wall_mm {
                    return (Some(s), Some(Issue::TooDeep));
                }
                // Round the socket's wall and floor, the surface has to stay a wall's width away.
                let (u, v) = (frame.u, frame.v);
                for t in [wall_mm, (wall_mm + deep) / 2.0, deep] {
                    let level = geom::add(centre, geom::scale(dir, t.max(wall_mm) / frame.scale));
                    let ring = c.outline(level, u, v, c.clearance_mm, 1.0 / frame.scale);
                    let floor = geom::add(centre, geom::scale(dir, deep / frame.scale));
                    let thin =
                        ring.iter().chain([&floor]).any(|&q| section.clearance(q) * frame.scale < wall_mm * 0.999);
                    if thin {
                        return (Some(s), Some(Issue::ThinWall));
                    }
                }
            }
            (Some(s), None)
        })
        .collect();
    for i in 0..flats.len() {
        for j in i + 1..flats.len() {
            let d = ((flats[i][0] - flats[j][0]).powi(2) + (flats[i][1] - flats[j][1]).powi(2)).sqrt();
            if d < 2.0 * reach + wall_mm {
                for k in [i, j] {
                    if out[k].1.is_none() {
                        out[k].1 = Some(Issue::Overlap);
                    }
                }
            }
        }
    }
    out
}

/// Where connectors fit best: for each outline of each section, the point
/// farthest from its edges, and a second one along it when there is room. A
/// place only counts when a connector fits there with a wall round its socket.
pub fn suggest(c: &Connectors, sections: &[Section], wall_mm: f32) -> Vec<[f32; 2]> {
    let need = c.reach(c.clearance_mm) + wall_mm;
    let mut out: Vec<[f32; 2]> = vec![];
    for section in sections {
        let (lo, hi) = section.loops.iter().flatten().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
            ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
        });
        if lo[0] > hi[0] {
            continue;
        }
        const N: usize = 48;
        let step = [(hi[0] - lo[0]) / N as f32, (hi[1] - lo[1]) / N as f32];
        let mut cells: Vec<([f32; 2], f32)> = vec![];
        for i in 0..N {
            for j in 0..N {
                let q = [lo[0] + step[0] * (i as f32 + 0.5), lo[1] + step[1] * (j as f32 + 0.5)];
                if section.inside(q) {
                    let d = section.edge_distance(q);
                    if d >= need {
                        cells.push((q, d));
                    }
                }
            }
        }
        cells.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut mine: Vec<[f32; 2]> = vec![];
        for (q, d) in cells {
            // Two connectors well apart keep the halves from turning on each other.
            let far = mine
                .iter()
                .chain(&out)
                .all(|m| ((m[0] - q[0]).powi(2) + (m[1] - q[1]).powi(2)).sqrt() >= 2.0 * need + 4.0 * c.reach(0.0));
            if far && d >= need {
                mine.push(q);
                if mine.len() == 2 {
                    break;
                }
            }
        }
        out.extend(mine);
    }
    out
}

/// A loose dowel standing on one end, its base centred on the origin, in millimetres.
pub fn dowel(c: &Connectors) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let ring = c.outline([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0, 1.0);
    let n = ring.len() as u32;
    let length = 2.0 * c.depth_mm;
    let mut positions = ring.clone();
    positions.extend(ring.iter().map(|p| [p[0], p[1], length]));
    let mut tris = vec![];
    for k in 0..n {
        let j = (k + 1) % n;
        tris.push([k, j, n + j]);
        tris.push([k, n + j, n + k]);
    }
    for k in 1..n - 1 {
        tris.push([0, k + 1, k]);
        tris.push([n, n + k, n + k + 1]);
    }
    (positions, tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connectors(kind: Kind, at: Vec<Vec3>) -> Connectors {
        Connectors { kind, shape: Shape::Round, size_mm: 4.0, depth_mm: 5.0, clearance_mm: 0.2, angle_deg: 0.0, at }
    }

    /// The section and surface of a closed box from `lo` to `hi`, cut at z = `z`.
    fn boxed(lo: Vec3, hi: Vec3, z: f32) -> (Frame, Section) {
        let c = |i: usize| {
            [
                if i & 1 > 0 { hi[0] } else { lo[0] },
                if i & 2 > 0 { hi[1] } else { lo[1] },
                if i & 4 > 0 { hi[2] } else { lo[2] },
            ]
        };
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut soup = TriSoup::default();
        for q in quads {
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                soup.push(t.map(c), 0, 0);
            }
        }
        let frame =
            Frame { point: [0.0, 0.0, z], normal: [0.0, 0.0, 1.0], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], scale: 1.0 };
        let outline = vec![[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]];
        (frame, Section { loops: vec![outline], surface: (Bvh::build(&soup), soup) })
    }

    #[test]
    fn outlines_have_the_clearance_all_round() {
        let c = Connectors { shape: Shape::Square, angle_deg: 30.0, ..connectors(Kind::Dowel, vec![]) };
        let pin = c.outline([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0, 1.0);
        let socket = c.outline([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], c.clearance_mm, 1.0);
        let side = |r: &[Vec3]| geom::len(geom::sub(r[1], r[0]));
        assert!((side(&pin) - 4.0).abs() < 1e-4 && (side(&socket) - 4.4).abs() < 1e-4);
        let round = connectors(Kind::Dowel, vec![]);
        let ring = round.outline([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0, 1.0);
        assert_eq!(ring.len(), ROUND_SIDES);
        assert!(ring.iter().all(|p| (geom::len(*p) - 2.0).abs() < 1e-5));
        let (positions, tris) = dowel(&round);
        assert_eq!((positions.len(), tris.len()), (2 * ROUND_SIDES, 4 * ROUND_SIDES - 4));
        assert!(positions.iter().all(|p| p[2] == 0.0 || p[2] == 10.0));
    }

    #[test]
    fn connectors_are_checked_against_the_section_each_other_and_the_walls() {
        let (frame, section) = boxed([-20.0, -10.0, 0.0], [20.0, 10.0, 30.0], 15.0);
        let sections = [section];
        let c = connectors(
            Kind::Dowel,
            vec![[-10.0, 0.0, 15.0], [10.0, 0.0, 15.0], [40.0, 0.0, 15.0], [11.0, 1.0, 15.0], [19.0, 0.0, 15.0]],
        );
        let issues: Vec<_> = check(&c, &frame, &sections, 0.8).into_iter().map(|x| x.1).collect();
        assert_eq!(
            issues,
            [None, Some(Issue::Overlap), Some(Issue::Outside), Some(Issue::Overlap), Some(Issue::ThinWall)]
        );
        // 5 mm of socket and its wall do not fit in 5.5 mm of model below the plane.
        let (frame, section) = boxed([-20.0, -10.0, 0.0], [20.0, 10.0, 30.0], 5.5);
        let c = connectors(Kind::Pin { on_low: false }, vec![[0.0, 0.0, 5.5]]);
        assert_eq!(check(&c, &frame, &[section], 0.8)[0].1, Some(Issue::TooDeep));
        // The same pin standing on the low half only needs a socket above.
        let (frame, section) = boxed([-20.0, -10.0, 0.0], [20.0, 10.0, 30.0], 5.5);
        let c = connectors(Kind::Pin { on_low: true }, vec![[0.0, 0.0, 5.5]]);
        assert_eq!(check(&c, &frame, &[section], 0.8)[0], (Some(0), None));
    }

    #[test]
    fn suggestions_fit_with_their_walls_and_spread_along_long_sections() {
        let (frame, section) = boxed([-30.0, -6.0, 0.0], [30.0, 6.0, 30.0], 15.0);
        let sections = [section];
        let c = connectors(Kind::Dowel, vec![]);
        let found = suggest(&c, &sections, 0.8);
        assert_eq!(found.len(), 2, "{found:?}");
        let placed = Connectors { at: found.iter().map(|&q| frame.lift(q)).collect(), ..c.clone() };
        assert!(check(&placed, &frame, &sections, 0.8).iter().all(|x| x.1.is_none()));
        // A section too thin for a socket and its walls gets none.
        let (_, thin) = boxed([-30.0, -2.0, 0.0], [30.0, 2.0, 30.0], 15.0);
        assert!(suggest(&c, &[thin], 0.8).is_empty());
    }
}
