//! Splitting a model that is too large for the printer: how many cuts each
//! axis needs so every piece fits the room a plate has, and how the pieces
//! share out across plates. The model's scale is never touched here.

/// The room one plate has for pieces, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// Width (X) and depth (Y) clear of margins, excluded zones and the tower's reserve.
    pub area_mm: [f32; 2],
    pub height_mm: f32,
    /// Space left between pieces on a plate.
    pub gap_mm: f32,
}

impl Room {
    pub fn valid(&self) -> bool {
        self.area_mm.iter().chain([&self.height_mm]).all(|v| v.is_finite() && *v >= 5.0)
            && (0.0..=50.0).contains(&self.gap_mm)
    }
}

/// Pieces are made a hair smaller than the room, so rounding never pushes one over.
const SAFETY: f32 = 0.995;

/// Cuts per axis so every cell fits the room standing as the model stands: X
/// and Y may swap their limits (pieces can turn a quarter on the plate),
/// whichever gives fewer pieces.
pub fn cuts_needed(size_mm: [f32; 3], room: &Room) -> [usize; 3] {
    let n = |size: f32, limit: f32| ((size / (limit * SAFETY)).ceil() as usize).max(1);
    let [w, d] = room.area_mm;
    let straight = (n(size_mm[0], w), n(size_mm[1], d));
    let turned = (n(size_mm[0], d), n(size_mm[1], w));
    let (x, y) = if turned.0 * turned.1 < straight.0 * straight.1 { turned } else { straight };
    [x - 1, y - 1, n(size_mm[2], room.height_mm) - 1]
}

/// `count` cuts spreading `lo..hi` into equal cells.
pub fn even_cuts(lo: f32, hi: f32, count: usize) -> Vec<f32> {
    (1..=count).map(|i| lo + (hi - lo) * i as f32 / (count + 1) as f32).collect()
}

/// Where a footprint goes: its plate, the corner nearest the room's origin,
/// and whether it turned a quarter to fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub plate: usize,
    pub at: [f32; 2],
    pub turned: bool,
}

/// Shares footprints (width, depth in mm) out across as few plates as it can,
/// in rows, `gap_mm` apart. Footprints that fit no plate in either turn come
/// back as `None`.
pub fn pack(footprints: &[[f32; 2]], room: &Room) -> Vec<Option<Spot>> {
    let [w, d] = room.area_mm;
    let g = room.gap_mm;
    let fits = |f: [f32; 2]| f[0] <= w + 1e-3 && f[1] <= d + 1e-3;
    // Rows hold the deepest first: each footprint lies with its long side along X when it can.
    let laid: Vec<Option<([f32; 2], bool)>> = footprints
        .iter()
        .map(|&f| {
            let turned = [f[1], f[0]];
            let (long, short) = if f[0] >= f[1] { (f, false) } else { (turned, true) };
            if fits(long) {
                Some((long, short))
            } else if fits([long[1], long[0]]) {
                Some(([long[1], long[0]], !short))
            } else {
                None
            }
        })
        .collect();
    let mut order: Vec<usize> = (0..footprints.len()).filter(|&i| laid[i].is_some()).collect();
    order.sort_by(|&a, &b| laid[b].unwrap().0[1].total_cmp(&laid[a].unwrap().0[1]).then(a.cmp(&b)));
    // Per plate: its rows as (y, depth, x used) and the depth used.
    type Row = (f32, f32, f32);
    let mut plates: Vec<(Vec<Row>, f32)> = vec![];
    let mut out = vec![None; footprints.len()];
    for i in order {
        let (f, turned) = laid[i].unwrap();
        let mut placed = None;
        'plates: for (p, (rows, used)) in plates.iter_mut().enumerate() {
            for row in rows.iter_mut() {
                let x = if row.2 > 0.0 { row.2 + g } else { 0.0 };
                if x + f[0] <= w + 1e-3 && f[1] <= row.1 + 1e-3 {
                    row.2 = x + f[0];
                    placed = Some(Spot { plate: p, at: [x, row.0], turned });
                    break 'plates;
                }
            }
            let y = if *used > 0.0 { *used + g } else { 0.0 };
            if y + f[1] <= d + 1e-3 {
                rows.push((y, f[1], f[0]));
                *used = y + f[1];
                placed = Some(Spot { plate: p, at: [0.0, y], turned });
                break;
            }
        }
        if placed.is_none() {
            plates.push((vec![(0.0, f[1], f[0])], f[1]));
            placed = Some(Spot { plate: plates.len() - 1, at: [0.0, 0.0], turned });
        }
        out[i] = placed;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Room = Room { area_mm: [200.0, 150.0], height_mm: 180.0, gap_mm: 5.0 };

    #[test]
    fn cuts_follow_the_room_and_a_model_that_fits_needs_none() {
        assert_eq!(cuts_needed([180.0, 140.0, 170.0], &ROOM), [0, 0, 0]);
        // Too wide, too deep and too tall each need their own cuts.
        assert_eq!(cuts_needed([450.0, 140.0, 100.0], &ROOM), [2, 0, 0]);
        assert_eq!(cuts_needed([100.0, 100.0, 400.0], &ROOM), [0, 0, 2]);
        // 320 deep: three pieces 150 deep, or two turned to lie 200 along X.
        assert_eq!(cuts_needed([100.0, 320.0, 100.0], &ROOM), [0, 1, 0]);
        assert_eq!(cuts_needed([140.0, 290.0, 50.0], &ROOM), [0, 1, 0]);
        assert_eq!(cuts_needed([290.0, 190.0, 50.0], &ROOM), [1, 0, 0]);
        assert_eq!(even_cuts(0.0, 30.0, 2), [10.0, 20.0]);
    }

    #[test]
    fn packing_fills_plates_in_rows_and_turns_pieces_to_fit() {
        let spots =
            pack(&[[90.0, 70.0], [90.0, 70.0], [90.0, 70.0], [140.0, 190.0], [10.0, 10.0], [300.0, 10.0]], &ROOM);
        // The tall one only fits turned, alone on its plate's first row.
        let tall = spots[3].unwrap();
        assert!(tall.turned);
        assert_eq!(spots[5], None);
        // The three 90 × 70: two side by side in a row, 5 mm apart.
        let mut same: Vec<Spot> = spots[..3].iter().map(|s| s.unwrap()).collect();
        same.sort_by(|a, b| (a.plate, a.at[1], a.at[0]).partial_cmp(&(b.plate, b.at[1], b.at[0])).unwrap());
        assert_eq!(same[1].at[0] - same[0].at[0], 95.0);
        // Nothing overlaps on any plate, and every piece stays inside the room.
        let size = |i: usize, s: Spot| {
            let f = [[90.0, 70.0], [90.0, 70.0], [90.0, 70.0], [140.0, 190.0], [10.0, 10.0]][i];
            if s.turned {
                [f[1], f[0]]
            } else {
                f
            }
        };
        let placed: Vec<(usize, Spot)> = (0..5).map(|i| (i, spots[i].unwrap())).collect();
        for &(i, a) in &placed {
            let sa = size(i, a);
            assert!(a.at[0] + sa[0] <= 200.0 + 1e-3 && a.at[1] + sa[1] <= 150.0 + 1e-3, "{i} {a:?}");
            for &(j, b) in &placed {
                let sb = size(j, b);
                if i < j && a.plate == b.plate {
                    let apart = a.at[0] + sa[0] + 5.0 <= b.at[0] + 1e-3
                        || b.at[0] + sb[0] + 5.0 <= a.at[0] + 1e-3
                        || a.at[1] + sa[1] + 5.0 <= b.at[1] + 1e-3
                        || b.at[1] + sb[1] + 5.0 <= a.at[1] + 1e-3;
                    assert!(apart, "{i} {a:?} {j} {b:?}");
                }
            }
        }
        assert_eq!(placed.iter().map(|p| p.1.plate).max(), Some(1));
    }
}
