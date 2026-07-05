//! The deterministic histogram rule — "nice" bin widths + edges.
//!
//! The binning decision (design): histogram binning is **presentation-adjacent but
//! done here** (Rust, deterministic, part of the payload), so the viewer stays
//! render-only. The rule snaps the bin width to a 1/2/5·10ⁿ round number targeting
//! ~`target` bins, and the edges to round multiples of that width — a legible axis
//! independent of the sample count.

use crate::core::charts::Bin;

/// Bin `data` into "nice" `[lo, hi)` bins with counts. Targets ~`target` bins; the
/// last bin is closed so the maximum sample is counted. Returns empty for empty or
/// all-equal data guarded to a single unit bin.
pub fn nice_bins(data: &[f64], target: usize) -> Vec<Bin> {
    let finite: Vec<f64> = data.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return Vec::new();
    }
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in &finite {
        min = min.min(v);
        max = max.max(v);
    }
    if max <= min {
        // All samples equal (min/max are finite here) — one unit-wide bin.
        return vec![Bin {
            lo: min - 0.5,
            hi: min + 0.5,
            count: finite.len() as u32,
        }];
    }
    let target = target.max(1);
    let width = nice_width((max - min) / target as f64);
    let start = (min / width).floor() * width;
    let n = (((max - start) / width).floor() as usize) + 1;
    let mut bins: Vec<Bin> = (0..n)
        .map(|k| Bin {
            lo: start + k as f64 * width,
            hi: start + (k + 1) as f64 * width,
            count: 0,
        })
        .collect();
    let last = bins.len() - 1;
    for &v in &finite {
        let mut idx = ((v - start) / width).floor() as isize;
        if idx < 0 {
            idx = 0;
        }
        let idx = (idx as usize).min(last); // the closed final bin catches the max
        bins[idx].count += 1;
    }
    bins
}

/// Snap a raw width up to the nearest 1/2/5·10ⁿ round number.
fn nice_width(raw: f64) -> f64 {
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag; // in [1, 10)
    let snap = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    snap * mag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_width_snaps_to_1_2_5() {
        assert_eq!(nice_width(0.9), 1.0);
        assert_eq!(nice_width(1.4), 2.0);
        assert_eq!(nice_width(3.7), 5.0);
        assert_eq!(nice_width(140.0), 200.0);
    }

    #[test]
    fn bins_cover_and_count_all_samples() {
        let data: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect(); // 0..9.9
        let bins = nice_bins(&data, 10);
        assert!(!bins.is_empty());
        let total: u32 = bins.iter().map(|b| b.count).sum();
        assert_eq!(total as usize, data.len());
        // edges are round multiples of the nice width
        assert!(bins[0].lo <= 0.0 && *bins.last().map(|b| &b.hi).unwrap() >= 9.9);
    }

    #[test]
    fn equal_data_makes_one_bin() {
        let bins = nice_bins(&[5.0, 5.0, 5.0], 24);
        assert_eq!(bins.len(), 1);
        assert_eq!(bins[0].count, 3);
    }
}
