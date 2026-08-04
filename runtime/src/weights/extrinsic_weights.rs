//! Bootstrap value for the base weight of executing an extrinsic.
//!
//! The reference-machine `benchmark overhead` run replaces this file. Until
//! then, this preserves the Polkadot SDK value previously inherited through
//! `BlockWeights::with_sensible_defaults`.

use sp_core::parameter_types;
use sp_weights::{constants::WEIGHT_REF_TIME_PER_NANOS, Weight};

parameter_types! {
    pub const ExtrinsicBaseWeight: Weight =
        Weight::from_parts(WEIGHT_REF_TIME_PER_NANOS.saturating_mul(108_157), 0);
}
