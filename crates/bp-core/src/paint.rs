//! Per-triangle filament painting as a quadtree of 1->4 splits, the structure
//! Bambu Studio's `TriangleSelector` stores in `paint_color`.

use rayon::prelude::*;

use crate::color::Classifier;
use crate::field::ColorField;
use crate::geom::{self, Bary, BaryTri, ROOT};
use crate::mesh::Mesh;

/// A painted triangle. Leaf labels are 0-based filament slots.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(u8),
    Split(Box<[Node; 4]>),
}

impl Node {
    /// Visits every leaf with its corners (as root barycentrics).
    pub fn for_each_leaf(&self, t: &BaryTri, f: &mut impl FnMut(&BaryTri, u8)) {
        match self {
            Node::Leaf(l) => f(t, *l),
            Node::Split(kids) => {
                for (kid, sub) in kids.iter().zip(geom::split4(t)) {
                    kid.for_each_leaf(&sub, f);
                }
            }
        }
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Split(k) => k.iter().map(Node::leaf_count).sum(),
        }
    }

    /// The label covering the most area.
    pub fn dominant(&self) -> u8 {
        match self {
            Node::Leaf(l) => *l,
            Node::Split(_) => {
                let mut area = [0.0f32; 256];
                self.area_by_label(1.0, &mut area);
                area.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i as u8)
            }
        }
    }

    fn area_by_label(&self, a: f32, out: &mut [f32; 256]) {
        match self {
            Node::Leaf(l) => out[*l as usize] += a,
            Node::Split(k) => k.iter().for_each(|n| n.area_by_label(a * 0.25, out)),
        }
    }

    /// Whether any leaf has `label`.
    pub fn has(&self, label: u8) -> bool {
        match self {
            Node::Leaf(l) => *l == label,
            Node::Split(k) => k.iter().any(|n| n.has(label)),
        }
    }

    pub fn is_split(&self) -> bool {
        matches!(self, Node::Split(_))
    }

    /// Relabels every leaf through `map` (old slot -> new slot), collapsing
    /// splits whose children end up the same colour.
    pub fn relabel(&self, map: &[u8]) -> Node {
        match self {
            Node::Leaf(l) => Node::Leaf(map.get(*l as usize).copied().unwrap_or(*l)),
            Node::Split(k) => {
                let kids = [k[0].relabel(map), k[1].relabel(map), k[2].relabel(map), k[3].relabel(map)];
                if let [Node::Leaf(a), Node::Leaf(b), Node::Leaf(c), Node::Leaf(d)] = &kids {
                    if a == b && b == c && c == d {
                        return Node::Leaf(*a);
                    }
                }
                Node::Split(Box::new(kids))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relabel_collapses_uniform_splits() {
        let n = Node::Split(Box::new([Node::Leaf(0), Node::Leaf(2), Node::Leaf(0), Node::Leaf(2)]));
        assert_eq!(n.relabel(&[0, 1, 0]), Node::Leaf(0));
        assert!(n.relabel(&[0, 1, 1]).is_split());
    }
}

/// Where each node looks at the texture: the centre, three points toward the
/// corners and three toward the edges.
const SAMPLES: [Bary; 7] = [
    [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
    [0.70, 0.15, 0.15],
    [0.15, 0.70, 0.15],
    [0.15, 0.15, 0.70],
    [0.45, 0.45, 0.10],
    [0.10, 0.45, 0.45],
    [0.45, 0.10, 0.45],
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintParams {
    /// Maximum subdivision depth at colour borders (0 paints whole triangles).
    pub max_depth: u8,
    /// Do not split triangles whose longest edge is already under this (source units).
    pub min_edge: f32,
}

struct Ctx<'a> {
    field: &'a ColorField<'a>,
    shape: &'a Mesh,
    cls: &'a Classifier,
    params: PaintParams,
    part: usize,
    tri: usize,
}

fn build_node(cx: &Ctx, t: &BaryTri, edge: f32, depth: u8) -> Node {
    let labels: [u8; 7] =
        SAMPLES.map(|s| cx.cls.nearest(cx.field.sample(cx.shape, cx.part, cx.tri, geom::bary_in(t, s)).lab()));
    if labels.iter().all(|&l| l == labels[0]) {
        return Node::Leaf(labels[0]);
    }
    if depth < cx.params.max_depth && edge > cx.params.min_edge {
        let kids = geom::split4(t).map(|sub| build_node(cx, &sub, edge * 0.5, depth + 1));
        if let [Node::Leaf(a), Node::Leaf(b), Node::Leaf(c), Node::Leaf(d)] = &kids {
            if a == b && b == c && c == d {
                return Node::Leaf(*a);
            }
        }
        return Node::Split(Box::new(kids));
    }
    // Majority vote; the centre sample breaks ties.
    let mut counts = [0u8; 256];
    for &l in &labels {
        counts[l as usize] += 2;
    }
    counts[labels[0] as usize] += 1;
    Node::Leaf(counts.iter().enumerate().max_by_key(|c| *c.1).map_or(0, |(i, _)| i as u8))
}

/// Paints triangle `tri` of part `part` alone, as [`build`] does.
pub fn build_one(
    field: &ColorField,
    shape: &Mesh,
    cls: &Classifier,
    params: PaintParams,
    part: usize,
    tri: usize,
) -> Node {
    let cx = Ctx { field, shape, cls, params, part, tri };
    build_node(&cx, &ROOT, geom::longest_edge(&shape.parts[part].corners(tri)), 0)
}

/// Paints every triangle of every part, in parallel.
pub fn build(field: &ColorField, shape: &Mesh, cls: &Classifier, params: PaintParams) -> Vec<Vec<Node>> {
    shape
        .parts
        .iter()
        .enumerate()
        .map(|(pi, part)| {
            (0..part.tris.len())
                .into_par_iter()
                .map(|ti| {
                    let cx = Ctx { field, shape, cls, params, part: pi, tri: ti };
                    build_node(&cx, &ROOT, geom::longest_edge(&part.corners(ti)), 0)
                })
                .collect()
        })
        .collect()
}
