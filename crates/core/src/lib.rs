//! Consensus-critical primitives for Summerpoem: canonical encoding, hashing,
//! transaction and block structures, emission schedule, and difficulty.

pub mod asert;
pub mod block;
pub mod compact;
pub mod emission;
pub mod encode;
pub mod hash;
pub mod merkle;
pub mod params;
pub mod tx;

pub use hash::Hash256;
pub use params::{Network, Params, PowParams};
