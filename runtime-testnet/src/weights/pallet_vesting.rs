//! Provisional SDK 845eb99 weights, not Quip reference-machine measurements.
//! The benchmark-weights job replaces this alias with the runtime-style table
//! and mirrors it to both runtimes. Reference calibration is required for launch
//! under QUI-2101 (parent QUI-2065), owned by roman@postquant.xyz.
pub type WeightInfo<T> = pallet_vesting::weights::SubstrateWeight<T>;
