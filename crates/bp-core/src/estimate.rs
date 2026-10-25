//! Filament swaps, the main cost of a multicolour print.
//!
//! The model is sliced into layers and each layer gets the set of colours it
//! prints. A colour only counts on a layer when it covers enough of it to be
//! printed at all: a sliver narrower than two lines of the nozzle is merged
//! into its neighbours by the slicer and never causes a swap.
//!
//! The order of the colours is then planned across all layers, as Bambu
//! Studio does: each layer starts with the filament already loaded when it
//! prints it, and the order and the colour a layer ends on are chosen to
//! keep the total cost lowest, counting either swaps or flush volume. The
//! real numbers depend on the slicer's settings, so this is a guide for
//! comparing settings, not a promise.

use std::collections::HashMap;
use std::rc::Rc;

use crate::geom::{self, ROOT};
use crate::mesh::Mesh;
use crate::paint::Node;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwapEstimate {
    pub layers: u32,
    /// Layers that print more than one colour.
    pub multicolor_layers: u32,
    /// Fewest swaps the layers need.
    pub swaps: u32,
    /// The colours of each layer that prints any, as (layer, slot bit mask), bottom up.
    pub layer_sets: Vec<(u32, u32)>,
    /// The swaps of the plan with fewest swaps, as (layer, from slot, to slot).
    pub changes: Vec<(u32, u8, u8)>,
}

/// `scale` turns mesh units into millimetres; layers are `layer_mm` tall. A
/// colour counts on a layer once its surface there reaches `min_area_mm2`.
pub fn estimate(mesh: &Mesh, trees: &[Vec<Node>], scale: f32, layer_mm: f32, min_area_mm2: f32) -> SwapEstimate {
    let Some((lo, hi)) = mesh.bounds() else { return SwapEstimate::default() };
    let layers = (((hi[2] - lo[2]) * scale / layer_mm).ceil() as usize).max(1);
    // Surface of each colour on each layer, in mm².
    let mut area = vec![[0.0f32; 32]; layers];
    let layer_of = |z: f32| ((((z - lo[2]) * scale) / layer_mm) as usize).min(layers - 1);
    for (part, nodes) in mesh.parts.iter().zip(trees) {
        for (ti, node) in nodes.iter().enumerate() {
            let c = part.corners(ti);
            node.for_each_leaf(&ROOT, &mut |t, label| {
                let p = t.map(|b| geom::at(&c, b));
                let (z0, z1) =
                    (p.iter().map(|v| v[2]).fold(f32::MAX, f32::min), p.iter().map(|v| v[2]).fold(f32::MIN, f32::max));
                let (l0, l1) = (layer_of(z0), layer_of(z1));
                // Spread over the layers it spans; a hair over zero keeps flat bits visible.
                let share = (geom::area(&p) * scale * scale).max(1e-9) / (l1 - l0 + 1) as f32;
                for a in &mut area[l0..=l1] {
                    a[label.min(31) as usize] += share;
                }
            });
        }
    }
    let layer_sets: Vec<(u32, u32)> = area
        .iter()
        .enumerate()
        .filter_map(|(l, a)| {
            let mut m = (0..32).filter(|&s| a[s] > 0.0 && a[s] >= min_area_mm2).fold(0u32, |m, s| m | 1 << s);
            // A layer printed only in slivers still prints its largest colour.
            if m == 0 {
                let (s, &v) = a.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1))?;
                if v > 0.0 {
                    m = 1 << s;
                }
            }
            (m != 0).then_some((l as u32, m))
        })
        .collect();
    let changes = plan(&layer_sets, |_, _| 1.0);
    SwapEstimate {
        layers: layers as u32,
        multicolor_layers: layer_sets.iter().filter(|(_, m)| m.count_ones() > 1).count() as u32,
        swaps: changes.len() as u32,
        changes,
        layer_sets,
    }
}

/// Nothing loaded yet: the first colour is loaded, not swapped to.
const NONE: usize = 32;
/// Above this many colours on a layer the order is chosen greedily.
const EXACT_UP_TO: usize = 8;

/// One way through a layer: its cost, the colour it ends on and the colours in order.
type Tour = (f32, u8, Vec<u8>);
/// A tour with its order shared, as the plan keeps it.
type SharedTour = (f32, u8, Rc<Vec<u8>>);
/// How a layer was reached: the colour it ends on, the one it started from, its order.
type Step = (usize, usize, Rc<Vec<u8>>);

/// The cheapest way to print every colour of `mask` from `start`, for each colour it can end on.
fn tours(mask: u32, start: usize, cost: &impl Fn(u8, u8) -> f32) -> Vec<Tour> {
    let el: Vec<u8> = (0..32u8).filter(|&s| mask & (1 << s) != 0).collect();
    let k = el.len();
    let step = |from: usize, to: u8| if from == NONE || from == to as usize { 0.0 } else { cost(from as u8, to) };
    let first = el.iter().position(|&e| e as usize == start);

    if k > EXACT_UP_TO {
        // Nearest next colour each time.
        let mut left = el.clone();
        let mut order = Vec::with_capacity(k);
        let (mut at, mut total) = (start, 0.0);
        if let Some(i) = first {
            order.push(left.remove(i));
        }
        while !left.is_empty() {
            let (i, c) =
                left.iter().enumerate().map(|(i, &e)| (i, step(at, e))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            total += c;
            at = left[i] as usize;
            order.push(left.remove(i));
        }
        return vec![(total, *order.last().unwrap(), order)];
    }

    // Held-Karp: cheapest path from `start` through a subset, ending at each colour.
    let full = (1usize << k) - 1;
    let mut dp = vec![f32::INFINITY; (full + 1) * k];
    let mut parent = vec![usize::MAX; (full + 1) * k];
    for j in 0..k {
        if first.is_none_or(|f| f == j) {
            dp[(1 << j) * k + j] = step(start, el[j]);
        }
    }
    for set in 1..=full {
        for j in 0..k {
            let here = dp[set * k + j];
            if set & (1 << j) == 0 || here.is_infinite() {
                continue;
            }
            for n in 0..k {
                if set & (1 << n) != 0 {
                    continue;
                }
                let next = set | (1 << n);
                let c = here + cost(el[j], el[n]);
                if c < dp[next * k + n] {
                    dp[next * k + n] = c;
                    parent[next * k + n] = j;
                }
            }
        }
    }
    (0..k)
        .filter(|&j| dp[full * k + j].is_finite())
        .map(|j| {
            let mut order = Vec::with_capacity(k);
            let (mut set, mut at) = (full, j);
            while at != usize::MAX {
                order.push(el[at]);
                let p = parent[set * k + at];
                set &= !(1 << at);
                at = p;
            }
            order.reverse();
            (dp[full * k + j], el[j], order)
        })
        .collect()
}

/// The order of colours across all layers that keeps the total `cost` of the
/// swaps lowest. Returns the swaps as (layer, from slot, to slot).
pub fn plan(layer_sets: &[(u32, u32)], cost: impl Fn(u8, u8) -> f32) -> Vec<(u32, u8, u8)> {
    let mut best = [f32::INFINITY; NONE + 1];
    best[NONE] = 0.0;
    // For each layer and colour it can end on: the colour it started from and its order.
    let mut back: Vec<Vec<Step>> = Vec::with_capacity(layer_sets.len());
    let mut cache: HashMap<(u32, usize), Vec<SharedTour>> = HashMap::new();
    for &(_, mask) in layer_sets {
        let mut next = [f32::INFINITY; NONE + 1];
        let mut from = vec![(usize::MAX, usize::MAX, Rc::new(vec![])); NONE + 1];
        for (s, &so_far) in best.iter().enumerate().filter(|(_, c)| c.is_finite()) {
            let options = cache
                .entry((mask, s))
                .or_insert_with(|| tours(mask, s, &cost).into_iter().map(|(c, e, o)| (c, e, Rc::new(o))).collect());
            for (c, end, order) in options.iter() {
                let total = so_far + c;
                if total < next[*end as usize] {
                    next[*end as usize] = total;
                    from[*end as usize] = (*end as usize, s, order.clone());
                }
            }
        }
        best = next;
        back.push(from.into_iter().filter(|f| f.0 != usize::MAX).collect());
    }
    // Walk back from the cheapest last colour.
    let Some(mut end) =
        best.iter().enumerate().filter(|(_, c)| c.is_finite()).min_by(|a, b| a.1.total_cmp(b.1)).map(|x| x.0)
    else {
        return vec![];
    };
    let mut orders = Vec::with_capacity(layer_sets.len());
    for step in back.iter().rev() {
        let (_, start, order) = step.iter().find(|f| f.0 == end).expect("a path reaches every finite state");
        orders.push((*start, order.clone()));
        end = *start;
    }
    orders.reverse();
    let mut changes = Vec::new();
    for (&(layer, _), (start, order)) in layer_sets.iter().zip(orders) {
        let mut at = start;
        for &c in order.iter() {
            if at != NONE && at != c as usize {
                changes.push((layer, at as u8, c));
            }
            at = c as usize;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::MeshPart;

    fn mesh(positions: Vec<[f32; 3]>, tris: Vec<[u32; 3]>) -> Mesh {
        Mesh { parts: vec![MeshPart { name: "t".into(), positions, tris, origin: None }], repair: Default::default() }
    }

    #[test]
    fn two_colours_stacked_need_one_swap() {
        // Two triangles, one under the other, 2 mm tall in total.
        let positions =
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.9], [0.0, 0.0, 1.1], [1.0, 0.0, 1.1], [0.0, 0.0, 2.0]];
        let mesh = mesh(positions, vec![[0, 1, 2], [3, 4, 5]]);
        let trees = [vec![Node::Leaf(0), Node::Leaf(1)]];
        let e = estimate(&mesh, &trees, 1.0, 0.2, 0.0);
        assert_eq!(e.layers, 10);
        assert_eq!(e.swaps, 1);
        assert_eq!(e.multicolor_layers, 0);
        assert_eq!(e.changes, vec![(5, 0, 1)]);
        assert_eq!(estimate(&mesh, &trees, 1.0, 0.4, 0.0).layers, 5);
    }

    #[test]
    fn side_by_side_colours_swap_once_per_layer() {
        // Two colours next to each other on every layer, 1 mm tall: five layers.
        let positions =
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 0.0, 1.0]];
        let mesh = mesh(positions, vec![[0, 1, 2], [3, 4, 5]]);
        let e = estimate(&mesh, &[vec![Node::Leaf(0), Node::Leaf(1)]], 1.0, 0.2, 0.0);
        assert_eq!((e.layers, e.multicolor_layers, e.swaps), (5, 5, 5));
        // Each layer starts on the colour the last one ended on.
        assert!(e.changes.windows(2).all(|w| w[0].2 == w[1].1 && w[1].0 == w[0].0 + 1), "{:?}", e.changes);
    }

    #[test]
    fn slivers_too_thin_to_print_do_not_swap() {
        // A wide wall in slot 0 and a thin strip in slot 1 next to it, 1 mm tall.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [10.0, 0.0, 0.0],
            [10.1, 0.0, 0.0],
            [10.0, 0.0, 1.0],
        ];
        let mesh = mesh(positions, vec![[0, 1, 2], [3, 4, 5]]);
        let trees = [vec![Node::Leaf(0), Node::Leaf(1)]];
        assert_eq!(estimate(&mesh, &trees, 1.0, 0.2, 0.0).swaps, 5);
        // Two 0.4 mm lines a layer: the strip is narrower and never swaps.
        assert_eq!(estimate(&mesh, &trees, 1.0, 0.2, 2.0 * 0.4 * 0.2).swaps, 0);
    }

    #[test]
    fn the_plan_orders_colours_to_purge_least() {
        // Going 0 -> 1 is dear, 1 -> 0 cheap. Each layer needs one swap and they
        // alternate, so the cheapest plan starts on 1 to take the cheap way twice;
        // printing in slot order would take the dear way twice (1100).
        let sets = vec![(0, 0b11), (1, 0b11), (2, 0b11)];
        let dear = |a: u8, b: u8| if (a, b) == (0, 1) { 500.0 } else { 100.0 };
        let changes = plan(&sets, dear);
        let total: f32 = changes.iter().map(|&(_, a, b)| dear(a, b)).sum();
        assert_eq!(changes.len(), 3);
        assert_eq!(total, 700.0, "{changes:?}");
        assert_eq!(changes[0], (0, 1, 0));
        // Unit costs: the fewest swaps.
        assert_eq!(plan(&sets, |_, _| 1.0).len(), 3);
    }

    #[test]
    fn many_colours_fall_back_to_greedy() {
        let sets = vec![(0, 0x3ff), (1, 0x3ff)];
        let changes = plan(&sets, |a, b| (a as f32 - b as f32).abs());
        // Ten colours a layer: nine swaps on the first, at least nine on the second.
        assert!(changes.len() >= 18 && changes.len() <= 19, "{}", changes.len());
    }
}
