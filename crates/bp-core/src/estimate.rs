//! A rough count of filament swaps, the main cost of a multicolour print.
//!
//! The model is sliced into layers; a layer needs one swap per extra colour on
//! it, plus one when it cannot start with the colour the previous layer ended on.
//! Bambu Studio orders colours per layer in a similar greedy way; the real
//! number depends on its flushing and ordering settings, so this is a guide for
//! comparing settings, not a promise.

use crate::geom::{self, ROOT};
use crate::mesh::Mesh;
use crate::paint::Node;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwapEstimate {
    pub layers: u32,
    /// Layers that print more than one colour.
    pub multicolor_layers: u32,
    pub swaps: u32,
    /// Flushed filament, at `flush_mm3` per swap and PLA density.
    pub purge_grams: f32,
}

pub const FLUSH_MM3: f32 = 280.0;
const PLA_G_PER_MM3: f32 = 0.001_24;

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
    let mut swaps = 0u32;
    let mut multicolor = 0u32;
    let mut current: Option<u32> = None;
    for &m in &masks {
        if m == 0 {
            continue;
        }
        let k = m.count_ones();
        if k > 1 {
            multicolor += 1;
        }
        match current {
            Some(c) if m & (1 << c) != 0 => {
                swaps += k - 1;
                // Ends on another colour of this layer when there is one.
                if k > 1 {
                    current = Some(31 - (m & !(1 << c)).leading_zeros());
                }
            }
            Some(_) => {
                swaps += k;
                current = Some(31 - m.leading_zeros());
            }
            None => {
                swaps += k - 1;
                current = Some(31 - m.leading_zeros());
            }
        }
    }
    SwapEstimate {
        layers: layers as u32,
        multicolor_layers: multicolor,
        swaps,
        purge_grams: swaps as f32 * FLUSH_MM3 * PLA_G_PER_MM3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::MeshPart;

    #[test]
    fn two_colours_stacked_need_one_swap() {
        // Two triangles, one under the other, 2 mm tall in total.
        let positions =
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.9], [0.0, 0.0, 1.1], [1.0, 0.0, 1.1], [0.0, 0.0, 2.0]];
        let mesh = Mesh {
            parts: vec![MeshPart { name: "t".into(), positions, tris: vec![[0, 1, 2], [3, 4, 5]], origin: None }],
            repair: Default::default(),
        };
        let trees = [vec![Node::Leaf(0), Node::Leaf(1)]];
        let e = estimate(&mesh, &trees, 1.0, 0.2);
        assert_eq!(e.layers, 10);
        assert_eq!(e.swaps, 1);
        assert_eq!(e.multicolor_layers, 0);
        assert_eq!(estimate(&mesh, &trees, 1.0, 0.4).layers, 5);
    }
}
