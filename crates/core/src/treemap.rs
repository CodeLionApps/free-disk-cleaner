//! Squarified treemap layout (Bruls, Huizing, van Wijk 2000). Pure function, no side effects.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Lays out `sizes` inside `bounds`. Output has the same length and order as `sizes`:
/// `result[i]` belongs to `sizes[i]`. Zero sizes (and everything, if `bounds` has no area)
/// get a zero-area rect at the bounds origin.
pub fn squarify(sizes: &[u64], bounds: Rect) -> Vec<Rect> {
    let empty = Rect {
        x: bounds.x,
        y: bounds.y,
        w: 0.0,
        h: 0.0,
    };
    let mut out = vec![empty; sizes.len()];
    let total: u128 = sizes.iter().map(|&s| u128::from(s)).sum();
    if total == 0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }

    // Largest first; stable sort keeps ties in input order, so the layout is deterministic.
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0).collect();
    order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]));

    let (bw, bh) = (f64::from(bounds.w), f64::from(bounds.h));
    let scale = bw * bh / total as f64;
    let areas: Vec<f64> = order.iter().map(|&i| sizes[i] as f64 * scale).collect();

    // Free space left to fill, in f64.
    let (mut x, mut y, mut w, mut h) = (f64::from(bounds.x), f64::from(bounds.y), bw, bh);
    let mut start = 0;
    while start < areas.len() {
        let side = w.min(h);
        // Grow the row while it does not worsen the worst aspect ratio.
        let mut end = start + 1;
        let mut sum = areas[start];
        while end < areas.len() {
            let next = sum + areas[end];
            if worst(next, areas[start], areas[end], side)
                > worst(sum, areas[start], areas[end - 1], side)
            {
                break;
            }
            sum = next;
            end += 1;
        }

        // Last row takes the whole remaining space so float drift cannot leave a gap or overflow.
        let last = end == areas.len();
        let horizontal = w < h; // row runs along the shorter side
        let thickness = if last {
            if horizontal { h } else { w }
        } else {
            sum / side
        };
        let mut offset = 0.0;
        for k in start..end {
            let len = if k + 1 == end {
                side - offset
            } else {
                areas[k] / thickness
            };
            let r = if horizontal {
                Rect {
                    x: (x + offset) as f32,
                    y: y as f32,
                    w: len as f32,
                    h: thickness as f32,
                }
            } else {
                Rect {
                    x: x as f32,
                    y: (y + offset) as f32,
                    w: thickness as f32,
                    h: len as f32,
                }
            };
            out[order[k]] = r;
            offset += len;
        }
        if horizontal {
            y += thickness;
            h -= thickness;
        } else {
            x += thickness;
            w -= thickness;
        }
        start = end;
    }
    out
}

/// Worst aspect ratio of a row with total area `sum`, largest item `max`, smallest `min`,
/// laid along a side of length `side`.
fn worst(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    let (s2, side2) = (sum * sum, side * side);
    (side2 * max / s2).max(s2 / (side2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const B: Rect = Rect {
        x: 10.0,
        y: 20.0,
        w: 800.0,
        h: 500.0,
    };

    fn area(r: &Rect) -> f64 {
        f64::from(r.w) * f64::from(r.h)
    }

    /// Checks every layout invariant; returns an error message on the first violation.
    fn check(sizes: &[u64], b: Rect, rects: &[Rect]) -> Result<(), String> {
        if rects.len() != sizes.len() {
            return Err("length mismatch".into());
        }
        let total: f64 = sizes.iter().map(|&s| s as f64).sum();
        let barea = area(&b);
        let eps = 1e-3 * f64::from(b.w.max(b.h));
        for (i, r) in rects.iter().enumerate() {
            let (x, y, w, h) = (
                f64::from(r.x),
                f64::from(r.y),
                f64::from(r.w),
                f64::from(r.h),
            );
            if w < 0.0 || h < 0.0 {
                return Err(format!("rect {i} negative: {r:?}"));
            }
            if x < f64::from(b.x) - eps
                || y < f64::from(b.y) - eps
                || x + w > f64::from(b.x) + f64::from(b.w) + eps
                || y + h > f64::from(b.y) + f64::from(b.h) + eps
            {
                return Err(format!("rect {i} out of bounds: {r:?}"));
            }
            if sizes[i] == 0 {
                if area(r) != 0.0 {
                    return Err(format!("zero size {i} has area"));
                }
                continue;
            }
            let want = sizes[i] as f64 / total;
            let got = area(r) / barea;
            if (got - want).abs() > 1e-3 * want + 1e-6 {
                return Err(format!("rect {i} area ratio {got} != {want}"));
            }
        }
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, c) = (&rects[i], &rects[j]);
                let ix = (f64::from(a.x + a.w).min(f64::from(c.x + c.w)) - f64::from(a.x.max(c.x)))
                    .max(0.0);
                let iy = (f64::from(a.y + a.h).min(f64::from(c.y + c.h)) - f64::from(a.y.max(c.y)))
                    .max(0.0);
                if ix * iy > 1e-6 * barea {
                    return Err(format!("rects {i} and {j} overlap: {a:?} {c:?}"));
                }
            }
        }
        if total > 0.0 {
            let sum: f64 = rects.iter().map(area).sum();
            if (sum - barea).abs() > 1e-3 * barea {
                return Err(format!("area sum {sum} != {barea}"));
            }
        }
        Ok(())
    }

    #[test]
    fn empty_and_all_zero() {
        assert!(squarify(&[], B).is_empty());
        let r = squarify(&[0, 0], B);
        assert_eq!(
            r,
            vec![
                Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 0.0,
                    h: 0.0
                };
                2
            ]
        );
    }

    #[test]
    fn single_item_fills_bounds() {
        assert_eq!(squarify(&[42], B), vec![B]);
    }

    #[test]
    fn degenerate_bounds() {
        for b in [Rect { w: 0.0, ..B }, Rect { h: 0.0, ..B }] {
            let r = squarify(&[1, 2, 3], b);
            assert!(
                r.iter()
                    .all(|r| r.w == 0.0 && r.h == 0.0 && r.x == b.x && r.y == b.y)
            );
        }
    }

    #[test]
    fn four_equal_in_square_are_squares() {
        let b = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let r = squarify(&[5, 5, 5, 5], b);
        check(&[5, 5, 5, 5], b, &r).unwrap();
        for r in &r {
            assert!(
                (r.w - 50.0).abs() < 1e-3 && (r.h - 50.0).abs() < 1e-3,
                "{r:?}"
            );
        }
    }

    #[test]
    fn paper_example() {
        let sizes = [6, 6, 4, 3, 2, 2, 1];
        let b = Rect {
            x: 0.0,
            y: 0.0,
            w: 6.0,
            h: 4.0,
        };
        let r = squarify(&sizes, b);
        check(&sizes, b, &r).unwrap();
        // First row of Bruls et al.: the two 6s stacked in a 3-wide column.
        assert!((r[0].w - 3.0).abs() < 1e-4 && (r[1].w - 3.0).abs() < 1e-4);
    }

    #[test]
    fn zeros_mixed_in() {
        let sizes = [0, 10, 0, 5];
        let r = squarify(&sizes, B);
        check(&sizes, B, &r).unwrap();
        assert_eq!(
            r[0],
            Rect {
                x: B.x,
                y: B.y,
                w: 0.0,
                h: 0.0
            }
        );
    }

    fn arb_bounds() -> impl Strategy<Value = Rect> {
        (
            -1000.0f32..1000.0,
            -1000.0f32..1000.0,
            1.0f32..2000.0,
            1.0f32..2000.0,
        )
            .prop_map(|(x, y, w, h)| Rect { x, y, w, h })
    }

    fn arb_sizes() -> impl Strategy<Value = Vec<u64>> {
        prop::collection::vec(prop_oneof![1 => Just(0u64), 9 => 1u64..1_000_000], 0..40)
    }

    proptest! {
        #[test]
        fn layout_invariants(sizes in arb_sizes(), b in arb_bounds()) {
            let r = squarify(&sizes, b);
            prop_assert_eq!(check(&sizes, b, &r), Ok(()));
            prop_assert_eq!(&r, &squarify(&sizes, b), "not deterministic");
        }

        #[test]
        fn permuting_input_permutes_output(sizes in arb_sizes(), b in arb_bounds(), seed in any::<u64>()) {
            // Distinct sizes so the sort order (and therefore the layout) is permutation-independent.
            let sizes: Vec<u64> = sizes.iter().enumerate().map(|(i, s)| s * 64 + i as u64 + 1).collect();
            let mut perm: Vec<usize> = (0..sizes.len()).collect();
            let mut s = seed;
            for i in (1..perm.len()).rev() {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                perm.swap(i, (s >> 33) as usize % (i + 1));
            }
            let permuted: Vec<u64> = perm.iter().map(|&i| sizes[i]).collect();
            let r = squarify(&sizes, b);
            let rp = squarify(&permuted, b);
            for (k, &i) in perm.iter().enumerate() {
                prop_assert_eq!(rp[k], r[i]);
            }
        }
    }
}
