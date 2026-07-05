//! The workspace error type.
//!
//! petekSim owns one error enum, [`SrsError`], surfaced across the crate. The
//! domain-agnostic oilfield-unit conversions that used to live here have moved
//! down the DAG into the petekTools toolkit (`petektools::units`); this module
//! now homes only the shared error type.

mod error;

pub use error::{Result, SrsError};
