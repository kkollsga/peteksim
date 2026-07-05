//! Analytic box in-place — the single source of truth for the MVP numbers.
//!
//! For a box with constant properties and a hard contact, hydrocarbon volume is
//! closed-form, so Monte Carlo is fast and the contact resolves sub-cell:
//! `GRV = area * height * above_contact_fraction`,
//! `HCPV = GRV * NTG * phi * (1 - Sw)`,
//! `STOIIP = HCPV / Boi`  or  `GIIP = HCPV / Bgi`  (all volumes m³; FVF in
//! Rm³/Sm³, so in-place comes out directly in Sm³ — SI standard,
//! `decision_si_units_standard`).
//!
//! `grv_from_grid` (srs-volumetrics) remains the general engine for the
//! structured/faulted grid; a test cross-checks the two agree for the box.

use crate::units::SrsError;
use petekstatic::volumetrics::{validate_fraction, validate_positive, GasFvf, OilFvf};

/// Which fluid the column holds (selects the FVF conversion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fluid {
    /// Oil — FVF is `Boi` \[Rm³/Sm³\], in-place in Sm³.
    Oil,
    /// Gas — FVF is `Bgi` \[Rm³/Sm³\], in-place in Sm³.
    Gas,
}

impl Fluid {
    /// The lower-case label (`"oil"` / `"gas"`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Fluid::Oil => "oil",
            Fluid::Gas => "gas",
        }
    }

    /// Parse `"oil"`/`"gas"` (case-insensitive).
    ///
    /// # Errors
    /// [`SrsError::InvalidInput`] for any other string.
    pub fn parse(s: &str) -> Result<Self, SrsError> {
        match s.to_ascii_lowercase().as_str() {
            "oil" => Ok(Fluid::Oil),
            "gas" => Ok(Fluid::Gas),
            other => Err(SrsError::InvalidInput(format!(
                "fluid must be 'oil' or 'gas', got '{other}'"
            ))),
        }
    }
}

/// One realization's scalar inputs (already drawn / chosen). SI units: area m²,
/// lengths m (depth positive down).
#[derive(Debug, Clone, Copy)]
pub struct BoxDraw {
    pub area_m2: f64,
    pub gross_height_m: f64,
    pub top_depth_m: f64,
    pub contact_depth_m: f64,
    pub porosity: f64,
    pub net_to_gross: f64,
    pub water_saturation: f64,
    /// Boi (oil) or Bgi (gas) \[Rm³/Sm³\].
    pub fvf: f64,
}

impl BoxDraw {
    /// Fraction of the gross column above the contact, in `[0, 1]`.
    #[must_use]
    pub fn above_contact_fraction(&self) -> f64 {
        if self.gross_height_m <= 0.0 {
            return 0.0;
        }
        ((self.contact_depth_m - self.top_depth_m) / self.gross_height_m).clamp(0.0, 1.0)
    }

    /// Gross rock volume of the hydrocarbon column \[m³\].
    #[must_use]
    pub fn grv_m3(&self) -> f64 {
        self.area_m2 * self.gross_height_m * self.above_contact_fraction()
    }

    /// Hydrocarbon pore volume \[m³\].
    #[must_use]
    pub fn hcpv_m3(&self) -> f64 {
        self.grv_m3() * self.net_to_gross * self.porosity * (1.0 - self.water_saturation)
    }

    /// Reject a non-physical draw with a typed error (H2). φ/NG/Sw must be
    /// fractions in `[0, 1]`, area/height strictly positive, and the FVF
    /// physically valid for the fluid (`Boi >= 1`, `0 < Bgi < 1`) — otherwise the
    /// analytic box arithmetic silently yields negative or `inf` in-place. The
    /// predicates are srs-volumetrics' (the volumetrics owner); the error composes
    /// into `SrsError` via `#[from]` and surfaces at the FFI as a `ValueError`.
    ///
    /// # Errors
    /// [`SrsError::Static`] (invalid input) on the first out-of-range field.
    pub fn validate(&self, fluid: Fluid) -> Result<(), SrsError> {
        validate_positive("area_m2", self.area_m2)?;
        validate_positive("gross_height_m", self.gross_height_m)?;
        validate_fraction("porosity", self.porosity)?;
        validate_fraction("net_to_gross", self.net_to_gross)?;
        validate_fraction("water_saturation", self.water_saturation)?;
        match fluid {
            Fluid::Oil => {
                OilFvf::new(self.fvf)?;
            }
            Fluid::Gas => {
                GasFvf::new(self.fvf)?;
            }
        }
        Ok(())
    }

    /// In-place surface volume \[Sm³\] (oil or gas). HCPV is reservoir m³ and the
    /// FVF is Rm³/Sm³, so the division lands directly in Sm³ — no barrel/ft³
    /// conversion in the chain.
    #[must_use]
    pub fn in_place(&self, fluid: Fluid) -> f64 {
        let hcpv = self.hcpv_m3();
        match fluid {
            Fluid::Oil | Fluid::Gas => hcpv / self.fvf,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petekstatic::grid::{build_box, BoxSpec, Dims};
    use petekstatic::volumetrics::{compute_in_place, populate_constant, ConstantPriors};

    fn draw(contact: f64) -> BoxDraw {
        BoxDraw {
            area_m2: 400_000.0,
            gross_height_m: 15.0,
            top_depth_m: 1500.0,
            contact_depth_m: contact,
            porosity: 0.25,
            net_to_gross: 0.8,
            water_saturation: 0.3,
            fvf: 1.25,
        }
    }

    #[test]
    fn full_column_matches_grv_from_grid() {
        // Cross-check: analytic == the grid-summation engine, full column.
        let d = draw(1800.0); // contact below base -> frac 1
        let analytic = d.in_place(Fluid::Oil);

        let mut grid = build_box(BoxSpec {
            area_m2: 400_000.0,
            gross_height_m: 15.0,
            dims: Dims::new(10, 10, 5).unwrap(),
            top_depth_m: 1500.0,
            aspect_ratio: 1.0,
        })
        .unwrap();
        populate_constant(
            &mut grid,
            ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
        )
        .unwrap();
        let grid_res = compute_in_place(&grid, 1800.0).unwrap();
        let grid_ooip = grid_res.ooip_sm3(petekstatic::volumetrics::OilFvf::new(1.25).unwrap());
        assert!(
            (analytic - grid_ooip).abs() / grid_ooip < 1e-9,
            "analytic {analytic} != grid {grid_ooip}"
        );
    }

    #[test]
    fn known_si_golden_value() {
        // 0.4 km² × 15 m, φ 0.25, NTG 0.8, Sw 0.3, Boi 1.25:
        // HCPV = 400 000·15·0.8·0.25·0.7 = 840 000 m³; OOIP = 672 000 Sm³.
        let d = draw(1800.0);
        assert!((d.hcpv_m3() - 840_000.0).abs() < 1e-6);
        assert!((d.in_place(Fluid::Oil) - 672_000.0).abs() < 1e-6);
    }

    #[test]
    fn contact_fraction_clamps() {
        assert!((draw(1507.5).above_contact_fraction() - 0.5).abs() < 1e-12);
        assert!((draw(1200.0).above_contact_fraction() - 0.0).abs() < 1e-12); // above top
        assert!((draw(2743.0).above_contact_fraction() - 1.0).abs() < 1e-12); // below base
    }

    #[test]
    fn gas_uses_bgi_directly() {
        let mut d = draw(2743.0);
        d.fvf = 0.004;
        let giip = d.in_place(Fluid::Gas);
        assert!((giip - d.hcpv_m3() / 0.004).abs() / giip < 1e-12);
    }
}
