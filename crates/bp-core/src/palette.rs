//! Picks N filament colours from the colours of the model: weighted k-means++ in Lab.

use crate::color::{dist2, srgb_to_lab, Lab, Rgb};
use crate::field::ColorField;
use crate::geom;
use crate::mesh::Mesh;

/// SplitMix64, enough randomness for sampling without a dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Index drawn with probability proportional to `w`.
    pub fn weighted(&mut self, cumulative: &[f32]) -> usize {
        let total = *cumulative.last().unwrap_or(&0.0);
        let r = self.f32() * total;
        cumulative.partition_point(|&c| c <= r).min(cumulative.len().saturating_sub(1))
    }
}

/// Colours at `n` points spread over the surface by area.
pub fn surface_samples(field: &ColorField, shape: &Mesh, n: usize, seed: u64) -> Vec<Rgb> {
    let mut owners = Vec::new();
    let mut cumulative = Vec::new();
    let mut acc = 0.0f32;
    for (pi, part) in shape.parts.iter().enumerate() {
        for t in 0..part.tris.len() {
            acc += geom::area(&part.corners(t));
            cumulative.push(acc);
            owners.push((pi, t));
        }
    }
    if acc <= 0.0 {
        return vec![];
    }
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let (pi, t) = owners[rng.weighted(&cumulative)];
            let (mut u, mut v) = (rng.f32(), rng.f32());
            if u + v > 1.0 {
                (u, v) = (1.0 - u, 1.0 - v);
            }
            field.sample(shape, pi, t, [1.0 - u - v, u, v]).rgb()
        })
        .collect()
}

/// K-means++ on colour bins weighted by sqrt(area), so a small but distinct
/// detail (eyes, lines) earns its own filament instead of the dominant colour
/// being split into two near-identical shades. Sorted by area, largest first.
pub fn auto_palette(samples: &[Rgb], k: usize) -> Vec<Rgb> {
    let k = k.max(1);
    // Bin to 32 levels per channel.
    let mut bins: std::collections::HashMap<u32, ([f32; 3], u32)> = std::collections::HashMap::new();
    for c in samples {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 31.0).round() as u32;
        let e = bins.entry(q(c[0]) << 10 | q(c[1]) << 5 | q(c[2])).or_insert(([0.0; 3], 0));
        for (acc, v) in e.0.iter_mut().zip(c) {
            *acc += v;
        }
        e.1 += 1;
    }
    let mut bins: Vec<(Rgb, u32)> = bins.into_values().map(|(s, n)| (s.map(|v| v / n as f32), n)).collect();
    bins.sort_by(|a, b| b.1.cmp(&a.1).then(a.0[0].total_cmp(&b.0[0])));
    if bins.len() <= k {
        return bins.into_iter().map(|b| b.0).collect();
    }
    let floor = 3.0f32.max(samples.len() as f32 * 1e-4);
    let labs: Vec<Lab> = bins.iter().map(|b| srgb_to_lab(b.0)).collect();
    let w: Vec<f32> = bins.iter().map(|b| if (b.1 as f32) < floor { 0.0 } else { (b.1 as f32).sqrt() }).collect();
    if w.iter().all(|&x| x == 0.0) {
        return bins.into_iter().take(k).map(|b| b.0).collect();
    }

    let mut rng = Rng::new(7);
    let mut best: Option<(f32, Vec<usize>)> = None;
    for _attempt in 0..8 {
        let cum = |v: &[f32]| {
            v.iter()
                .scan(0.0, |a, &x| {
                    *a += x;
                    Some(*a)
                })
                .collect::<Vec<f32>>()
        };
        let mut centers = vec![labs[rng.weighted(&cum(&w))]];
        while centers.len() < k {
            let d: Vec<f32> = labs
                .iter()
                .zip(&w)
                .map(|(l, &wi)| centers.iter().map(|c| dist2(*l, *c)).fold(f32::MAX, f32::min) * wi)
                .collect();
            if d.iter().sum::<f32>() <= 0.0 {
                break;
            }
            centers.push(labs[rng.weighted(&cum(&d))]);
        }
        let mut assign = vec![0usize; labs.len()];
        for _ in 0..50 {
            for (i, l) in labs.iter().enumerate() {
                assign[i] = nearest(&centers, *l);
            }
            let mut moved = false;
            for (c, center) in centers.iter_mut().enumerate() {
                let (mut s, mut ws) = ([0.0f32; 3], 0.0f32);
                for i in (0..labs.len()).filter(|&i| assign[i] == c) {
                    for j in 0..3 {
                        s[j] += labs[i][j] * w[i];
                    }
                    ws += w[i];
                }
                if ws > 0.0 {
                    let n = s.map(|v| v / ws);
                    moved |= dist2(n, *center) > 1e-6;
                    *center = n;
                }
            }
            if !moved {
                break;
            }
        }
        let err: f32 = labs.iter().enumerate().map(|(i, l)| dist2(*l, centers[assign[i]]) * w[i]).sum();
        if best.as_ref().is_none_or(|b| err < b.0) {
            best = Some((err, assign));
        }
    }
    let assign = best.map(|b| b.1).unwrap_or_default();
    // Each filament is the area-weighted mean sRGB of its cluster.
    let mut out: Vec<(Rgb, u32)> = (0..k)
        .filter_map(|c| {
            let members: Vec<usize> = (0..bins.len()).filter(|&i| assign[i] == c && w[i] > 0.0).collect();
            let n: u32 = members.iter().map(|&i| bins[i].1).sum();
            (n > 0).then(|| {
                let mut s = [0.0f32; 3];
                for &i in &members {
                    for (acc, v) in s.iter_mut().zip(bins[i].0) {
                        *acc += v * bins[i].1 as f32;
                    }
                }
                (s.map(|v| v / n as f32), n)
            })
        })
        .collect();
    out.sort_by_key(|o| std::cmp::Reverse(o.1));
    out.into_iter().map(|o| o.0).collect()
}

/// CIE76 colour difference (ΔE) between two sRGB colours.
pub fn delta_e(a: Rgb, b: Rgb) -> f32 {
    dist2(srgb_to_lab(a), srgb_to_lab(b)).sqrt()
}

/// For each colour, the closest other colour and their ΔE.
pub fn nearest_others(palette: &[Rgb]) -> Vec<Option<(usize, f32)>> {
    (0..palette.len())
        .map(|i| {
            (0..palette.len())
                .filter(|&j| j != i)
                .map(|j| (j, delta_e(palette[i], palette[j])))
                .min_by(|a, b| a.1.total_cmp(&b.1))
        })
        .collect()
}

/// Agglomerative reduction: while there are more than `target` colours, the two
/// closest (ΔE) are merged. The merged slot keeps the colour of whichever had
/// more triangles, because a filament is a real spool, not an average.
/// Returns the new palette and the old-slot -> new-slot map.
pub fn reduce(palette: &[Rgb], weights: &[usize], target: usize) -> (Vec<Rgb>, Vec<u8>) {
    let target = target.max(1);
    // Each cluster: (colour, weight, members).
    let mut clusters: Vec<(Rgb, usize, Vec<usize>)> =
        palette.iter().enumerate().map(|(i, &c)| (c, weights.get(i).copied().unwrap_or(0), vec![i])).collect();
    while clusters.len() > target {
        let mut best = (f32::MAX, 0, 1);
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                let d = delta_e(clusters[i].0, clusters[j].0);
                if d < best.0 {
                    best = (d, i, j);
                }
            }
        }
        let (_, i, j) = best;
        let b = clusters.remove(j);
        let a = &mut clusters[i];
        if b.1 > a.1 {
            a.0 = b.0;
        }
        a.1 += b.1;
        a.2.extend(b.2);
    }
    let mut map = vec![0u8; palette.len()];
    for (new, c) in clusters.iter().enumerate() {
        for &old in &c.2 {
            map[old] = new as u8;
        }
    }
    (clusters.into_iter().map(|c| c.0).collect(), map)
}

/// Merges slot `from` into slot `into`. Returns the new palette and the slot map.
pub fn merge(palette: &[Rgb], from: usize, into: usize) -> (Vec<Rgb>, Vec<u8>) {
    let mut map = Vec::with_capacity(palette.len());
    let mut out = Vec::with_capacity(palette.len().saturating_sub(1));
    let mut new_index = vec![0u8; palette.len()];
    for (i, &c) in palette.iter().enumerate() {
        if i != from {
            new_index[i] = out.len() as u8;
            out.push(c);
        }
    }
    for i in 0..palette.len() {
        map.push(if i == from { new_index[into] } else { new_index[i] });
    }
    (out, map)
}

fn nearest(centers: &[Lab], l: Lab) -> usize {
    let mut best = (f32::MAX, 0);
    for (i, c) in centers.iter().enumerate() {
        let d = dist2(l, *c);
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_small_distinct_colour() {
        // 70% brown, 25% dark, 4% orange in two shades, 1% white.
        let mut s = vec![];
        s.extend(std::iter::repeat_n([0.30, 0.15, 0.12], 7000));
        s.extend(std::iter::repeat_n([0.33, 0.17, 0.13], 1000));
        s.extend(std::iter::repeat_n([0.13, 0.13, 0.13], 2500));
        s.extend(std::iter::repeat_n([0.91, 0.64, 0.38], 400));
        s.extend(std::iter::repeat_n([0.90, 0.90, 0.88], 100));
        let p = auto_palette(&s, 4);
        assert_eq!(p.len(), 4);
        assert!(p.iter().any(|c| c[0] > 0.85 && c[2] > 0.8), "white lost: {p:?}");
        assert!(p[0][0] > 0.25 && p[0][0] < 0.35, "dominant first: {p:?}");
    }

    #[test]
    fn reduce_merges_the_closest_pair_first() {
        let pal = [[0.9, 0.1, 0.1], [0.85, 0.12, 0.1], [0.1, 0.1, 0.9], [0.95, 0.95, 0.95]];
        let (out, map) = reduce(&pal, &[10, 50, 30, 5], 3);
        assert_eq!(out.len(), 3);
        // The two reds merge and keep the heavier one's colour.
        assert_eq!(map[0], map[1]);
        assert_eq!(out[map[0] as usize], pal[1]);
        assert_ne!(map[2], map[3]);
    }

    #[test]
    fn manual_merge_removes_the_slot() {
        let pal = [[0.0; 3], [0.5; 3], [1.0; 3]];
        let (out, map) = merge(&pal, 0, 2);
        assert_eq!(out, vec![[0.5; 3], [1.0; 3]]);
        assert_eq!(map, vec![1, 0, 1]);
    }

    #[test]
    fn fewer_colours_than_k() {
        let s = vec![[1.0, 0.0, 0.0]; 50];
        assert_eq!(auto_palette(&s, 4).len(), 1);
    }
}
