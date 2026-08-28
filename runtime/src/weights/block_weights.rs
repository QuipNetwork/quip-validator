//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 54.0.0
//! DATE: 2026-08-28 (Y/M/D)
//! HOSTNAME: `runner-qkwqxw0vd-project-80034548-concurrent-0`, CPU: `AMD Ryzen 7 3700X 8-Core Processor`
//!
//! SHORT-NAME: `block`, LONG-NAME: `BlockExecution`, RUNTIME: `Development`
//! WARMUPS: `10`, REPEAT: `100`
//! WEIGHT-PATH: `runtime/src/weights`
//! WEIGHT-METRIC: `Average`, WEIGHT-MUL: `1.0`, WEIGHT-ADD: `0`

// Executed Command:
//   /ci-cache/target/release/quip-network-node
//   benchmark
//   overhead
//   --dev
//   --wasm-execution
//   compiled
//   --warmup
//   10
//   --repeat
//   100
//   --weight-path
//   runtime/src/weights

use sp_core::parameter_types;
use sp_weights::{constants::WEIGHT_REF_TIME_PER_NANOS, Weight};

parameter_types! {
    /// Weight of executing an empty block.
    /// Calculated by multiplying the *Average* with `1.0` and adding `0`.
    ///
    /// Stats nanoseconds:
    ///   Min, Max: 329_454, 353_730
    ///   Average:  337_289
    ///   Median:   336_668
    ///   Std-Dev:  3956.06
    ///
    /// Percentiles nanoseconds:
    ///   99th: 347_328
    ///   95th: 345_053
    ///   75th: 338_992
    pub const BlockExecutionWeight: Weight =
        Weight::from_parts(WEIGHT_REF_TIME_PER_NANOS.saturating_mul(337_289), 0);
}

#[cfg(test)]
mod test_weights {
    use sp_weights::constants;

    /// Checks that the weight exists and is sane.
    // NOTE: If this test fails but you are sure that the generated values are fine,
    // you can delete it.
    #[test]
    fn sane() {
        let w = super::BlockExecutionWeight::get();

        // At least 100 µs.
        assert!(
            w.ref_time() >= 100u64 * constants::WEIGHT_REF_TIME_PER_MICROS,
            "Weight should be at least 100 µs."
        );
        // At most 50 ms.
        assert!(
            w.ref_time() <= 50u64 * constants::WEIGHT_REF_TIME_PER_MILLIS,
            "Weight should be at most 50 ms."
        );
    }
}
