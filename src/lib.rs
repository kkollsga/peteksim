//! `peteksim` — the SIMULATION layer / product facade of the petek
//! subsurface-modelling suite, consolidated into **one crate**.
//!
//! Packaging note (2026-07-05): the three historical published library crates
//! (`srs-units`, `srs-pvt`, `srs-core`) were merged into this single `peteksim`
//! crate. Today's boundaries are preserved as **modules** with the same
//! one-directional import discipline they had as crates:
//!
//! ```text
//! units → pvt → core
//! ```
//!
//! - [`units`] — the one workspace error type ([`SrsError`]).
//! - [`pvt`] — PVT properties (FVF-as-input: [`pvt::OilFvf`], [`pvt::GasFvf`]).
//! - [`core`] — the orchestration spine: the model-first refine loop, the
//!   appraisal facade, the analytic box path, and the typed viewer bundles.
//!
//! The headline Rust API (`run_model`, the appraisal facade types, the
//! `Distribution` seam, the view bundles, …) is re-exported at the crate root,
//! so callers reach the common types without needing to know which module they
//! live in.

pub mod core;
pub mod pvt;
pub mod units;

// The single workspace error type, at the crate root.
pub use units::{Result, SrsError};

// The orchestration surface (the top of the DAG) at the crate root — this carries
// the headline API: run_model, the appraisal facade types, the Distribution seam,
// the view bundles, and the re-exported petekStatic / petekTools seam types.
pub use core::*;
