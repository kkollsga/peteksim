//! The `pvt` module — PVT properties. MVP scope is **FVF-as-input** ([`OilFvf`],
//! [`GasFvf`]); the `gas_fvf` (DAK Z) correlation is a later fast-follow that
//! produces the same [`GasFvf`] value type, keeping the downstream boundary
//! stable.

mod fvf;

pub use fvf::{GasFvf, OilFvf};
