//! Bootstrap value for the weight of executing an empty block.
//!
//! The reference-machine `benchmark overhead` run replaces this file. Until
//! then, this preserves the Polkadot SDK value previously inherited through
//! `BlockWeights::with_sensible_defaults`.

use sp_core::parameter_types;
use sp_weights::{constants::WEIGHT_REF_TIME_PER_NANOS, Weight};

parameter_types! {
    pub const BlockExecutionWeight: Weight =
        Weight::from_parts(WEIGHT_REF_TIME_PER_NANOS.saturating_mul(431_614), 0);
}
