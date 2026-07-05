//! The model-first refinement loop — now a **thin facade over the geomodel
//! seam**. The structural/population half (surface solve → layering →
//! population) was relocated to petekStatic's `srs-model` on 2026-07-03
//! (`task_relocate_refine_orchestration`): [`RefiningModel`] delegates the build
//! to `srs_model::StaticModelBuilder` and presents the produced
//! [`srs_model::StaticModel`]'s own volumetrics (the model owns volumes per the
//! layer charter, graph `decision_layer_charters`), applying the fluid's FVF at
//! this facade. SI units throughout (`decision_si_units_standard`): area m²,
//! depths m (positive down), in-place Sm³, GRV reported in mcm (10⁶ m³).

use crate::inplace::Fluid;
use srs_gridder::{Conformity, SolveOpts};
use srs_model::{BuildOpts, StaticModel, StaticModelBuilder};
use srs_units::SrsError;
use srs_volumetrics::{ConstantPriors, GasFvf, OilFvf};
use srs_wireframe::Wireframe;

/// A positioned petrophysical sample for grid population: TVD \[m\] +
/// porosity + water saturation (fractions). Re-exported from the geomodel layer
/// (`srs_model::PetroSample`), which owns population now.
pub type PetroSample = srs_model::PetroSample;

/// A refinable structural model: a footprint + a growing set of top-surface
/// depth control points. Re-solving converges a structured grid and its volumes.
/// Facade state = the geomodel builder + the fluid/FVF tail applied here.
#[derive(Debug, Clone)]
pub struct RefiningModel {
    builder: StaticModelBuilder,
    fluid: Fluid,
    fvf: f64,
}

/// The result of converging the model at the current control set.
#[derive(Debug, Clone)]
pub struct Refined {
    /// The populated static model (framework + grid + cubes) — the source of the
    /// view bundles (`map_bundle` / `volume_bundle` / `intersection_bundle`).
    pub model: StaticModel,
    /// In-place surface volume \[Sm³\] (oil or gas).
    pub in_place: f64,
    /// GRV of the hydrocarbon column \[mcm = 10⁶ m³\].
    pub grv_mcm: f64,
    /// Number of top-surface control points honoured.
    pub controls: usize,
}

/// The volumetric scalars a [`Wireframe`] does not carry — supplied alongside it to
/// [`RefiningModel::from_wireframe`]. SI units: area m², height m.
#[derive(Debug, Clone, Copy)]
pub struct WireframeSeed {
    pub area_m2: f64,
    pub gross_height_m: f64,
    pub nk: usize,
    pub priors: ConstantPriors,
    pub fluid: Fluid,
    /// Boi (oil) or Bgi (gas) \[Rm³/Sm³\].
    pub fvf: f64,
}

impl RefiningModel {
    /// A new model seeded with the four corner controls at `top_depth_m`, so
    /// the initial converged grid is the flat box.
    ///
    /// # Errors
    /// [`SrsError`] if dimensions are degenerate, area/height are non-positive,
    /// or the contact depth is not finite.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        area_m2: f64,
        gross_height_m: f64,
        ni: usize,
        nj: usize,
        nk: usize,
        top_depth_m: f64,
        contact_depth_m: f64,
        priors: ConstantPriors,
        fluid: Fluid,
        fvf: f64,
    ) -> Result<Self, SrsError> {
        let builder = StaticModelBuilder::flat(
            ni,
            nj,
            top_depth_m,
            contact_depth_m,
            BuildOpts {
                area_m2,
                gross_height_m,
                nk,
                conformity: Conformity::Proportional,
                solve_opts: SolveOpts::default(),
                priors,
            },
        )?;
        Ok(Self {
            builder,
            fluid,
            fvf,
        })
    }

    /// Seed a [`RefiningModel`] from a constraining [`Wireframe`] — the data-layer
    /// hand-off. The wireframe drives the **structure** (its `Top` horizon becomes
    /// the control set; a fluid contact sets the column base); the volumetric
    /// scalars the wireframe doesn't carry come from `seed`. Grid `(ni, nj)` is
    /// taken from the top surface lattice (`ncol-1, nrow-1`).
    ///
    /// # Errors
    /// [`SrsError`] if the wireframe has no `Top` horizon, the top surface is
    /// degenerate or fully undefined, no fluid contact is present, or `seed`
    /// dimensions are degenerate.
    pub fn from_wireframe(wf: &Wireframe, seed: WireframeSeed) -> Result<Self, SrsError> {
        let builder = StaticModelBuilder::from_wireframe(
            wf,
            BuildOpts {
                area_m2: seed.area_m2,
                gross_height_m: seed.gross_height_m,
                nk: seed.nk,
                conformity: Conformity::Proportional,
                solve_opts: SolveOpts::default(),
                priors: seed.priors,
            },
        )?;
        Ok(Self {
            builder,
            fluid: seed.fluid,
            fvf: seed.fvf,
        })
    }

    /// Attach positioned petro samples (TVD, φ, Sw) so [`Self::solve`] populates
    /// cells from upscaled logs (within each cell's depth range) instead of
    /// constant priors; cells with no samples keep the priors. Empty = priors.
    #[must_use]
    pub fn with_logs(mut self, samples: Vec<PetroSample>) -> Self {
        self.builder = self.builder.with_logs(samples);
        self
    }

    /// Add a top-surface depth control point on the `(ni+1) x (nj+1)` node
    /// lattice — the new datum the grid re-converges to honour.
    pub fn add_top_control(&mut self, ip: usize, jp: usize, depth_m: f64) {
        self.builder.add_top_control(ip, jp, depth_m);
    }

    /// Converge the grid at the current controls and recompute volumes: build the
    /// `StaticModel` across the seam, read its own in-place, apply the FVF here.
    ///
    /// # Errors
    /// [`SrsError`] if the geomodel build, FVF, or in-place calculation fails.
    pub fn solve(&self) -> Result<Refined, SrsError> {
        let model = self.builder.build()?;
        let res = model.in_place()?;
        let in_place = match self.fluid {
            Fluid::Oil => res.ooip_sm3(OilFvf::new(self.fvf)?),
            Fluid::Gas => res.ogip_sm3(GasFvf::new(self.fvf)?),
        };
        Ok(Refined {
            model,
            in_place,
            grv_mcm: res.grv_mcm(),
            controls: self.builder.control_count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_contact(contact_depth_m: f64) -> RefiningModel {
        RefiningModel::new(
            400_000.0,
            15.0,
            10,
            10,
            5,
            1500.0,
            contact_depth_m,
            ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
            Fluid::Oil,
            1.25,
        )
        .unwrap()
    }

    fn model() -> RefiningModel {
        // Contact 15 m below the base -> full column initially.
        model_with_contact(1530.0)
    }

    #[test]
    fn flat_model_matches_box_volume() {
        // Initial flat top: bulk volume == area * height = 400 000 × 15 = 6 mcm.
        let r = model().solve().unwrap();
        let expected_grv_mcm = 6.0;
        assert!((r.grv_mcm - expected_grv_mcm).abs() / expected_grv_mcm < 1e-6);
        assert!(r.in_place > 0.0);
    }

    #[test]
    fn adding_a_structural_high_changes_the_answer() {
        // Contact partway up so only the crest is in the column; a shallow
        // control should pull more rock above the contact -> more oil.
        let mut m = model_with_contact(1507.5); // mid-column
        let before = m.solve().unwrap();
        m.add_top_control(5, 5, 1494.0); // a structural high at the center
        let after = m.solve().unwrap();
        assert!(after.controls == before.controls + 1);
        assert!(
            after.in_place > before.in_place,
            "structural high should raise in-place: {} -> {}",
            before.in_place,
            after.in_place
        );
    }

    #[test]
    fn pure_dip_conserves_gross_volume() {
        // Tilting the top while keeping constant thickness must not change GRV
        // when the whole column stays above the contact.
        let mut m = model();
        let flat = m.solve().unwrap();
        m.add_top_control(5, 5, 1488.0);
        let tilted = m.solve().unwrap();
        assert!((tilted.grv_mcm - flat.grv_mcm).abs() / flat.grv_mcm < 1e-6);
    }

    // --- from_wireframe (data-layer hand-off) ---
    use srs_wireframe::{
        Boundary, Contact, ContactKind, GriddedDepth, Hardness, Horizon, HorizonRole, Wireframe,
    };

    /// A wireframe with an `n×n` top surface at a constant `depth_m` and one OWC.
    fn flat_wireframe(n: usize, depth_m: f64, owc_m: f64) -> Wireframe {
        let depth = vec![depth_m; n * n];
        Wireframe {
            boundary: Boundary {
                ring: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]],
                hardness: Hardness::Interpolated,
            },
            horizons: vec![Horizon {
                name: "top".into(),
                role: HorizonRole::Top,
                surface: GriddedDepth {
                    ncol: n,
                    nrow: n,
                    depth_m: depth,
                    is_control: vec![true; n * n],
                },
            }]
            .into(),
            contacts: vec![Contact {
                kind: ContactKind::Owc,
                depth_m: owc_m,
                hardness: Hardness::Hard,
            }],
        }
    }

    fn seed() -> WireframeSeed {
        WireframeSeed {
            area_m2: 400_000.0,
            gross_height_m: 15.0,
            nk: 5,
            priors: ConstantPriors {
                porosity: 0.25,
                net_to_gross: 0.8,
                water_saturation: 0.3,
            },
            fluid: Fluid::Oil,
            fvf: 1.25,
        }
    }

    #[test]
    fn flat_wireframe_matches_box_volume() {
        // 11×11 flat top, contact below the base -> full column == area*height.
        let wf = flat_wireframe(11, 1500.0, 1530.0);
        let m = RefiningModel::from_wireframe(&wf, seed()).unwrap();
        let r = m.solve().unwrap();
        let expected_mcm = 6.0; // 400 000 m² × 15 m
        assert!((r.grv_mcm - expected_mcm).abs() / expected_mcm < 1e-6);
        assert!(r.in_place > 0.0);
        // (ni, nj) inferred from the surface lattice (ncol-1, nrow-1).
        assert_eq!(r.controls, 11 * 11);
    }

    #[test]
    fn structural_high_in_the_wireframe_raises_in_place() {
        // Contact mid-column; a shallow crest pulls more rock above it -> more oil.
        let mut wf = flat_wireframe(11, 1500.0, 1507.5);
        let flat = RefiningModel::from_wireframe(&wf, seed())
            .unwrap()
            .solve()
            .unwrap();
        // raise the centre node into a structural high (horizons are shared as an
        // `Arc<Vec<_>>` at the petekStatic seam; `make_mut` gives unique access)
        std::sync::Arc::make_mut(&mut wf.horizons)[0]
            .surface
            .depth_m[5 * 11 + 5] = 1494.0;
        let crest = RefiningModel::from_wireframe(&wf, seed())
            .unwrap()
            .solve()
            .unwrap();
        assert!(
            crest.in_place > flat.in_place,
            "structural high should raise in-place: {} -> {}",
            flat.in_place,
            crest.in_place
        );
    }

    #[test]
    fn logs_populate_cells_in_their_depth_range() {
        // A flat 11x11 top at 1500 m, gross 15 -> column spans 1500..1515. Logs only
        // in the upper half (1500..1507.5) with a high porosity 0.30, distinct from
        // the 0.25 prior.
        let wf = flat_wireframe(11, 1500.0, 1530.0);
        let samples: Vec<PetroSample> = (0..=75)
            .map(|i| (1500.0 + 0.1 * f64::from(i), 0.30, 0.20)) // (tvd m, phi, sw)
            .collect();
        let r = RefiningModel::from_wireframe(&wf, seed())
            .unwrap()
            .with_logs(samples)
            .solve()
            .unwrap();
        let poro = r.model.grid().properties().get("PORO").unwrap();
        // some cells (upper layers) took the log value 0.30, others kept the prior 0.25
        assert!(
            poro.values.iter().any(|&v| (v - 0.30).abs() < 1e-9),
            "log-driven cells"
        );
        assert!(
            poro.values.iter().any(|&v| (v - 0.25).abs() < 1e-9),
            "prior cells below logs"
        );
        let sw = r.model.grid().properties().get("SW").unwrap();
        assert!(
            sw.values.iter().any(|&v| (v - 0.20).abs() < 1e-9),
            "log Sw applied"
        );
    }

    #[test]
    fn empty_logs_fall_back_to_priors() {
        let wf = flat_wireframe(11, 1500.0, 1530.0);
        let r = RefiningModel::from_wireframe(&wf, seed())
            .unwrap()
            .with_logs(vec![]) // empty -> priors everywhere
            .solve()
            .unwrap();
        let poro = r.model.grid().properties().get("PORO").unwrap();
        assert!(
            poro.values.iter().all(|&v| (v - 0.25).abs() < 1e-9),
            "all prior porosity"
        );
    }

    #[test]
    fn from_wireframe_needs_a_top_and_a_contact() {
        // No Top horizon -> error.
        let mut wf = flat_wireframe(4, 1500.0, 1530.0);
        std::sync::Arc::make_mut(&mut wf.horizons).clear();
        assert!(RefiningModel::from_wireframe(&wf, seed()).is_err());
        // No contact -> error.
        let mut wf2 = flat_wireframe(4, 1500.0, 1530.0);
        wf2.contacts.clear();
        assert!(RefiningModel::from_wireframe(&wf2, seed()).is_err());
    }
}
