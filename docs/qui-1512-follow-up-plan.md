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
  completed `46ab082` reference measurements, extra-case coverage, full-weight
  composition and the current 1,002,055-byte claim envelope.
- [Release inventory](release.md#040-package-inventory-mr-100): 0.4.0 packages,
  excluded Python crate and independently versioned packages.

Source decisions: [QUI-1512 findings](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-36bacce1),
[settlement decision](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-e2464260)
and [response](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-40530a42).
Completing the specification does not imply deployment of the full integration.
The implementation direction is tracked by
[QUI-1815](https://linear.app/quip-network/issue/QUI-1815).

## Remaining tracker inventory

Dedicated integration, royalty and reserve-isolation follow-ups now exist under
QUI-1512. Their linked permanent specifications retain the deferred decisions and
acceptance criteria; tracking does not imply deployment or activation.

| Work | Tracking / remaining requirement |
| --- | --- |
| Permissionless one-order settlement | [QUI-1816](https://linear.app/quip-network/issue/QUI-1816); implemented and reference-measured; final review/CI and deployment remain |
| A: ranking/payload consistency and best vector | [QUI-1408](https://linear.app/quip-network/issue/QUI-1408), [QUI-1675](https://linear.app/quip-network/issue/QUI-1675); implemented, deployment still required |
| SDK fresh-network verification | [QUI-1676](https://linear.app/quip-network/issue/QUI-1676); run after deployment |
| B: outstanding proposer capacity | [QUI-1818](https://linear.app/quip-network/issue/QUI-1818); implemented |
| C: observation/correlation protocol (points 4/6/8) | Specification delivered under [QUI-1512](https://linear.app/quip-network/issue/QUI-1512); [QUI-1817](https://linear.app/quip-network/issue/QUI-1817) tracks finality; consuming correlation/application implementation remains deferred |
| D: signed winner destination (point 7) | [QUI-1819](https://linear.app/quip-network/issue/QUI-1819) tracks payee direction; independent metadata implemented, payment activation remains deferred |
| Local Quip EVM / external EVM / both | [QUI-1820](https://linear.app/quip-network/issue/QUI-1820); deferred decision, do not assume external-only |
| Native-only / EVM-only / both reward model | [QUI-1820](https://linear.app/quip-network/issue/QUI-1820); deferred funding, execution and closure decision, no suppression, offsets or exemptions inferred |
| EVM activation and full Trustee/consumer application | [QUI-1820](https://linear.app/quip-network/issue/QUI-1820); deferred admission/deployment policy, network/contracts, source finality, escrow deadlines/refunds, signer trust/rotation and replay protection |
| Evidence/domain integration | [QUI-1820](https://linear.app/quip-network/issue/QUI-1820); deferred cross-language codec/hash fixtures, archive/late observer support, domain administration and reset/in-flight cutover |
| Solver/program royalties | [QUI-1821](https://linear.app/quip-network/issue/QUI-1821); deferred recipients, rates, funding and execution |
| Per-order reserve isolation and recovery | [QUI-1822](https://linear.app/quip-network/issue/QUI-1822); deferred, unnamed aggregate reserves can mask missing backing |
| SDK payout-rule classes | [QUI-1606](https://linear.app/quip-network/issue/QUI-1606); not a substitute for the payee protocol |
| Precompile / XQVM / ZK / hardware claims | QUI-999 / QUI-1475 / QUI-1134 and separate hardware-attestation work |

Unpaid orders retain both slots and funds; no forced-close or forced-refund policy
is introduced. Later changes after reset must audit actual new-network state and
add only justified bounded repair/migration, while preserving archived evidence.
Reference regeneration completed in bot commit `46ab082`. Final review and CI
of the resulting head remain merge gates; post-deployment SDK verification is
separate. Local tests never substitute for those stages.
