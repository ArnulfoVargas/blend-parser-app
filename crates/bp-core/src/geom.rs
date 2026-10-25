//! Vector math helpers for `[f32; 3]` and barycentric points.

pub type Vec3 = [f32; 3];
/// Barycentric weights of a point inside a triangle.
pub type Bary = [f32; 3];
/// The three corners of a (sub)triangle, as barycentrics of its root triangle.
pub type BaryTri = [Bary; 3];

pub const ROOT: BaryTri = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
pub fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
pub fn normalize(a: Vec3) -> Vec3 {
    let l = len(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        a
    }
}
#[inline]
pub fn mid(a: Bary, b: Bary) -> Bary {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

/// Point at barycentric `b` of triangle `(p0, p1, p2)`.
#[inline]
pub fn at(p: &[Vec3; 3], b: Bary) -> Vec3 {
    [
        p[0][0] * b[0] + p[1][0] * b[1] + p[2][0] * b[2],
        p[0][1] * b[0] + p[1][1] * b[1] + p[2][1] * b[2],
        p[0][2] * b[0] + p[1][2] * b[1] + p[2][2] * b[2],
    ]
}

/// Barycentric point `b` expressed inside sub-triangle `t`, back in root barycentrics.
#[inline]
pub fn bary_in(t: &BaryTri, b: Bary) -> Bary {
    [
        t[0][0] * b[0] + t[1][0] * b[1] + t[2][0] * b[2],
        t[0][1] * b[0] + t[1][1] * b[1] + t[2][1] * b[2],
        t[0][2] * b[0] + t[1][2] * b[1] + t[2][2] * b[2],
    ]
}

pub fn area(p: &[Vec3; 3]) -> f32 {
    0.5 * len(cross(sub(p[1], p[0]), sub(p[2], p[0])))
}

pub fn longest_edge(p: &[Vec3; 3]) -> f32 {
    len(sub(p[1], p[0])).max(len(sub(p[2], p[1]))).max(len(sub(p[0], p[2])))
}

/// Children of a 1->4 split, in the order Bambu's `TriangleSelector` uses with
/// `special_side = 0`: corner A, corner B, corner C, centre.
pub fn split4(t: &BaryTri) -> [BaryTri; 4] {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
    [[a, ab, ca], [ab, b, bc], [bc, c, ca], [ab, bc, ca]]
}

/// Möller–Trumbore. Returns the ray parameter `t` and the barycentrics of the hit.
pub fn ray_triangle(o: Vec3, d: Vec3, p: &[Vec3; 3]) -> Option<(f32, Bary)> {
    let e1 = sub(p[1], p[0]);
    let e2 = sub(p[2], p[0]);
    let h = cross(d, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-12 {
        return None;
    }
    let f = 1.0 / a;
    let s = sub(o, p[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * dot(e2, q);
    (t > 1e-6).then_some((t, [1.0 - u - v, u, v]))
}

/// Closest point on triangle `p` to `q` (Ericson, Real-Time Collision Detection 5.1.5).
/// Returns the barycentrics of that point.
pub fn closest_on_triangle(q: Vec3, p: &[Vec3; 3]) -> Bary {
    let (a, b, c) = (p[0], p[1], p[2]);
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(q, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = sub(q, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = sub(q, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [1.0 - v - w, v, w]
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: [Vec3; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    #[test]
    fn ray_hits_inside() {
        let (t, b) = ray_triangle([0.25, 0.25, 1.0], [0.0, 0.0, -1.0], &T).unwrap();
        assert!((t - 1.0).abs() < 1e-6);
        assert!((b[1] - 0.25).abs() < 1e-6 && (b[2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn ray_misses_outside() {
        assert!(ray_triangle([2.0, 2.0, 1.0], [0.0, 0.0, -1.0], &T).is_none());
    }

    #[test]
    fn closest_point_projects_onto_face_and_edges() {
        let b = closest_on_triangle([0.2, 0.2, 5.0], &T);
        assert!((at(&T, b)[0] - 0.2).abs() < 1e-6);
        let b = closest_on_triangle([0.5, -3.0, 0.0], &T);
        assert!((at(&T, b)[0] - 0.5).abs() < 1e-6 && at(&T, b)[1].abs() < 1e-6);
    }

    #[test]
    fn split_children_cover_the_parent() {
        let kids = split4(&ROOT);
        let total: f32 = kids.iter().map(|k| area(&[at(&T, k[0]), at(&T, k[1]), at(&T, k[2])])).sum();
        assert!((total - area(&T)).abs() < 1e-6);
    }
}
