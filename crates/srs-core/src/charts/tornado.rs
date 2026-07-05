//! Build a [`TornadoChart`](crate::charts::TornadoChart) from the ranked
//! [`TornadoBar`]s the uncertainty run already produced. Render-only: the swings
//! come straight from `tornado()`; here we only convert the output metric to the
//! display unit and label the pivots.

use crate::charts::{TornadoBarOut, TornadoChart};
use srs_model::TornadoBar;

/// Map the `bars` (oil-in-place swing per input, `Sm³`) onto a tornado payload in
/// display units (`base`/`scale` share those units — e.g. `SM3_PER_MSM3`, `"MSm³"`).
///
/// The pivots (`lo_val`/`hi_val`) are input values in their own units and are **not**
/// scaled. `out_min`/`out_max` (the faint outer full-span band) are left `None`: the
/// tornado swings between the `lo_pct`/`hi_pct` pivots only, so this is the inner
/// (P90→P10) band — the payload omits the outer span rather than fake it.
pub fn tornado_chart(
    bars: &[TornadoBar],
    base: f64,
    scale: f64,
    units: &str,
    fold_count: Option<usize>,
) -> TornadoChart {
    let s = if scale != 0.0 { scale } else { 1.0 };
    let bars_out = bars
        .iter()
        .map(|b| TornadoBarOut {
            param: b.input.clone(),
            display_name: display_param(&b.input),
            in_lo: Some(b.lo_val),
            in_hi: Some(b.hi_val),
            out_lo: b.out_lo / s,
            out_hi: b.out_hi / s,
            out_min: None,
            out_max: None,
            swing: b.swing / s,
        })
        .collect();
    TornadoChart {
        mark: "tornado",
        title: format!("STOIIP tornado ({units})"),
        units: units.to_string(),
        base,
        bars: bars_out,
        fold_count,
    }
}

/// Map a raw MC input dimension name onto a human display label that disambiguates
/// the two families the tornado can mix: the **box/PVT draws** (engine scalars,
/// fixed names) and the **property level shifts** (named after the cube they shift,
/// e.g. `PORO`/`NTG`/`SW`/`PHIE`). Any name outside the known engine-scalar set is
/// a property level shift. Returns `None` when the label adds nothing over `param`.
fn display_param(raw: &str) -> Option<String> {
    let s = match raw {
        "area_m2" => "area",
        "gross_height_m" => "gross height",
        "contact_depth_m" => "contact depth",
        "goc_depth_m" | "goc" => "GOC depth",
        "porosity" => "porosity (draw)",
        "net_to_gross" => "net-to-gross (draw)",
        "water_saturation" => "water saturation (draw)",
        "boi" => "Bo",
        "bgi" => "Bg",
        other => return Some(format!("{other} level shift")),
    };
    Some(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(input: &str, lo: f64, hi: f64, out_lo: f64, out_hi: f64) -> TornadoBar {
        TornadoBar {
            input: input.into(),
            lo_val: lo,
            hi_val: hi,
            out_lo,
            out_hi,
            swing: (out_hi - out_lo).abs(),
        }
    }

    #[test]
    fn scales_outputs_but_not_pivots() {
        let bars = vec![bar("net_to_gross", 0.6, 0.9, 30e6, 50e6)];
        let ch = tornado_chart(&bars, 40.0, 1e6, "MSm³", Some(8));
        assert_eq!(ch.mark, "tornado");
        assert_eq!(ch.base, 40.0);
        let b = &ch.bars[0];
        assert_eq!(b.in_lo, Some(0.6)); // pivot input untouched
        assert!((b.out_lo - 30.0).abs() < 1e-9); // Sm³ -> MSm³
        assert!((b.out_hi - 50.0).abs() < 1e-9);
        assert!((b.swing - 20.0).abs() < 1e-9);
        assert!(b.out_min.is_none()); // inner-only
    }

    #[test]
    fn disambiguates_draw_from_level_shift() {
        // A box draw named `porosity` and a property level shift named `PORO` must
        // read distinctly (the W6 naming collision).
        let bars = vec![
            bar("porosity", 0.2, 0.3, 30e6, 50e6),
            bar("PORO", 0.2, 0.3, 30e6, 50e6),
        ];
        let ch = tornado_chart(&bars, 40.0, 1e6, "MSm³", Some(8));
        assert_eq!(ch.bars[0].display_name.as_deref(), Some("porosity (draw)"));
        assert_eq!(ch.bars[1].display_name.as_deref(), Some("PORO level shift"));
    }
}
