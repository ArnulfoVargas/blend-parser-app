//! A bounding volume hierarchy over a triangle soup: ray picking for the brush,
//! sphere queries for the brush footprint and nearest-point queries for colour transfer.

use crate::geom::{self, Bary, Vec3};

/// Triangles flattened across parts, each remembering which part and triangle it came from.
#[derive(Clone, Debug, Default)]
pub struct TriSoup {
    pub corners: Vec<[Vec3; 3]>,
    pub owner: Vec<(u32, u32)>,
}

impl TriSoup {
    pub fn push(&mut self, corners: [Vec3; 3], part: u32, tri: u32) {
        self.corners.push(corners);
        self.owner.push((part, tri));
    }
    pub fn len(&self) -> usize {
        self.corners.len()
    }
    pub fn is_empty(&self) -> bool {
        self.corners.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
struct Aabb {
    lo: Vec3,
    hi: Vec3,
}

impl Aabb {
    const EMPTY: Aabb = Aabb { lo: [f32::MAX; 3], hi: [f32::MIN; 3] };
    fn grow(&mut self, p: Vec3) {
        self.lo = std::array::from_fn(|i| self.lo[i].min(p[i]));
        self.hi = std::array::from_fn(|i| self.hi[i].max(p[i]));
    }
    fn dist2(&self, p: Vec3) -> f32 {
        (0..3).map(|i| (self.lo[i] - p[i]).max(0.0).max(p[i] - self.hi[i])).map(|d| d * d).sum()
    }
    /// Slab test; returns the entry distance.
    fn ray(&self, o: Vec3, inv: Vec3, tmax: f32) -> Option<f32> {
        let (mut t0, mut t1) = (0.0f32, tmax);
        for i in 0..3 {
            let a = (self.lo[i] - o[i]) * inv[i];
            let b = (self.hi[i] - o[i]) * inv[i];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        (t0 <= t1).then_some(t0)
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// Leaf: first index into `order`. Inner: index of the left child (right = left + 1).
    start: u32,
    /// Leaf: number of triangles. Inner: 0.
    count: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// Index into the soup.
    pub tri: u32,
    pub t: f32,
    pub bary: Bary,
    pub point: Vec3,
    pub normal: Vec3,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(soup: &TriSoup) -> Self {
        let n = soup.len();
        if n == 0 {
            return Self::default();
        }
        let centroid: Vec<Vec3> =
            soup.corners.iter().map(|c| geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0)).collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes = vec![Node { bounds: Aabb::EMPTY, start: 0, count: n as u32 }];
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let (start, count) = (nodes[ni].start as usize, nodes[ni].count as usize);
            let mut b = Aabb::EMPTY;
            let mut cb = Aabb::EMPTY;
            for &t in &order[start..start + count] {
                for p in soup.corners[t as usize] {
                    b.grow(p);
                }
                cb.grow(centroid[t as usize]);
            }
            nodes[ni].bounds = b;
            if count <= LEAF {
                continue;
            }
            let ext = geom::sub(cb.hi, cb.lo);
            let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
                0
            } else if ext[1] >= ext[2] {
                1
            } else {
                2
            };
            if ext[axis] <= 0.0 {
                continue;
            }
            let half = count / 2;
            order[start..start + count].select_nth_unstable_by(half, |&a, &b| {
                centroid[a as usize][axis].total_cmp(&centroid[b as usize][axis])
            });
            let left = nodes.len();
            nodes.push(Node { bounds: Aabb::EMPTY, start: start as u32, count: half as u32 });
            nodes.push(Node { bounds: Aabb::EMPTY, start: (start + half) as u32, count: (count - half) as u32 });
            nodes[ni].start = left as u32;
            nodes[ni].count = 0;
            stack.push(left);
            stack.push(left + 1);
        }
        Self { nodes, order }
    }

    pub fn raycast(&self, soup: &TriSoup, o: Vec3, d: Vec3) -> Option<RayHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut best: Option<RayHit> = None;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni];
            let tmax = best.map_or(f32::MAX, |h| h.t);
            if node.bounds.ray(o, inv, tmax).is_none() {
                continue;
            }
            if node.count == 0 {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
                continue;
            }
            for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let c = &soup.corners[t as usize];
                if let Some((dist, bary)) = geom::ray_triangle(o, d, c) {
                    if dist < best.map_or(f32::MAX, |h| h.t) {
                        let normal = geom::normalize(geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0])));
                        best = Some(RayHit { tri: t, t: dist, bary, point: geom::at(c, bary), normal });
                    }
                }
            }
        }
        best
    }

    /// Every crossing of the ray, unsorted: its distance and +1 when the ray
    /// enters the surface (against the triangle normal) or -1 when it leaves.
    pub fn ray_crossings(&self, soup: &TriSoup, o: Vec3, d: Vec3, out: &mut Vec<(f32, i8)>) {
        if self.nodes.is_empty() {
            return;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni];
            if node.bounds.ray(o, inv, f32::MAX).is_none() {
                continue;
            }
            if node.count == 0 {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
                continue;
            }
            for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let c = &soup.corners[t as usize];
                if let Some((dist, _)) = geom::ray_triangle(o, d, c) {
                    let n = geom::cross(geom::sub(c[1], c[0]), geom::sub(c[2], c[0]));
                    out.push((dist, if geom::dot(n, d) < 0.0 { 1 } else { -1 }));
                }
            }
        }
    }

    /// Triangles with any corner or their centroid inside the sphere.
    pub fn sphere(&self, soup: &TriSoup, center: Vec3, r: f32, out: &mut Vec<u32>) {
        if self.nodes.is_empty() {
            return;
        }
        let r2 = r * r;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni];
            if node.bounds.dist2(center) > r2 {
                continue;
            }
            if node.count == 0 {
                stack.push(node.start as usize);
                stack.push(node.start as usize + 1);
                continue;
            }
            for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let c = &soup.corners[t as usize];
                let cen = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
                let inside = |p: Vec3| {
                    let d = geom::sub(p, center);
                    geom::dot(d, d) <= r2
                };
                if inside(cen) || c.iter().any(|&p| inside(p)) {
                    out.push(t);
                }
            }
        }
    }

    /// Nearest triangle to `q` and the barycentrics of the closest point on it.
    pub fn nearest(&self, soup: &TriSoup, q: Vec3) -> Option<(u32, Bary)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = (f32::MAX, 0u32, [1.0, 0.0, 0.0]);
        let mut stack = vec![(0usize, 0.0f32)];
        while let Some((ni, d)) = stack.pop() {
            if d >= best.0 {
                continue;
            }
            let node = self.nodes[ni];
            if node.count == 0 {
                let (l, r) = (node.start as usize, node.start as usize + 1);
                let (dl, dr) = (self.nodes[l].bounds.dist2(q), self.nodes[r].bounds.dist2(q));
                // Push the farther child first so the nearer one is visited first.
                if dl < dr {
                    stack.push((r, dr));
                    stack.push((l, dl));
                } else {
                    stack.push((l, dl));
                    stack.push((r, dr));
                }
                continue;
            }
            for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                let c = &soup.corners[t as usize];
                let b = geom::closest_on_triangle(q, c);
                let p = geom::sub(geom::at(c, b), q);
                let d2 = geom::dot(p, p);
                if d2 < best.0 {
                    best = (d2, t, b);
                }
            }
        }
        (best.0 < f32::MAX).then_some((best.1, best.2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize) -> TriSoup {
        let mut s = TriSoup::default();
        for y in 0..n {
            for x in 0..n {
                let (x0, y0, x1, y1) = (x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0);
                s.push([[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0]], 0, (2 * (y * n + x)) as u32);
                s.push([[x0, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]], 0, (2 * (y * n + x) + 1) as u32);
            }
        }
        s
    }

    #[test]
    fn raycast_matches_brute_force() {
        let s = grid(20);
        let bvh = Bvh::build(&s);
        let hit = bvh.raycast(&s, [7.3, 4.6, 5.0], [0.0, 0.0, -1.0]).unwrap();
        assert!((hit.point[0] - 7.3).abs() < 1e-4 && (hit.point[1] - 4.6).abs() < 1e-4);
        assert!(bvh.raycast(&s, [70.0, 4.6, 5.0], [0.0, 0.0, -1.0]).is_none());
    }

    #[test]
    fn sphere_finds_neighbourhood() {
        let s = grid(20);
        let bvh = Bvh::build(&s);
        let mut out = vec![];
        bvh.sphere(&s, [10.0, 10.0, 0.0], 1.0, &mut out);
        assert!(out.len() >= 8 && out.len() < 30, "{}", out.len());
    }

    #[test]
    fn nearest_point_on_grid() {
        let s = grid(10);
        let bvh = Bvh::build(&s);
        let (t, b) = bvh.nearest(&s, [3.5, 2.2, 4.0]).unwrap();
        let p = geom::at(&s.corners[t as usize], b);
        assert!((p[0] - 3.5).abs() < 1e-4 && (p[1] - 2.2).abs() < 1e-4 && p[2].abs() < 1e-6);
    }
}
