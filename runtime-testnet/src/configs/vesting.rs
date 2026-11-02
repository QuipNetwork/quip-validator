//! Signed custody grants. Mainnet allocation policy is resolved at launch;
//! the same pallet is available without genesis grants on testnet.
use crate::{Balance, Balances, BlockNumber, Runtime, RuntimeEvent, System, EXISTENTIAL_DEPOSIT};
use frame_support::{parameter_types, traits::WithdrawReasons};
use sp_runtime::traits::ConvertInto;

// Revisitable defaults, not approval of any real grant or calendar schedule.
pub const DEFAULT_CLIFF_BLOCKS: BlockNumber = 5_256_000;
pub const DEFAULT_LINEAR_BLOCKS: BlockNumber = 15_768_000;
pub const DEFAULT_TRANCHE_CADENCE_MONTHS: u32 = 3; // quarterly; twelve issues over 36 months
pub const DEFAULT_MAX_VESTING_SCHEDULES: u32 = 28;

parameter_types! {
    // Raise the cost of filling 28 schedule slots; council force-remove is denied.
    pub const MinVestedTransfer: Balance = 100 * EXISTENTIAL_DEPOSIT;
    pub UnvestedFundsAllowedWithdrawReasons: WithdrawReasons = WithdrawReasons::empty();
}

impl pallet_vesting::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type BlockNumberToBalance = ConvertInto;
    type MinVestedTransfer = MinVestedTransfer;
    type WeightInfo = crate::weights::pallet_vesting::WeightInfo<Runtime>;
    type UnvestedFundsAllowedWithdrawReasons = UnvestedFundsAllowedWithdrawReasons;
    type BlockNumberProvider = System;
    const MAX_VESTING_SCHEDULES: u32 = DEFAULT_MAX_VESTING_SCHEDULES;
}

// The freeze constrains total balance: held stake discounts the free-balance
// restriction. A stash with enough held stake can transfer a new grant at once.
// Staking frozen funds via Fortitude::Force is intentional. A guarantee that
// grants themselves stay illiquid needs a new owner decision and hold-based design.
#[cfg(test)]
mod tests;
