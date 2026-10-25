//! The colour at any point of the working mesh. Triangles that still know
//! their source triangle read the source UVs directly; after simplification
//! colours are transferred from the closest point on the original surface.

use crate::bvh::{Bvh, TriSoup};
use crate::color::Sample;
use crate::geom::{self, Bary};
use crate::mesh::Mesh;
use crate::scene::Scene;

pub struct ColorField<'a> {
    scene: &'a Scene,
    transfer: Option<(Bvh, TriSoup)>,
}

impl<'a> ColorField<'a> {
    pub fn new(scene: &'a Scene, mesh: &Mesh) -> Self {
        let transfer = mesh.parts.iter().any(|p| p.origin.is_none()).then(|| {
            let mut soup = TriSoup::default();
            for (pi, p) in scene.parts.iter().enumerate() {
                for t in 0..p.tris.len() {
                    soup.push(p.corners(t), pi as u32, t as u32);
                }
            }
            (Bvh::build(&soup), soup)
        });
        Self { scene, transfer }
    }

    /// Colour at barycentric `b` of triangle `tri` of part `part`. `mesh` is the
    /// working mesh before flattening, which has the same triangles as the final one.
    pub fn sample(&self, mesh: &Mesh, part: usize, tri: usize, b: Bary) -> Sample {
        let mp = &mesh.parts[part];
        if let Some(origin) = &mp.origin {
            let (src, corners) = &origin[tri];
            return self.scene.parts[part].sample(&self.scene.images, *src as usize, geom::bary_in(corners, b));
        }
        let Some((bvh, soup)) = &self.transfer else { return Sample::Float([0.8, 0.8, 0.8]) };
        match bvh.nearest(soup, geom::at(&mp.corners(tri), b)) {
            Some((t, sb)) => {
                let (sp, st) = soup.owner[t as usize];
                self.scene.parts[sp as usize].sample(&self.scene.images, st as usize, sb)
            }
            None => Sample::Float([0.8, 0.8, 0.8]),
        }
    }
}
