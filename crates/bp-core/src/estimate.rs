//! A rough count of filament swaps, the main cost of a multicolour print.
//!
//! The model is sliced into layers; a layer needs one swap per extra colour on
//! it, plus one when it cannot start with the colour the previous layer ended on.
//! Bambu Studio orders colours per layer in a similar greedy way; the real
//! number depends on its flushing and ordering settings, so this is a guide for
//! comparing settings, not a promise. Each swap is kept with its direction, so
//! [`crate::waste`] can price it with the flush volume of that pair.

use crate::geom::{self, ROOT};
use crate::mesh::Mesh;
use crate::paint::Node;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwapEstimate {
    pub layers: u32,
    /// Layers that print more than one colour.
    pub multicolor_layers: u32,
    pub swaps: u32,
    /// The swaps of each layer that has any, as (layer, from slot, to slot), bottom up.
    pub changes: Vec<(u32, u8, u8)>,
    /// Slots in the order they first print; the first one is loaded, not swapped to.
    pub first_use: Vec<u8>,
}

impl SwapEstimate {
    /// Layers up to the last swap: how tall a prime tower has to grow.
    pub fn tower_layers(&self) -> u32 {
        self.changes.last().map_or(0, |c| c.0 + 1)
    }

    /// Swaps per layer, for the layers that have any.
    pub fn per_layer(&self) -> impl Iterator<Item = &[(u32, u8, u8)]> {
        self.changes.chunk_by(|a, b| a.0 == b.0)
    }
}

/// `scale` turns mesh units into millimetres; layers are `layer_mm` tall.
pub fn estimate(mesh: &Mesh, trees: &[Vec<Node>], scale: f32, layer_mm: f32) -> SwapEstimate {
    let Some((lo, hi)) = mesh.bounds() else { return SwapEstimate::default() };
    let layers = (((hi[2] - lo[2]) * scale / layer_mm).ceil() as usize).max(1);
    let mut masks = vec![0u32; layers];
    let layer_of = |z: f32| ((((z - lo[2]) * scale) / layer_mm) as usize).min(layers - 1);
    for (part, nodes) in mesh.parts.iter().zip(trees) {
        for (ti, node) in nodes.iter().enumerate() {
            let c = part.corners(ti);
            node.for_each_leaf(&ROOT, &mut |t, label| {
                let z: Vec<f32> = t.iter().map(|&b| geom::at(&c, b)[2]).collect();
                let (z0, z1) = (z.iter().copied().fold(f32::MAX, f32::min), z.iter().copied().fold(f32::MIN, f32::max));
                for m in &mut masks[layer_of(z0)..=layer_of(z1)] {
                    *m |= 1 << (label.min(31));
                }
            });
        }
    }
    let mut e = SwapEstimate { layers: layers as u32, ..Default::default() };
    let mut current: Option<u8> = None;
    for (layer, &m) in masks.iter().enumerate() {
        if m == 0 {
            continue;
        }
        if m.count_ones() > 1 {
            e.multicolor_layers += 1;
        }
        // The layer starts on the colour already loaded when it prints it, then
        // goes through the others from the lowest slot up.
        let mut order: Vec<u8> = (0..32u8).filter(|&s| m & (1 << s) != 0).collect();
        if let Some(c) = current.filter(|&c| m & (1 << c) != 0) {
            order.retain(|&s| s != c);
            order.insert(0, c);
        }
        for &s in &order {
            match current {
                None => {}
                Some(c) if c == s => continue,
                Some(c) => e.changes.push((layer as u32, c, s)),
            }
            if !e.first_use.contains(&s) {
                e.first_use.push(s);
            }
            current = Some(s);
        }
    }
    e.swaps = e.changes.len() as u32;
    e
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
        let e = estimate(&mesh, &trees, 1.0, 0.2);
        assert_eq!(e.layers, 10);
        assert_eq!(e.swaps, 1);
        assert_eq!(e.multicolor_layers, 0);
        assert_eq!(e.changes, vec![(5, 0, 1)]);
        assert_eq!(e.first_use, vec![0, 1]);
        assert_eq!(e.tower_layers(), 6);
        assert_eq!(estimate(&mesh, &trees, 1.0, 0.4).layers, 5);
    }

    #[test]
    fn side_by_side_colours_swap_back_and_forth() {
        // Two colours next to each other on every layer, 1 mm tall: five layers.
        let positions =
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 0.0, 1.0]];
        let mesh = mesh(positions, vec![[0, 1, 2], [3, 4, 5]]);
        let e = estimate(&mesh, &[vec![Node::Leaf(0), Node::Leaf(1)]], 1.0, 0.2);
        assert_eq!((e.layers, e.multicolor_layers, e.swaps), (5, 5, 5));
        // Each layer starts on the colour the last one ended on.
        assert_eq!(&e.changes[..3], &[(0, 0, 1), (1, 1, 0), (2, 0, 1)]);
        assert_eq!(e.per_layer().count(), 5);
    }
}
