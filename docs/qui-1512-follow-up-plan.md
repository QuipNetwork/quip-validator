# QUI-1512 implementation handoff and remaining tracking

Independent A–D and permissionless one-order settlement are implemented for the
runtime 119 reset. Runtime spec 119, transaction 7 and mempool storage 4 remain
unchanged. This does not claim deployment of a Trustee, EVM payment system or
royalty mechanism. No pre-reset compute state is imported.

Durable guidance is maintained in:

- [Mempool API and lifecycle](quantum-compute-mempool.md): strict improvement,
  best-anchored storage, outstanding proposer slots, atomic settlement/recovery,
  signed payee metadata, event transitions and reserve-isolation limitation.
- [Compute interaction protocol](quip-evm-compute-interaction.md): finalised
  evidence, remark correlation, retry/domain/reset rules and deferred integration.
- [Benchmarking and release gate](pallet-benchmarking-plan.md#permissionless-settlement-reference-coverage-and-release-gate):
  the 4b70 reference baseline, extra-case coverage, full-weight composition,
  current 1,002,055-byte claim envelope and required final reference run.
- [Release inventory](release.md#040-package-inventory-mr-100): 0.4.0 packages,
  excluded Python crate and independently versioned packages.

Source decisions: [QUI-1512 findings](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-36bacce1),
[settlement decision](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-e2464260)
and [response](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-40530a42).
QUI-1512 being Done describes evaluation, not deployment of the full integration.

## Remaining tracker inventory

Keep these entries until dedicated issue links exist; planner coordinates tickets.
Consolidating the implementation plan does not cancel deferred work.

| Work | Tracking / remaining requirement |
| --- | --- |
| A: ranking/payload consistency and best vector | [QUI-1408](https://linear.app/quip-network/issue/QUI-1408), [QUI-1675](https://linear.app/quip-network/issue/QUI-1675); implemented, deployment still required |
| SDK fresh-network verification | [QUI-1676](https://linear.app/quip-network/issue/QUI-1676); run after deployment |
| B: outstanding proposer capacity | Implemented; dedicated issue link pending |
| C: observation/correlation protocol (points 4/6/8) | Specification delivered; dedicated issue and consuming implementation links pending |
| D: signed winner destination (point 7) | Independent metadata implemented; dedicated issue link pending |
| Local Quip EVM / external EVM / both | Deferred decision; dedicated issue link pending; do not assume external-only |
| Native-only / EVM-only / both reward model | Deferred funding, execution and closure decision; no suppression, offsets or exemptions inferred |
| EVM activation and full Trustee/consumer application | Deferred admission/deployment policy, network/contracts, source finality, escrow deadlines/refunds, signer trust/rotation, replay protection; issue links pending |
| Evidence/domain integration | Deferred cross-language codec/hash fixtures, archive/late observer support, domain administration and reset/in-flight cutover; issue links pending |
| Solver/program royalties | Deferred recipients, rates, funding and execution; issue link pending |
| Per-order reserve isolation and recovery | Deferred; unnamed aggregate reserves can mask missing backing; issue link pending |
| SDK payout-rule classes | [QUI-1606](https://linear.app/quip-network/issue/QUI-1606); not a substitute for the payee protocol |
| Precompile / XQVM / ZK / hardware claims | QUI-999 / QUI-1475 / QUI-1134 and separate hardware-attestation work |

Unpaid orders retain both slots and funds; no forced-close or forced-refund policy
is introduced. Later changes after reset must audit actual new-network state and
add only justified bounded repair/migration, while preserving archived evidence.
Final code review, CI and reference regeneration remain merge gates; post-deployment
SDK verification is separate. Local tests never substitute for those stages.
