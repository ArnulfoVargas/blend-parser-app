//! sRGB / CIELAB conversions and hex parsing. Colours are sRGB in 0..1 unless noted.

use std::sync::OnceLock;

pub type Rgb = [f32; 3];
pub type Lab = [f32; 3];

#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn lab_from_linear(r: f32, g: f32, b: f32) -> Lab {
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn srgb_to_lab(c: Rgb) -> Lab {
    lab_from_linear(srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]))
}

/// Lab for an 8-bit sRGB texel, through a linearisation table.
pub fn srgb8_to_lab(c: [u8; 3]) -> Lab {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    let lut = LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)));
    lab_from_linear(lut[c[0] as usize], lut[c[1] as usize], lut[c[2] as usize])
}

#[inline]
pub fn dist2(a: Lab, b: Lab) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// CIEDE2000 colour difference, as Bambu Studio compares filament colours.
pub fn delta_e2000(lab1: Lab, lab2: Lab) -> f32 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = lab1.map(f64::from);
    let [l2, a2, b2] = lab2.map(f64::from);
    let p25 = 25f64.powi(7);
    let c_mean = ((a1 * a1 + b1 * b1).sqrt() + (a2 * a2 + b2 * b2).sqrt()) / 2.0;
    let g = 0.5 * (1.0 - (c_mean.powi(7) / (c_mean.powi(7) + p25)).sqrt());
    let (pa1, pa2) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (pc1, pc2) = ((pa1 * pa1 + b1 * b1).sqrt(), (pa2 * pa2 + b2 * b2).sqrt());
    let hue = |a: f64, b: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(a);
            if h < 0.0 {
                h + 2.0 * PI
            } else {
                h
            }
        }
    };
    let (ph1, ph2) = (hue(pa1, b1), hue(pa2, b2));
    let dl = l2 - l1;
    let dc = pc2 - pc1;
    let dh = if pc1 * pc2 == 0.0 {
        0.0
    } else {
        let mut d = ph2 - ph1;
        if d < -PI {
            d += 2.0 * PI;
        } else if d > PI {
            d -= 2.0 * PI;
        }
        2.0 * (pc1 * pc2).sqrt() * (d / 2.0).sin()
    };
    let l_mean = (l1 + l2) / 2.0;
    let c_mean2 = (pc1 + pc2) / 2.0;
    let h_sum = ph1 + ph2;
    let h_mean = if pc1 * pc2 == 0.0 {
        h_sum
    } else if (ph1 - ph2).abs() <= PI {
        h_sum / 2.0
    } else if h_sum < 2.0 * PI {
        (h_sum + 2.0 * PI) / 2.0
    } else {
        (h_sum - 2.0 * PI) / 2.0
    };
    let rad = |d: f64| d.to_radians();
    let t =
        1.0 - 0.17 * (h_mean - rad(30.0)).cos() + 0.24 * (2.0 * h_mean).cos() + 0.32 * (3.0 * h_mean + rad(6.0)).cos()
            - 0.2 * (4.0 * h_mean - rad(63.0)).cos();
    let dtheta = rad(30.0) * (-((h_mean - rad(275.0)) / rad(25.0)).powi(2)).exp();
    let rc = 2.0 * (c_mean2.powi(7) / (c_mean2.powi(7) + p25)).sqrt();
    let sl = 1.0 + 0.015 * (l_mean - 50.0).powi(2) / (20.0 + (l_mean - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_mean2;
    let sh = 1.0 + 0.015 * c_mean2 * t;
    let rt = -(2.0 * dtheta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh / sh).powi(2) + rt * (dc / sc) * (dh / sh)).sqrt() as f32
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([ch(0)?, ch(2)?, ch(4)?])
}

pub fn to_hex(c: Rgb) -> String {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", q(c[0]), q(c[1]), q(c[2]))
}

pub fn to_u8(c: Rgb) -> [u8; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c[0]), q(c[1]), q(c[2])]
}

/// A colour sample, either a texel (cheap Lab via table) or a float colour.
#[derive(Clone, Copy, Debug)]
pub enum Sample {
    Byte([u8; 3]),
    Float(Rgb),
}

impl Sample {
    pub fn lab(self) -> Lab {
        match self {
            Sample::Byte(c) => srgb8_to_lab(c),
            Sample::Float(c) => srgb_to_lab(c),
        }
    }
    pub fn rgb(self) -> Rgb {
        match self {
            Sample::Byte(c) => [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0],
            Sample::Float(c) => c,
        }
    }
}

/// Nearest-palette lookup in Lab.
#[derive(Clone, Debug)]
pub struct Classifier {
    labs: Vec<Lab>,
    slots: Vec<u8>,
}

impl Classifier {
    pub fn new(palette: &[Rgb]) -> Self {
        Self { labs: palette.iter().map(|&c| srgb_to_lab(c)).collect(), slots: (0..palette.len() as u8).collect() }
    }

    /// Several target colours may feed the same slot (after merging slots).
    pub fn with_slots(targets: &[(Rgb, u8)]) -> Self {
        Self { labs: targets.iter().map(|t| srgb_to_lab(t.0)).collect(), slots: targets.iter().map(|t| t.1).collect() }
    }
    pub fn len(&self) -> usize {
        self.labs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.labs.is_empty()
    }
    pub fn nearest(&self, lab: Lab) -> u8 {
        let mut best = (f32::MAX, 0u8);
        for (i, &p) in self.labs.iter().enumerate() {
            let d = dist2(lab, p);
            if d < best.0 {
                best = (d, i as u8);
            }
        }
        self.slots[best.1 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = parse_hex("#4D2720").unwrap();
        assert_eq!(to_hex(c), "#4D2720");
        assert!(parse_hex("4d2").is_none());
    }

    #[test]
    fn byte_and_float_lab_agree() {
        let a = srgb8_to_lab([232, 164, 97]);
        let b = srgb_to_lab([232.0 / 255.0, 164.0 / 255.0, 97.0 / 255.0]);
        assert!(dist2(a, b) < 1e-6);
    }

    #[test]
    fn ciede2000_matches_the_reference_pairs() {
        // Sharma, Wu and Dalal's test data.
        let pairs = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            ([50.0, 2.5, 0.0], [50.0, 3.2592, 0.335], 1.0),
            ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ];
        for (a, b, de) in pairs {
            assert!((delta_e2000(a, b) - de).abs() < 1e-3, "{a:?} {b:?}: {} vs {de}", delta_e2000(a, b));
        }
    }

    #[test]
    fn classifier_picks_nearest() {
        let c = Classifier::new(&[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.9, 0.6, 0.3]]);
        assert_eq!(c.nearest(srgb_to_lab([0.85, 0.62, 0.35])), 2);
        assert_eq!(c.nearest(srgb_to_lab([0.05, 0.05, 0.05])), 0);
    }
}
