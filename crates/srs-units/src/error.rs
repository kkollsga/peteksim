//! The single workspace error type. Every crate surfaces failures as [`SrsError`].

use thiserror::Error;

/// Workspace-wide result alias.
pub type Result<T> = std::result::Result<T, SrsError>;

/// Errors raised across the petekSim crates.
#[derive(Debug, Error)]
pub enum SrsError {
    /// A caller-supplied value was outside its valid range (with a reason).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A grid dimension or index was out of bounds.
    #[error("grid error: {0}")]
    Grid(String),

    /// A value fell outside a correlation/spec's documented validity range.
    #[error("outside validity range: {0}")]
    OutOfRange(String),

    /// A failure from the petekStatic geomodel layer (grid/wireframe), composed
    /// across the repo seam so `?` chains and `source()` reaches the origin.
    #[error(transparent)]
    Static(#[from] petekstatic_error::StaticError),

    /// A failure from the petekio DATA layer (ingest/geometry), composed directly
    /// at the ingest seam so `?` chains in one hop (petekio's `GeoError` also
    /// composes into `StaticError`, but the direct variant lets a bare seam call
    /// use `?` without the two-`From`-hop workaround).
    #[error(transparent)]
    Geo(#[from] petekio::GeoError),

    /// A failure from the petekTools numeric kernels (kriging/variograms/
    /// sampling), composed directly at the toolkit seam so `?` chains in one hop.
    #[error(transparent)]
    Algo(#[from] petektools::AlgoError),
}
