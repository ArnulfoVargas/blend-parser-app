//! Splits a painted mesh into one closed solid per filament colour, for
//! printing colours as separate pieces or for slicers without paint support.
//!
//! The colours are read from the painting itself, leaf by leaf, not from each
//! triangle's main colour, so the pieces follow the painted borders. Leaves of
//! neighbouring triangles split to different depths meet with T-junctions;
//! they are stitched first, so every piece is closed.
//!
//! Each colour becomes a shell: its patch of surface, the same patch pushed
//! inward by `thickness`, and walls joining the two along the patch border.
//! Optionally one colour (the main one) becomes the body the others sit in:
//! its own surface, plus the other colours' patches sunk by the same
//! `thickness`, so each shell drops into its pocket and together they make
//! up the model without overlapping.

use std::collections::{HashMap, HashSet};

use crate::geom::{self, BaryTri, Vec3, ROOT};
use crate::mesh::{Mesh, MeshPart};
use crate::paint::Node;

pub struct ColorPiece {
    pub slot: u8,
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// True when this is the main body, with pockets for the other colours.
    pub solid: bool,
}

/// The surface as painted: one triangle per painted leaf, T-junctions stitched.
pub struct Painted {
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub labels: Vec<u8>,
}

/// Barycentric coordinates as integers summing to `ONE`: leaf corners are
/// halves of halves of the root, so they are exact at this scale.
const ONE: i64 = 1 << 24;

/// A point of the painted surface, the same whichever triangle names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Corner(u32),
    /// On the edge between two vertices (lower id first), with the lower one's weight.
    Edge(u32, u32, i64),
    /// Inside a triangle: its first two barycentrics.
    Inside(u32, i64, i64),
}

fn int_bary(b: [f32; 3]) -> [i64; 3] {
    let i = (b[0] as f64 * ONE as f64).round() as i64;
    let j = (b[1] as f64 * ONE as f64).round() as i64;
    [i, j, ONE - i - j]
}

fn key(t: [u32; 3], tri: u32, b: [i64; 3]) -> Key {
    let zeros = b.iter().filter(|&&x| x == 0).count();
    if zeros >= 2 {
        let k = (0..3).find(|&k| b[k] != 0).unwrap_or(0);
        return Key::Corner(t[k]);
    }
    if zeros == 1 {
        let z = (0..3).find(|&k| b[k] == 0).unwrap_or(0);
        let (p, q) = ((z + 1) % 3, (z + 2) % 3);
        return match t[p].cmp(&t[q]) {
            std::cmp::Ordering::Less => Key::Edge(t[p], t[q], b[p]),
            std::cmp::Ordering::Greater => Key::Edge(t[q], t[p], b[q]),
            // A degenerate triangle: its "edge" is a point.
            std::cmp::Ordering::Equal => Key::Corner(t[p]),
        };
    }
    Key::Inside(tri, b[0], b[1])
}

/// Vertex ids with exact duplicates joined, so seams in the file do not open the pieces.
fn welded(part: &MeshPart) -> Vec<[u32; 3]> {
    let mut first: HashMap<[u32; 3], u32> = HashMap::with_capacity(part.positions.len());
    let ids: Vec<u32> =
        part.positions.iter().enumerate().map(|(i, p)| *first.entry(p.map(f32::to_bits)).or_insert(i as u32)).collect();
    part.tris.iter().map(|t| t.map(|v| ids[v as usize])).collect()
}

/// The painted leaves of one part as a closed, consistently wound surface
/// when the part is: each leaf is a triangle, or, where finer leaves next to
/// it add points on its edges, a fan round its centre through those points.
pub fn painted(part: &MeshPart, nodes: &[Node]) -> Painted {
    let tris = welded(part);
    let mut leaves: Vec<(u32, [[i64; 3]; 3], u8)> = vec![];
    for (ti, node) in nodes.iter().enumerate().take(tris.len()) {
        node.for_each_leaf(&ROOT, &mut |t: &BaryTri, label| leaves.push((ti as u32, t.map(int_bary), label)));
    }
    let mut index: HashMap<Key, u32> = HashMap::with_capacity(leaves.len() * 2);
    let mut out = Painted { positions: vec![], tris: vec![], labels: vec![] };
    let mut vertex = |k: Key, ti: u32, b: [i64; 3], out: &mut Painted| -> u32 {
        *index.entry(k).or_insert_with(|| {
            let c = part.corners(ti as usize);
            let f = b.map(|x| (x as f64 / ONE as f64) as f32);
            out.positions.push(geom::at(&c, f));
            out.positions.len() as u32 - 1
        })
    };
    // Every leaf corner first, so the edges below can find the points on them.
    let mut present: HashSet<Key> = HashSet::with_capacity(leaves.len() * 2);
    for &(ti, corners, _) in &leaves {
        for b in corners {
            present.insert(key(tris[ti as usize], ti, b));
        }
    }
    // Points of finer leaves on the edge from `p` to `q`, in order: they are
    // always midpoints of midpoints, so it only needs to look at those.
    fn between(t: [u32; 3], ti: u32, p: [i64; 3], q: [i64; 3], present: &HashSet<Key>, out: &mut Vec<[i64; 3]>) {
        if (0..3).any(|k| (p[k] + q[k]) % 2 != 0) {
            return;
        }
        let m = [(p[0] + q[0]) / 2, (p[1] + q[1]) / 2, (p[2] + q[2]) / 2];
        if !present.contains(&key(t, ti, m)) {
            return;
        }
        between(t, ti, p, m, present, out);
        out.push(m);
        between(t, ti, m, q, present, out);
    }
    for (ti, corners, label) in leaves {
        let t = tris[ti as usize];
        let mut ring: Vec<[i64; 3]> = Vec::with_capacity(6);
        for k in 0..3 {
            ring.push(corners[k]);
            between(t, ti, corners[k], corners[(k + 1) % 3], &present, &mut ring);
        }
        let ids: Vec<u32> = ring.iter().map(|&b| vertex(key(t, ti, b), ti, b, &mut out)).collect();
        if ids.len() == 3 {
            if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                out.tris.push([ids[0], ids[1], ids[2]]);
                out.labels.push(label);
            }
            continue;
        }
        // The leaf is convex: a fan from its centre covers it without slivers.
        let c = part.corners(ti as usize);
        let mid = [0, 1, 2].map(|k| corners.iter().map(|b| b[k]).sum::<i64>() as f64 / 3.0 / ONE as f64);
        out.positions.push(geom::at(&c, mid.map(|x| x as f32)));
        let centre = out.positions.len() as u32 - 1;
        for k in 0..ids.len() {
            let (a, b) = (ids[k], ids[(k + 1) % ids.len()]);
            if a != b {
                out.tris.push([centre, a, b]);
                out.labels.push(label);
            }
        }
    }
    out
}

/// Inward offset of each vertex: along its area-weighted normal, stretched
/// so the flat faces round it stay `thickness` from their offset (at most
/// twice, at sharp corners).
fn inward(positions: &[Vec3], tris: &[[u32; 3]], thickness: f32) -> Vec<Vec3> {
    let mut n = vec![[0.0f32; 3]; positions.len()];
    let faces: Vec<Vec3> = tris
        .iter()
        .map(|t| {
            let c = t.map(|v| positions[v as usize]);
            geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]))
        })
        .collect();
    for (t, f) in tris.iter().zip(&faces) {
        for &v in t {
            n[v as usize] = geom::add(n[v as usize], *f);
        }
    }
    let n: Vec<Vec3> = n.into_iter().map(geom::normalize).collect();
    let mut cos = vec![1.0f32; positions.len()];
    for (t, f) in tris.iter().zip(&faces) {
        let f = geom::normalize(*f);
        for &v in t {
            cos[v as usize] = cos[v as usize].min(geom::dot(n[v as usize], f));
        }
    }
    positions
        .iter()
        .zip(&n)
        .zip(&cos)
        .map(|((&p, &nv), &c)| geom::sub(p, geom::scale(nv, thickness / c.max(0.5))))
        .collect()
}

fn find(up: &mut [u32], mut x: u32) -> u32 {
    while up[x as usize] != x {
        up[x as usize] = up[up[x as usize] as usize];
        x = up[x as usize];
    }
    x
}

/// Builds one piece. Its vertices are triangle corners grouped round each
/// vertex by the triangles of the same kind they join through, so where a
/// colour only touches itself at a point each side gets its own copy and
/// the piece stays edge-manifold.
struct Builder<'a> {
    painted: &'a Painted,
    inner: &'a [Vec3],
    up: Vec<u32>,
    ids: HashMap<(u32, bool), u32>,
    piece: ColorPiece,
}

impl<'a> Builder<'a> {
    /// `kind(t)` is how triangle `t` is drawn in this piece (None: not at all).
    fn new(
        painted: &'a Painted,
        inner: &'a [Vec3],
        across: &HashMap<(u32, u32), u32>,
        kind: impl Fn(usize) -> Option<bool>,
        slot: u8,
        solid: bool,
    ) -> Self {
        let mut up: Vec<u32> = (0..painted.tris.len() as u32 * 3).collect();
        for (ti, t) in painted.tris.iter().enumerate() {
            let Some(mine) = kind(ti) else { continue };
            for k in 0..3 {
                let (a, c) = (t[k], t[(k + 1) % 3]);
                let Some(&u) = across.get(&(c, a)) else { continue };
                if kind(u as usize) != Some(mine) {
                    continue;
                }
                let tu = painted.tris[u as usize];
                for (v, here) in [(a, k), (c, (k + 1) % 3)] {
                    if let Some(there) = (0..3).find(|&j| tu[j] == v) {
                        let (x, y) = (find(&mut up, ti as u32 * 3 + here as u32), find(&mut up, u * 3 + there as u32));
                        up[x as usize] = y;
                    }
                }
            }
        }
        Self {
            painted,
            inner,
            up,
            ids: HashMap::new(),
            piece: ColorPiece { slot, positions: vec![], tris: vec![], solid },
        }
    }

    /// Corner `k` of triangle `t`, on the surface or sunk.
    fn id(&mut self, t: u32, k: usize, sunk: bool) -> u32 {
        let v = self.painted.tris[t as usize][k] as usize;
        // The body's own surface is never split: where it pinches, the pockets
        // on either side are, and the walls round each pocket meet on it.
        let group = if self.piece.solid && !sunk { v as u32 } else { find(&mut self.up, t * 3 + k as u32) };
        let (outer, inner, positions) = (&self.painted.positions, self.inner, &mut self.piece.positions);
        *self.ids.entry((group, sunk)).or_insert_with(|| {
            positions.push(if sunk { inner[v] } else { outer[v] });
            positions.len() as u32 - 1
        })
    }

    fn tri(&mut self, t: u32, sunk: bool, flip: bool) {
        let o = [0, 1, 2].map(|k| self.id(t, k, sunk));
        self.piece.tris.push(if flip { [o[2], o[1], o[0]] } else { o });
    }

    /// The wall under edge `k` of triangle `t`, which stays on the surface,
    /// down to the sunk corners of `low` (`t` itself for a shell, the
    /// neighbouring pocket for the body).
    fn wall(&mut self, t: u32, k: usize, low: u32) {
        let tri = self.painted.tris[t as usize];
        let (a, c) = (tri[k], tri[(k + 1) % 3]);
        let lt = self.painted.tris[low as usize];
        let (Some(la), Some(lc)) = ((0..3).find(|&j| lt[j] == a), (0..3).find(|&j| lt[j] == c)) else { return };
        let (ao, co) = (self.id(t, k, false), self.id(t, (k + 1) % 3, false));
        let (ai, ci) = (self.id(low, la, true), self.id(low, lc, true));
        // The same diagonal whichever piece builds the wall, so a shell's wall
        // and its pocket's are the same two triangles back to back.
        if a < c {
            self.piece.tris.push([co, ao, ci]);
            self.piece.tris.push([ao, ai, ci]);
        } else {
            self.piece.tris.push([co, ao, ai]);
            self.piece.tris.push([co, ai, ci]);
        }
    }
}

/// One closed piece per colour of `painted`, from slot 0 up; slots with no paint give none.
pub fn pieces_of(painted: &Painted, slots: usize, thickness: f32, main: Option<u8>) -> Vec<ColorPiece> {
    let inner = inward(&painted.positions, &painted.tris, thickness);
    let labels = &painted.labels;
    // The triangle on the left of each directed edge.
    let mut across: HashMap<(u32, u32), u32> = HashMap::with_capacity(painted.tris.len() * 3);
    for (i, t) in painted.tris.iter().enumerate() {
        for k in 0..3 {
            across.insert((t[k], t[(k + 1) % 3]), i as u32);
        }
    }
    // A body can have complementary pockets only if the original surface
    // encloses a volume. Open sources get printable shells for every colour.
    let closed = across.len() == painted.tris.len() * 3 && across.keys().all(|&(a, b)| across.contains_key(&(b, a)));
    let mut out = vec![];
    for slot in (0..slots.min(256)).map(|slot| slot as u8) {
        if !labels.contains(&slot) {
            continue;
        }
        let solid = closed && main == Some(slot);
        // A shell draws its colour; the body everything, its colour on the surface and the rest sunk.
        let kind = |t: usize| if solid { Some(labels[t] != slot) } else { (labels[t] == slot).then_some(false) };
        let mut b = Builder::new(painted, &inner, &across, kind, slot, solid);
        for (ti, t) in painted.tris.iter().enumerate() {
            let ti = ti as u32;
            if labels[ti as usize] != slot {
                if solid {
                    // Another colour's patch, sunk: the floor of its pocket.
                    b.tri(ti, true, false);
                }
                continue;
            }
            b.tri(ti, false, false);
            if !solid {
                // Its inside face, looking in.
                b.tri(ti, true, true);
            }
            for k in 0..3 {
                let (a, c) = (t[k], t[(k + 1) % 3]);
                // A wall where the neighbour is another colour: round the
                // shell's patch, or down into a pocket of the body.
                match across.get(&(c, a)).copied() {
                    Some(u) if labels[u as usize] != slot => b.wall(ti, k, if solid { u } else { ti }),
                    // An original boundary also needs a wall from the outer face
                    // to the inner face when it belongs to a colour shell.
                    None if !solid => b.wall(ti, k, ti),
                    _ => {}
                }
            }
        }
        out.push(b.piece);
    }
    out
}

/// `trees[part][tri]` is each triangle's painting. `thickness` is in mesh units.
pub fn color_pieces(
    mesh: &Mesh,
    trees: &[Vec<Node>],
    slots: usize,
    thickness: f32,
    solid_slot: Option<u8>,
) -> Vec<ColorPiece> {
    let mut all: Vec<Option<ColorPiece>> = (0..slots.min(256)).map(|_| None).collect();
    for (part, nodes) in mesh.parts.iter().zip(trees) {
        let surface = painted(part, nodes);
        for piece in pieces_of(&surface, slots, thickness, solid_slot) {
            let entry = &mut all[piece.slot as usize];
            if let Some(into) = entry {
                let base = into.positions.len() as u32;
                into.positions.extend(piece.positions);
                into.tris.extend(piece.tris.into_iter().map(|t| t.map(|v| v + base)));
                into.solid &= piece.solid;
            } else {
                *entry = Some(piece);
            }
        }
    }
    all.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::RepairReport;

    fn cube() -> Mesh {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
            [0.0, 0.0, 10.0],
            [10.0, 0.0, 10.0],
            [10.0, 10.0, 10.0],
            [0.0, 10.0, 10.0],
        ];
        let tris = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        Mesh {
            parts: vec![MeshPart { name: "c".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
            ..Default::default()
        }
    }

    /// Every directed edge used once, with its opposite used once too.
    fn closed(p: &ColorPiece) -> bool {
        let mut e: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &p.tris {
            for k in 0..3 {
                *e.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        e.iter().all(|(&(a, b), &n)| n == 1 && e.get(&(b, a)) == Some(&1))
    }

    fn volume(p: &ColorPiece) -> f32 {
        p.tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| p.positions[v as usize]);
                geom::dot(a, geom::cross(b, c)) / 6.0
            })
            .sum()
    }

    fn leaf(l: u8) -> Node {
        Node::Leaf(l)
    }

    fn split(k: [Node; 4]) -> Node {
        Node::Split(Box::new(k))
    }

    /// The top split twice on one triangle and not on its neighbour: T-junctions on the diagonal.
    fn painted_cube() -> Vec<Vec<Node>> {
        let mut nodes: Vec<Node> = (0..12).map(|_| leaf(0)).collect();
        nodes[2] = split([split([leaf(1), leaf(1), leaf(0), leaf(1)]), leaf(1), leaf(0), leaf(1)]);
        nodes[3] = leaf(1);
        vec![nodes]
    }

    #[test]
    fn painted_surface_follows_the_leaves_and_stays_closed() {
        let m = cube();
        let s = painted(&m.parts[0], &painted_cube()[0]);
        assert!(s.labels.iter().filter(|&&l| l == 1).count() > 2, "the leaves are kept, not the main colour");
        let all = ColorPiece { slot: 0, positions: s.positions.clone(), tris: s.tris.clone(), solid: false };
        assert!(closed(&all), "T-junctions are stitched");
        assert!((volume(&all) - 1000.0).abs() < 1e-2);
    }

    #[test]
    fn each_colour_is_a_closed_shell() {
        let m = cube();
        let pieces = color_pieces(&m, &painted_cube(), 2, 0.5, None);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(closed));
        assert!(pieces.iter().all(|p| volume(p) > 0.0));
    }

    #[test]
    fn open_painted_surface_exports_closed_shells_even_with_a_main_colour() {
        let mesh = Mesh {
            parts: vec![MeshPart {
                name: "sheet".into(),
                positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
                tris: vec![[0, 1, 2], [0, 2, 3]],
                origin: None,
            }],
            repair: RepairReport::default(),
            ..Default::default()
        };
        let trees = vec![vec![leaf(0), leaf(255)]];
        for main in [None, Some(0)] {
            let pieces = color_pieces(&mesh, &trees, 256, 0.5, main);
            assert_eq!(pieces.iter().map(|p| p.slot).collect::<Vec<_>>(), [0, 255]);
            assert!(pieces.iter().all(|p| !p.solid && closed(p)));
            for piece in &pieces {
                assert!((volume(piece) - 25.0).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn the_main_body_has_pockets_the_shells_fill_exactly() {
        let m = cube();
        let trees = painted_cube();
        let pieces = color_pieces(&m, &trees, 2, 0.5, Some(0));
        let body = pieces.iter().find(|p| p.solid).unwrap();
        let shell = pieces.iter().find(|p| !p.solid).unwrap();
        assert!(closed(body) && closed(shell));
        // Body and shell make up the cube, without overlapping.
        assert!((volume(body) + volume(shell) - 1000.0).abs() < 1e-2, "{} + {}", volume(body), volume(shell));
        assert!(volume(body) < 1000.0 - 1.0);
    }

    #[test]
    fn three_colours_meeting_keep_every_piece_closed() {
        let m = cube();
        let mut nodes: Vec<Node> = (0..12).map(|_| leaf(0)).collect();
        nodes[2] = split([leaf(1), leaf(2), leaf(1), leaf(2)]);
        nodes[3] = leaf(2);
        nodes[6] = leaf(1);
        let trees = vec![nodes];
        for main in [None, Some(0)] {
            let pieces = color_pieces(&m, &trees, 3, 0.4, main);
            assert_eq!(pieces.len(), 3);
            assert!(pieces.iter().all(closed), "{main:?}");
            if main.is_some() {
                let total: f32 = pieces.iter().map(volume).sum();
                assert!((total - 1000.0).abs() < 1e-2, "{total}");
            }
        }
    }

    /// A closed UV sphere, faces outwards.
    fn sphere(r: f32, rings: u32, segs: u32) -> Mesh {
        let mut positions = vec![[0.0, 0.0, -r]];
        for i in 1..rings {
            let a = std::f32::consts::PI * i as f32 / rings as f32 - std::f32::consts::FRAC_PI_2;
            for j in 0..segs {
                let b = std::f32::consts::TAU * j as f32 / segs as f32;
                positions.push([r * a.cos() * b.cos(), r * a.cos() * b.sin(), r * a.sin()]);
            }
        }
        positions.push([0.0, 0.0, r]);
        let top = positions.len() as u32 - 1;
        let at = |i: u32, j: u32| 1 + (i - 1) * segs + j % segs;
        let mut tris = vec![];
        for j in 0..segs {
            tris.push([0, at(1, j + 1), at(1, j)]);
            tris.push([top, at(rings - 1, j), at(rings - 1, j + 1)]);
            for i in 1..rings - 1 {
                tris.push([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
                tris.push([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
            }
        }
        Mesh {
            parts: vec![MeshPart { name: "s".into(), positions, tris, origin: None }],
            repair: RepairReport::default(),
            ..Default::default()
        }
    }

    /// A random painting: splits up to `depth` deep, three colours.
    fn random_tree(seed: &mut u64, depth: u32) -> Node {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let r = (*seed >> 33) as u32;
        if depth == 0 || !r.is_multiple_of(3) {
            return Node::Leaf((r / 3 % 3) as u8);
        }
        split([
            random_tree(seed, depth - 1),
            random_tree(seed, depth - 1),
            random_tree(seed, depth - 1),
            random_tree(seed, depth - 1),
        ])
    }

    #[test]
    fn a_randomly_painted_sphere_splits_into_closed_pieces_that_add_up() {
        let m = sphere(20.0, 16, 32);
        let whole = {
            let p = &m.parts[0];
            ColorPiece { slot: 0, positions: p.positions.clone(), tris: p.tris.clone(), solid: false }
        };
        for seed0 in 1..6u64 {
            let mut seed = seed0;
            let trees = vec![(0..m.parts[0].tris.len()).map(|_| random_tree(&mut seed, 4)).collect::<Vec<_>>()];
            let shells = color_pieces(&m, &trees, 3, 0.8, None);
            assert!(shells.iter().all(closed), "seed {seed0}");
            let with_body = color_pieces(&m, &trees, 3, 0.8, Some(0));
            assert!(with_body.iter().all(closed), "seed {seed0}");
            let total: f32 = with_body.iter().map(volume).sum();
            assert!(
                (total - volume(&whole)).abs() < volume(&whole) * 1e-4,
                "seed {seed0}: {total} vs {}",
                volume(&whole)
            );
        }
    }
}
