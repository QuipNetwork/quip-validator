# Runtime 120 compatibility

Runtime 120 was never deployed. The `v0.4.1`-tagged 120 blob must never be
enacted on any network. This runtime requires fresh genesis and provides no
upgrade path from older storage versions.

The accepted batch-mapping and native contract-deposit policies, plus the
re-anchor procedure, are documented in the [repository README](../README.md).

The stable2609 port renumbers `MultiBlockElectionSigned` events rather than
appending them: `Slashed`, `Ejected`, `Discarded`, and `Bailed` move from indices
3–6 to 6–9. Decoders and indexers using pre-port runtime-120 metadata, including
the published `v0.4.1` 120/7 blob, must refresh their metadata. Revive's
`DeletionQueue` value changes from `BoundedVec<u8>` to
`DeletionQueueItem { trie_id, account_id }`. This storage-layout change is safe
only because runtime 120 launches from fresh genesis; this port is not an
in-place upgrade for an existing runtime-120 chain.

`System.CodeUpdated` (event index 2) gains a `hash: H256` field, so upgrade
watchers must update their decoders. `Offences.Offence` (event index 0) renames
`timeslot` to `slot`; the SCALE encoding is unchanged, but name-based decoders
must update. Against the archived pre-vesting baseline, `Template` at index 7
is removed and that index is retired; `Vesting` is present at index 32.
The new signed-election events occupying indices 3–5 are
`RewardPaymentDeferred`, `UnpaidRewardEvicted`, and `FeeRefundFailed`.

Other storage metadata changes include Revive's `EthereumBlock` type moving
from `evm::api::rpc_types_gen::Block` to `evm::api::block::Block`, and new
`Revive.NativeDepositOf`, `MultiBlockElectionSigned.UnpaidRewards`, and
`Vesting.Vesting`/`Vesting.StorageVersion` entries. These are part of the
fresh-genesis baseline, not an in-place storage migration.

`ReviveApi` is now version 2 and uses `pallet_revive_types` V1 payload/result
types. It adds `eth_estimate_gas`, `eth_pre_dispatch_weight`,
`trace_call_with_config`, `max_extrinsic_weight_in_gas`, `version_declarations`,
and 24 `*_versioned` methods. Deploy a 2609-compatible eth-rpc proxy in lockstep
with this runtime.

`RuntimeFreezeReason::Revive` at index 14 raises `Balances::MaxFreezes` from
0 to 1. With Revive configured with `Deposit = ()`, that freeze is never applied.

Since runtime 120 (stable2609), general (V5) extrinsic wrappers must use
transaction-extension version 0. Nonzero versions previously accepted around
`eth_transact` now fail during decoding. The checked-in Quip production
builders construct none of these nonzero wrappers, in either the inspected
pre-port or current code, so this tightening introduces no wrapper-related
upgrade-ordering requirement for that tooling. Third-party clients that
hand-built such wrappers must use bare V5 or general V5/version 0 instead.
Only the outer wrapper changes: the signed Ethereum payload is unchanged
and does not need to be re-signed.
This applies to legacy/EIP-155, EIP-2930 and EIP-1559 transactions; Ethereum
transaction type numbers are separate from outer extension versions.

The batch-mapping call has index 13; Revive also adds `AutoMappingEnabled`
and `PendingDepositCleanup` errors at indices 66 and 67.

The signed election phase remains disabled. `MaxFeeRefund = 0` is a latent
change from the old SDK's uncapped winner fee refund: enabling the phase
without revisiting this setting would refund no fees. The reward source uses
the dedicated `qp/sgnrd` pallet account rather than SDK minting. It has no
configured funding, so positive rewards from an empty pot are deferred;
external transfers can fund it, but payouts still transfer existing funds.

`MultiBlockElectionSigned.claim_unpaid_reward` is call 5; its new errors are
`NoUnpaidReward` (9) and `PotStillDepleted` (10). The disabled signed phase
normally creates no rewards to claim; the call itself remains dispatchable
and rejects a claim when no queued reward exists.

Against the archived pre-vesting baseline, `System.BlockWeights` base block
and extrinsic costs move from 439,945/319,244 ns to 447,438/316,760 ns.
`Xqvm.WeightPerStep` changes from 35,592 to 29,256 ps and the derived
`Xqvm.MaxStepLimit` from 21,044,939 to 25,613,469. These values include the
reference-machine regeneration in `c67fbdb`. Mempool VM errors occupy
indices 36–44; its limits are 5,000,000 transform steps per submission and
16,777,216 bytes of VM memory, with `VmWeightPerStep` matching the XQVM rate.

The runtime remains at spec 120/transaction 7 with the same signed-extension
set, but its metadata hash has changed. Signers enabling `CheckMetadataHash`
need the new metadata. Wallets caching by `(specName, specVersion)` must
refetch rather than assume that matching 120/7 identifies the metadata in
the published `v0.4.1` blob.

The pre-vesting evidence (captured from `ba20b1e`, checked in by `b483bfb`)
remains in git history at
[`b483bfb:tests/fixtures/pre-vesting/manifest.json`](https://gitlab.com/quip.network/quip-validator/-/blob/b483bfb1f8fb58e0be86f72c73bdf8119de63329/tests/fixtures/pre-vesting/manifest.json).
The original `f7da5ec` post-vesting capture is preserved in
[`tests/fixtures/post-vesting-f7da5ec/manifest.json`](../tests/fixtures/post-vesting-f7da5ec/manifest.json);
its validation claims predate the review corrections above. The corrected
100×ED capture is recorded separately in
[`tests/fixtures/post-vesting-fix/manifest.json`](../tests/fixtures/post-vesting-fix/manifest.json).
SDK weights remain provisional until the reference-machine run and final
metadata/storage/signing review tracked by
[QUI-2101](https://linear.app/quip-network/issue/QUI-2101/vesting-release-gate-reference-machine-weights-and-metadata-re),
owned by `roman@postquant.xyz`.
