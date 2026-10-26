//! Finding the pieces a model is made of. Disconnected geometry is found
//! exactly: triangles that share a vertex, after welding corners closer than an
//! explicit tolerance, belong to one piece. A continuous mesh is never split on
//! a guess; it only gets plane proposals, each with the signal behind it.

use std::collections::HashMap;

use crate::geom::{self, Vec3};
use crate::mesh::MeshPart;
use crate::scene::Part;

/// Union-find over vertex ids.
struct Joins(Vec<u32>);

impl Joins {
    fn new(n: usize) -> Self {
        Self((0..n as u32).collect())
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            self.0[x as usize] = self.0[self.0[x as usize] as usize];
            x = self.0[x as usize];
        }
        x
    }

    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b as usize] = a;
        }
    }
}

/// Each vertex's representative once corners within `weld` of each other are one.
/// Seams duplicate positions; without this every seam would split a piece.
pub fn weld_ids(positions: &[Vec3], weld: f32) -> Vec<u32> {
    let mut joins = Joins::new(positions.len());
    if weld > 0.0 && weld.is_finite() {
        let cell = |p: Vec3| p.map(|v| (v / weld).floor() as i64);
        let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for (i, &p) in positions.iter().enumerate() {
            grid.entry(cell(p)).or_default().push(i as u32);
        }
        for (i, &p) in positions.iter().enumerate() {
            let c = cell(p);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(near) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else { continue };
                        for &j in near {
                            if (j as usize) > i && geom::len(geom::sub(positions[j as usize], p)) <= weld {
                                joins.join(i as u32, j);
                            }
                        }
                    }
                }
            }
        }
    } else {
        // Exactly equal positions are always the same corner.
        let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
        for (i, p) in positions.iter().enumerate() {
            let key = p.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
            let first = *seen.entry(key).or_insert(i as u32);
            joins.join(first, i as u32);
        }
    }
    (0..positions.len() as u32).map(|i| joins.find(i)).collect()
}

/// Triangles of each disconnected piece, largest first. Triangles meet when
/// they share a corner after welding within `weld` (source units).
pub fn pieces(positions: &[Vec3], tris: &[[u32; 3]], weld: f32) -> Vec<Vec<u32>> {
    let ids = weld_ids(positions, weld);
    let mut joins = Joins::new(positions.len());
    for t in tris {
        for k in 1..3 {
            joins.join(ids[t[0] as usize], ids[t[k] as usize]);
        }
    }
    let mut by_root: HashMap<u32, usize> = HashMap::new();
    let mut out: Vec<Vec<u32>> = vec![];
    for (ti, t) in tris.iter().enumerate() {
        let root = joins.find(ids[t[0] as usize]);
        let n = out.len();
        let at = *by_root.entry(root).or_insert(n);
        if at == n {
            out.push(vec![]);
        }
        out[at].push(ti as u32);
    }
    out.sort_by_key(|p| std::cmp::Reverse(p.len()));
    out
}

/// Every welded edge used once in each direction: closed and consistently wound.
pub fn closed(positions: &[Vec3], tris: &[[u32; 3]], subset: &[u32], weld: f32) -> bool {
    let ids = weld_ids(positions, weld);
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for &ti in subset {
        let t = tris[ti as usize].map(|v| ids[v as usize]);
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        for k in 0..3 {
            *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    !edges.is_empty() && edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
}

/// The part made of `tris` of `part`: its own vertices and face corners, the same materials.
pub fn sub_part(part: &Part, tris: &[u32]) -> Part {
    let mut vertex = vec![u32::MAX; part.positions.len()];
    let used = part.tri_loops.iter().flatten().map(|&l| l as usize + 1).max().unwrap_or(0);
    let stored = part.uv_layers.iter().map(Vec::len).chain(part.color_layers.iter().map(Vec::len)).max().unwrap_or(0);
    let mut corner = vec![u32::MAX; used.max(stored)];
    let mut out = Part {
        name: part.name.clone(),
        sources: part.sources.clone(),
        uv_layers: vec![vec![]; part.uv_layers.len()],
        color_layers: vec![vec![]; part.color_layers.len()],
        object: part.object,
        plate: part.plate,
        ..Default::default()
    };
    let mut loops = 0u32;
    for &ti in tris {
        let ti = ti as usize;
        out.tris.push(part.tris[ti].map(|v| {
            if vertex[v as usize] == u32::MAX {
                vertex[v as usize] = out.positions.len() as u32;
                out.positions.push(part.positions[v as usize]);
            }
            vertex[v as usize]
        }));
        out.tri_loops.push(part.tri_loops[ti].map(|l| {
            let l = l as usize;
            if corner[l] == u32::MAX {
                corner[l] = loops;
                loops += 1;
                for (to, from) in out.uv_layers.iter_mut().zip(&part.uv_layers) {
                    to.push(from.get(l).copied().unwrap_or_default());
                }
                for (to, from) in out.color_layers.iter_mut().zip(&part.color_layers) {
                    to.push(from.get(l).copied().unwrap_or_default());
                }
            }
            corner[l]
        }));
        out.tri_source.push(part.tri_source.get(ti).copied().unwrap_or(0));
    }
    out
}

/// What a proposed cut is based on.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// The section narrows to `area` between wider sections on both sides (all in source units²).
    Neck { area: f32, before: f32, after: f32 },
    /// The colour (filament slot) that covers most of the section changes here.
    Colour { from: u8, to: u8 },
}

/// A plane across one axis, in the mesh's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub part: usize,
    pub axis: usize,
    pub at: f32,
    pub signal: Signal,
}

const LEVELS: usize = 64;

/// Section area and dominant label of each level across `axis`, levels spread
/// evenly inside the bounds. Each triangle only visits the levels it spans.
fn profile(part: &MeshPart, labels: &[u8], axis: usize, lo: f32, hi: f32) -> (Vec<f32>, Vec<HashMap<u8, f32>>) {
    let step = (hi - lo) / (LEVELS + 1) as f32;
    let mut area = vec![0.0f32; LEVELS];
    let mut colour: Vec<HashMap<u8, f32>> = vec![HashMap::new(); LEVELS];
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    for (ti, t) in part.tris.iter().enumerate() {
        let c = t.map(|i| part.positions[i as usize]);
        let (tlo, thi) = c.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[axis]), b.max(p[axis])));
        let first = (((tlo - lo) / step).ceil() as i64 - 1).max(0) as usize;
        let last = ((((thi - lo) / step).floor() as i64) - 1).min(LEVELS as i64 - 1);
        if last < first as i64 {
            continue;
        }
        for (level, (area, colour)) in
            area.iter_mut().zip(colour.iter_mut()).enumerate().take(last as usize + 1).skip(first)
        {
            let at = lo + step * (level + 1) as f32;
            let d = c.map(|p| p[axis] - at);
            // Round the triangle, where it leaves the low side and where it comes back.
            let (mut out, mut back) = (None, None);
            for k in 0..3 {
                let (a, b) = (k, (k + 1) % 3);
                if (d[a] < 0.0) != (d[b] < 0.0) {
                    let f = d[a] / (d[a] - d[b]);
                    let p: Vec3 = std::array::from_fn(|i| c[a][i] + (c[b][i] - c[a][i]) * f);
                    if d[a] < 0.0 {
                        out = Some(p);
                    } else {
                        back = Some(p);
                    }
                }
            }
            let (Some(p), Some(q)) = (out, back) else { continue };
            // A closed surface's section loops add up to the section's signed area.
            *area += 0.5 * (p[u] * q[v] - q[u] * p[v]);
            let length = ((q[u] - p[u]).powi(2) + (q[v] - p[v]).powi(2)).sqrt();
            *colour.entry(labels.get(ti).copied().unwrap_or(0)).or_insert(0.0) += length;
        }
    }
    (area.into_iter().map(f32::abs).collect(), colour)
}

/// Planes worth reviewing across X, Y and Z: narrowings with more of the model
/// on both sides and where the dominant colour changes. `labels` is the slot of
/// each triangle. Necks are only read on closed parts, where areas mean something.
pub fn proposals(parts: &[MeshPart], labels: &[Vec<u8>], selected: &[usize], is_closed: &[bool]) -> Vec<Proposal> {
    let mut out = vec![];
    for (pi, part) in parts.iter().enumerate() {
        if part.tris.is_empty() || (!selected.is_empty() && !selected.contains(&pi)) {
            continue;
        }
        let (lo, hi) = part.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        });
        let empty = vec![];
        let labels = labels.get(pi).unwrap_or(&empty);
        for axis in 0..3 {
            if hi[axis] - lo[axis] <= 1e-6 {
                continue;
            }
            let step = (hi[axis] - lo[axis]) / (LEVELS + 1) as f32;
            let at = |level: usize| lo[axis] + step * (level + 1) as f32;
            let (area, colour) = profile(part, labels, axis, lo[axis], hi[axis]);
            if is_closed.get(pi).copied().unwrap_or(false) {
                // Some model on each side: the widest section at least 6 levels away.
                let reach = 6;
                let narrow: Vec<Option<(f32, f32)>> = (0..LEVELS)
                    .map(|i| {
                        if i < reach || i >= LEVELS - reach || area[i] <= 0.0 {
                            return None;
                        }
                        let before = area[..=i - reach].iter().copied().fold(0.0, f32::max);
                        let after = area[i + reach..].iter().copied().fold(0.0, f32::max);
                        (area[i] <= 0.5 * before.min(after)).then_some((before, after))
                    })
                    .collect();
                // A run of narrow levels is one neck: proposed at its narrowest, the middle of a flat bar.
                let mut found: Vec<(f32, usize, f32, f32)> = vec![];
                let mut i = 0;
                while i < LEVELS {
                    if narrow[i].is_none() {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < LEVELS && narrow[i].is_some() {
                        i += 1;
                    }
                    let least = area[start..i].iter().copied().fold(f32::MAX, f32::min);
                    let flat: Vec<usize> = (start..i).filter(|&k| area[k] <= least * 1.02).collect();
                    let k = flat[flat.len() / 2];
                    let (before, after) = narrow[k].unwrap();
                    found.push((area[k] / before.min(after), k, before, after));
                }
                found.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut kept: Vec<usize> = vec![];
                for (_, i, before, after) in found {
                    if kept.len() < 2 && kept.iter().all(|&k| k.abs_diff(i) > LEVELS / 8) {
                        kept.push(i);
                        out.push(Proposal {
                            part: pi,
                            axis,
                            at: at(i),
                            signal: Signal::Neck { area: area[i], before, after },
                        });
                    }
                }
            }
            let dominant: Vec<Option<u8>> = colour
                .iter()
                .map(|c| c.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))).map(|(&k, _)| k))
                .collect();
            // A change that holds for a few levels each side, not a speckle.
            let hold = 3;
            let mut last_change = None::<usize>;
            for i in hold..LEVELS - hold + 1 {
                let (Some(a), Some(b)) = (dominant[i - 1], dominant[i]) else { continue };
                if a == b {
                    continue;
                }
                let steady = dominant[i - hold..i].iter().all(|&d| d == Some(a))
                    && dominant[i..i + hold].iter().all(|&d| d == Some(b));
                if steady && last_change.is_none_or(|l| i - l > hold) {
                    last_change = Some(i);
                    out.push(Proposal {
                        part: pi,
                        axis,
                        at: (at(i - 1) + at(i)) / 2.0,
                        signal: Signal::Colour { from: a, to: b },
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box as its own vertices and triangles, faces outwards.
    pub(crate) fn boxed(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let c = |i: usize| {
            [
                if i & 1 > 0 { hi[0] } else { lo[0] },
                if i & 2 > 0 { hi[1] } else { lo[1] },
                if i & 4 > 0 { hi[2] } else { lo[2] },
            ]
        };
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        ((0..8).map(c).collect(), quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect())
    }

    fn join(a: (Vec<Vec3>, Vec<[u32; 3]>), b: (Vec<Vec3>, Vec<[u32; 3]>)) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let base = a.0.len() as u32;
        let mut p = a.0;
        p.extend(b.0);
        let mut t = a.1;
        t.extend(b.1.iter().map(|t| t.map(|v| v + base)));
        (p, t)
    }

    /// Every triangle with its own three corners, as importers leave unwelded meshes.
    fn unweld((p, t): (Vec<Vec3>, Vec<[u32; 3]>)) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let positions: Vec<Vec3> = t.iter().flat_map(|t| t.map(|v| p[v as usize])).collect();
        let tris = (0..t.len() as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
        (positions, tris)
    }

    #[test]
    fn disconnected_boxes_are_pieces_and_an_unwelded_box_is_one() {
        let (p, t) = join(boxed([0.0; 3], [1.0; 3]), boxed([3.0, 0.0, 0.0], [5.0, 1.0, 1.0]));
        let found = pieces(&p, &t, 1e-4);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.len() == 12 && closed(&p, &t, f, 1e-4)));

        // Duplicate corners at every face: one piece once welded, even with no tolerance.
        let (mut p, t) = unweld(boxed([0.0; 3], [1.0; 3]));
        assert_eq!(pieces(&p, &t, 1e-4).len(), 1);
        assert!(closed(&p, &t, &(0..12).collect::<Vec<_>>(), 1e-4));
        assert_eq!(pieces(&p, &t, 0.0).len(), 1);
        // Corners a hair apart only meet within the tolerance.
        for (i, v) in p.iter_mut().enumerate() {
            v[0] += i as f32 * 1e-6;
        }
        assert_eq!(pieces(&p, &t, 0.0).len(), 12);
        assert_eq!(pieces(&p, &t, 1e-3).len(), 1);

        // Boxes touching face to face share no vertex: still two pieces.
        let (p, t) = join(boxed([0.0; 3], [1.0; 3]), boxed([1.0, 0.25, 0.25], [2.0, 0.75, 0.75]));
        assert_eq!(pieces(&p, &t, 1e-4).len(), 2);
        // A tolerance as wide as the gap welds two nearby boxes into one.
        let (p, t) = join(boxed([0.0; 3], [1.0; 3]), boxed([1.05, 0.0, 0.0], [2.0, 1.0, 1.0]));
        assert_eq!(pieces(&p, &t, 0.01).len(), 2);
        assert_eq!(pieces(&p, &t, 0.06).len(), 1);
    }

    #[test]
    fn an_open_piece_is_reported_open() {
        let (p, mut t) = boxed([0.0; 3], [1.0; 3]);
        t.pop();
        assert!(!closed(&p, &t, &(0..11).collect::<Vec<_>>(), 1e-4));
    }

    fn mesh((positions, tris): (Vec<Vec3>, Vec<[u32; 3]>)) -> MeshPart {
        MeshPart { name: "m".into(), positions, tris, origin: None }
    }

    #[test]
    fn a_dumbbell_gets_a_neck_proposal_and_a_plain_box_none() {
        // Two wide boxes joined by a thin bar, as one welded closed surface is
        // hard to build by hand; a solid made of three stacked closed boxes has
        // the same section profile.
        let (p, t) = join(
            join(boxed([0.0; 3], [10.0, 10.0, 10.0]), boxed([4.0, 4.0, 10.0], [6.0, 6.0, 20.0])),
            boxed([0.0, 0.0, 20.0], [10.0, 10.0, 30.0]),
        );
        let found = proposals(&[mesh((p, t))], &[], &[], &[true]);
        let neck = found.iter().find(|p| p.axis == 2 && matches!(p.signal, Signal::Neck { .. })).expect("neck along Z");
        assert!(neck.at > 10.0 && neck.at < 20.0, "{neck:?}");
        let Signal::Neck { area, before, after } = neck.signal else { unreachable!() };
        assert!(
            (area - 4.0).abs() < 0.5 && (before - 100.0).abs() < 1.0 && (after - 100.0).abs() < 1.0,
            "{area} {before} {after}"
        );

        let plain = proposals(&[mesh(boxed([0.0; 3], [10.0; 3]))], &[], &[], &[true]);
        assert!(plain.is_empty(), "{plain:?}");
        // Open parts give no neck: their areas would be guesses.
        let (p, t) = join(
            join(boxed([0.0; 3], [10.0, 10.0, 10.0]), boxed([4.0, 4.0, 10.0], [6.0, 6.0, 20.0])),
            boxed([0.0, 0.0, 20.0], [10.0, 10.0, 30.0]),
        );
        assert!(proposals(&[mesh((p, t))], &[], &[], &[false]).is_empty());
    }

    #[test]
    fn a_colour_change_is_proposed_where_it_holds() {
        // A tall box, red below z = 12 and blue above, built from two boxes.
        let (p, t) = join(boxed([0.0; 3], [10.0, 10.0, 12.0]), boxed([0.0, 0.0, 12.0], [10.0, 10.0, 30.0]));
        let labels = vec![(0..t.len()).map(|i| u8::from(i >= 12)).collect::<Vec<u8>>()];
        let found = proposals(&[mesh((p, t))], &labels, &[], &[false]);
        let change: Vec<_> = found.iter().filter(|p| p.axis == 2).collect();
        assert_eq!(change.len(), 1, "{found:?}");
        assert!((change[0].at - 12.0).abs() < 0.5);
        assert_eq!(change[0].signal, Signal::Colour { from: 0, to: 1 });
    }

    #[test]
    fn a_sub_part_keeps_only_its_vertices_and_corners() {
        use crate::scene::ColorSource;
        let (positions, tris) = join(boxed([0.0; 3], [1.0; 3]), boxed([3.0, 0.0, 0.0], [5.0, 1.0, 1.0]));
        let n = tris.len() as u32;
        let part = Part {
            name: "two".into(),
            positions,
            tri_loops: (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect(),
            tri_source: (0..n).map(|i| (i >= 12) as u16).collect(),
            tris,
            sources: vec![ColorSource::Flat([1.0, 0.0, 0.0]), ColorSource::Flat([0.0, 0.0, 1.0])],
            uv_layers: vec![(0..n * 3).map(|i| [i as f32, 0.0]).collect()],
            ..Default::default()
        };
        let second = sub_part(&part, &(12..24).collect::<Vec<_>>());
        assert_eq!((second.positions.len(), second.tris.len()), (8, 12));
        assert!(second.tri_source.iter().all(|&s| s == 1));
        assert_eq!(second.uv_layers[0].len(), 36);
        assert_eq!(second.uv_layers[0][0], [36.0, 0.0]);
        assert!(second.positions.iter().all(|p| p[0] >= 3.0));
    }
}
