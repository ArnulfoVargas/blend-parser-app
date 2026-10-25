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
    fn classifier_picks_nearest() {
        let c = Classifier::new(&[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.9, 0.6, 0.3]]);
        assert_eq!(c.nearest(srgb_to_lab([0.85, 0.62, 0.35])), 2);
        assert_eq!(c.nearest(srgb_to_lab([0.05, 0.05, 0.05])), 0);
    }
}
