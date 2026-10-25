//! `paint_color` serialisation, bit for bit as Bambu Studio / PrusaSlicer's
//! `TriangleSelector::serialize` writes it into a 3MF.
//!
//! Per node: 2 bits for the number of split sides (0 = leaf). A split node then
//! stores 2 bits of special side and its children in reverse order. A leaf stores
//! its state in 2 bits, or `11` plus 4 bits of `state - 3` for states 3 and up.
//! The bit stream is packed into nibbles and written as hex, last nibble first.
//! State 0 means "not painted"; filament slot `n` (0-based) is state `n + 1`.

use crate::paint::Node;

fn push_node(n: &Node, bits: &mut Vec<bool>) {
    match n {
        Node::Leaf(slot) => {
            let state = *slot as u32 + 1;
            bits.extend([false, false]);
            if state < 3 {
                bits.extend([state & 1 != 0, state & 2 != 0]);
            } else {
                bits.extend([true, true]);
                let s = state - 3;
                bits.extend((0..4).map(|i| s >> i & 1 != 0));
            }
        }
        Node::Split(kids) => {
            // Three sides split, special side 0.
            bits.extend([true, true, false, false]);
            for k in kids.iter().rev() {
                push_node(k, bits);
            }
        }
    }
}

pub fn encode(n: &Node) -> String {
    let mut bits = Vec::with_capacity(8);
    push_node(n, &mut bits);
    while bits.len() % 4 != 0 {
        bits.push(false);
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let nibbles: Vec<u8> = bits
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| HEX[(c[0] as usize) | (c[1] as usize) << 1 | (c[2] as usize) << 2 | (c[3] as usize) << 3])
        .collect();
    nibbles.iter().rev().map(|&c| c as char).collect()
}

#[derive(Debug, PartialEq)]
pub struct DecodeError;

/// Inverse of [`encode`] for the subset it writes (leaves and 3-side splits).
pub fn decode(s: &str) -> Result<Node, DecodeError> {
    let mut bits = Vec::new();
    for c in s.chars().rev() {
        let v = c.to_digit(16).ok_or(DecodeError)?;
        bits.extend((0..4).map(|i| v >> i & 1 != 0));
    }
    let mut pos = 0;
    let n = read(&bits, &mut pos)?;
    if bits.len() - pos >= 4 {
        return Err(DecodeError);
    }
    Ok(n)
}

fn take(bits: &[bool], pos: &mut usize, k: usize) -> Result<u32, DecodeError> {
    if *pos + k > bits.len() {
        return Err(DecodeError);
    }
    let v = (0..k).fold(0u32, |a, i| a | (bits[*pos + i] as u32) << i);
    *pos += k;
    Ok(v)
}

fn read(bits: &[bool], pos: &mut usize) -> Result<Node, DecodeError> {
    match take(bits, pos, 2)? {
        0 => {
            let mut state = take(bits, pos, 2)?;
            if state == 3 {
                state = 3 + take(bits, pos, 4)?;
            }
            if state == 0 {
                return Err(DecodeError);
            }
            Ok(Node::Leaf((state - 1) as u8))
        }
        3 => {
            take(bits, pos, 2)?;
            let mut kids = [read(bits, pos)?, read(bits, pos)?, read(bits, pos)?, read(bits, pos)?];
            kids.reverse();
            Ok(Node::Split(Box::new(kids)))
        }
        _ => Err(DecodeError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_match_bambu() {
        // Values read from a 3MF saved by Bambu Studio 2.3.
        assert_eq!(encode(&Node::Leaf(0)), "4");
        assert_eq!(encode(&Node::Leaf(1)), "8");
        assert_eq!(encode(&Node::Leaf(2)), "0C");
        assert_eq!(encode(&Node::Leaf(3)), "1C");
    }

    #[test]
    fn decodes_a_bambu_split() {
        // A corner-split triangle from the same file.
        let s = "40C0C0C30C0C0C30C0C0C3";
        let n = decode(s).unwrap();
        assert!(n.is_split());
        assert_eq!(encode(&n), s);
    }

    #[test]
    fn roundtrip_nested() {
        let n = Node::Split(Box::new([
            Node::Leaf(4),
            Node::Split(Box::new([Node::Leaf(0), Node::Leaf(1), Node::Leaf(2), Node::Leaf(3)])),
            Node::Leaf(15),
            Node::Leaf(0),
        ]));
        assert_eq!(decode(&encode(&n)).unwrap(), n);
    }
}
