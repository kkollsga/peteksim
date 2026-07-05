//! Build a [`DistributionPanel`](crate::charts::DistributionPanel) from kept
//! realization vectors: a "nice"-binned histogram + an exceedance CDF (`% ≥ x`, the
//! reservoir sense) + P90/P50/P10 markers, per series, in display units.

use crate::charts::bins::nice_bins;
use crate::charts::{CdfPoint, DistSeries, DistributionPanel, Markers};

/// P-curve markers for a series, in the run's native `Sm³` (scaled to display units
/// alongside the samples). P90 is the low case, P10 the high (reservoir convention).
#[derive(Debug, Clone, Copy)]
pub struct DistMarkers {
    pub p90: f64,
    pub p50: f64,
    pub p10: f64,
}

/// Assemble a distribution panel. Each `(name, samples_sm3, markers)` becomes one
/// overlaid series; `scale`/`units` convert to the display unit (e.g. `SM3_PER_MSM3`,
/// `"MSm³"` for oil; `SM3_PER_BCM`, `"bcm"` for gas). `target_bins` sets the nice-bin
/// target (~24 reads well). Empty-sample series are skipped.
pub fn distribution_panel(
    title: &str,
    units: &str,
    series: &[(String, &[f64], DistMarkers)],
    scale: f64,
    target_bins: usize,
) -> DistributionPanel {
    let s = if scale != 0.0 { scale } else { 1.0 };
    let out = series
        .iter()
        .filter(|(_, samples, _)| !samples.is_empty())
        .map(|(name, samples, mk)| {
            let scaled: Vec<f64> = samples.iter().map(|v| v / s).collect();
            let bins = nice_bins(&scaled, target_bins);
            let cdf = exceedance(&scaled, &bins);
            DistSeries {
                name: name.clone(),
                bins,
                cdf,
                markers: Markers {
                    p90: mk.p90 / s,
                    p50: mk.p50 / s,
                    p10: mk.p10 / s,
                },
            }
        })
        .collect();
    DistributionPanel {
        mark: "distribution",
        title: title.to_string(),
        units: units.to_string(),
        series: out,
    }
}

/// The exceedance curve at each bin edge: `exceedance(x) = fraction of samples ≥ x`.
/// Monotone non-increasing from 1 at the first edge toward 0 at the last — the
/// petroleum P-curve sense (P90 sits high on the curve, P10 low).
fn exceedance(scaled: &[f64], bins: &[crate::charts::Bin]) -> Vec<CdfPoint> {
    if bins.is_empty() || scaled.is_empty() {
        return Vec::new();
    }
    let n = scaled.len() as f64;
    let mut edges: Vec<f64> = bins.iter().map(|b| b.lo).collect();
    edges.push(bins.last().unwrap().hi);
    edges
        .into_iter()
        .map(|x| {
            let ge = scaled.iter().filter(|&&v| v >= x).count() as f64;
            CdfPoint {
                x,
                exceedance: ge / n,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_bins_scales_and_orders_exceedance() {
        let samples: Vec<f64> = (0..1000).map(|i| 30e6 + (i as f64) * 4e4).collect(); // Sm³
        let mk = DistMarkers {
            p90: 34e6,
            p50: 50e6,
            p10: 66e6,
        };
        let panel = distribution_panel(
            "STOIIP",
            "MSm³",
            &[("Field".to_string(), &samples, mk)],
            1e6,
            24,
        );
        assert_eq!(panel.series.len(), 1);
        let s = &panel.series[0];
        // scaled to MSm³
        assert!((s.markers.p90 - 34.0).abs() < 1e-9);
        assert!(s.markers.p90 <= s.markers.p50 && s.markers.p50 <= s.markers.p10);
        // bins count every sample; exceedance starts at 1 and is non-increasing
        let total: u32 = s.bins.iter().map(|b| b.count).sum();
        assert_eq!(total as usize, samples.len());
        assert!((s.cdf.first().unwrap().exceedance - 1.0).abs() < 1e-9);
        for w in s.cdf.windows(2) {
            assert!(w[1].exceedance <= w[0].exceedance + 1e-12);
        }
    }

    #[test]
    fn empty_series_skipped() {
        let empty: Vec<f64> = Vec::new();
        let mk = DistMarkers {
            p90: 0.0,
            p50: 0.0,
            p10: 0.0,
        };
        let panel = distribution_panel("GIIP", "bcm", &[("Gas".to_string(), &empty, mk)], 1e9, 24);
        assert!(panel.series.is_empty());
    }
}
