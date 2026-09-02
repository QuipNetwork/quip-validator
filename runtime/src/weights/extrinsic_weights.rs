//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 54.0.0
//! DATE: 2026-09-02 (Y/M/D)
//! HOSTNAME: `runner-qkwqxw0vd-project-80034548-concurrent-0`, CPU: `AMD Ryzen 7 3700X 8-Core Processor`
//!
//! SHORT-NAME: `extrinsic`, LONG-NAME: `ExtrinsicBase`, RUNTIME: `Development`
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
    /// Weight of executing a NO-OP extrinsic, for example `System::remark`.
    /// Calculated by multiplying the *Average* with `1.0` and adding `0`.
    ///
    /// Stats nanoseconds:
    ///   Min, Max: 590_047, 591_487
    ///   Average:  590_561
    ///   Median:   590_558
    ///   Std-Dev:  267.64
    ///
    /// Percentiles nanoseconds:
    ///   99th: 591_389
    ///   95th: 590_960
    ///   75th: 590_756
    pub const ExtrinsicBaseWeight: Weight =
        Weight::from_parts(WEIGHT_REF_TIME_PER_NANOS.saturating_mul(590_561), 0);
}

#[cfg(test)]
mod test_weights {
    use sp_weights::constants;

    /// Checks that the weight exists and is sane.
    // NOTE: If this test fails but you are sure that the generated values are fine,
    // you can delete it.
    #[test]
    fn sane() {
        let w = super::ExtrinsicBaseWeight::get();

        // At least 10 µs.
        assert!(
            w.ref_time() >= 10u64 * constants::WEIGHT_REF_TIME_PER_MICROS,
            "Weight should be at least 10 µs."
        );
        // At most 1 ms.
        assert!(
            w.ref_time() <= constants::WEIGHT_REF_TIME_PER_MILLIS,
            "Weight should be at most 1 ms."
        );
    }
}
