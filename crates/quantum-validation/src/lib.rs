//! Deterministic, `no_std` quantum/Ising validation primitives for QUIP.
//!
//! This crate owns pure math and structural validation helpers that are shared
//! by higher-level consumers such as the future quantum job mempool pallet and
//! a future quantum PoW pallet.
//!
//! The public API is organized by concern:
//!
//! - [`energy`] for exact Ising energy computation
//! - [`diversity`] for symmetric Hamming distance and diversity scoring
//! - [`ising`] for deterministic nonce derivation and model generation
//! - [`validation`] for spin and solution-set validation
//! - [`errors`] for typed validation failures
//! - [`fixed`] for shared fixed-point aliases and constants
//!
//! # Fixed-point model
//!
//! The crate follows the milli-precision convention from the Notion spec:
//!
//! - energies use [`MilliEnergy`] (`i64`)
//! - local fields and couplings use [`MilliValue`] (`i32`)
//! - diversity uses [`MilliDiversity`] (`u32`)
//! - the scale factor is [`MILLI_SCALE`] = `1000`
//!
//! For example, `-1.25` is represented as `-1250`.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod diversity;
pub mod energy;
pub mod errors;
pub mod fixed;
pub mod hardness;
pub mod ising;
pub mod packed;
pub mod puzzle_spec;
pub mod validation;

pub use crate::diversity::{calculate_diversity, select_diverse, symmetric_hamming};
pub use crate::energy::{
    energy_of_solution, energy_of_solution_indexed, expected_bound_milli, expected_gse,
};
pub use crate::errors::ValidationError;
pub use crate::fixed::{MilliDiversity, MilliEnergy, MilliValue, MILLI_SCALE};
#[cfg(any(feature = "std", feature = "runtime-benchmarks"))]
pub use crate::hardness::caterpillar_of_cliques;
pub use crate::hardness::{
    canonical_graph, cycle_rank, energy_bound_milli, expected_frustration_for, frustration_index,
    frustration_index_milli, frustration_sigmas, induced_width_at_most, loosest_energy_bound_milli,
    verify_planar_embedding, Regime, WitnessError, HANDICAP_MAX_SIGMA,
};
pub use crate::ising::{
    derive_nonce, generate_ising_model, generate_ising_model_and_frustration, IsingInstance,
};
pub use crate::packed::{packed_solution_byte_len, unpack_solution};
pub use crate::puzzle_spec::{AllowedValueSpec, MAX_INDEXED_BITS};
pub use crate::validation::{
    topology_consistency_ok, validate_solution, validate_solution_set, validate_spins,
    validate_topology_consistency, SolutionValidation, TopologyIndex,
};
