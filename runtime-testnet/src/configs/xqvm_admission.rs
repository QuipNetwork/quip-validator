//! Pause new XQVM program admission pending a verifier update.
use crate::RuntimeCall;
use frame_support::traits::Contains;

/// Preserve the default filter for every call except new XQVM program storage.
/// This does not read storage. Root bypass is an accepted governance capability.
pub struct RuntimeCallFilter;
impl Contains<RuntimeCall> for RuntimeCallFilter {
    fn contains(call: &RuntimeCall) -> bool {
        !matches!(
            call,
            RuntimeCall::Xqvm(pallet_xqvm::Call::store_program { .. })
        )
    }
}

#[cfg(test)]
mod tests;
