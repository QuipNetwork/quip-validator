# QUI-1512 follow-up plan

Status: independent A–D implemented on the review branch, 2026-10-11; review,
reference timing and deployment gates remain. The decisions and original scope
below are retained for review. This does not claim completion of the Trustee or
EVM contract implementation.

## Decisions agreed with the user

Recorded 2026-10-11:

- Implement settlement and all A–D before the runtime 119 network reset.
- Accept only strictly better resubmissions; document rejection of equal/worse
  submissions and preserve the accepted payload and tie priority.
- Unpaid orders retain their proposer slot and funds. No forced-close or
  forced-refund mechanism is included.
- Correlate external jobs using an atomic remark-plus-proposal batch for now.
- Require an EVM payee for submissions to orders designated for external payment.
- Implement winner payees only; defer solver/program royalties to later work.
- Deliver a chain-agnostic interaction specification for C. Choose the initial
  EVM network, chain-specific finality policy and escrow timelock/refund settings
  during the later EVM/Trustee deployment work, before admitting external jobs.

### Deferred EVM scope and reward decision

The user explicitly deferred this decision: “EVM” may mean Quip's own EVM
attached to the Substrate node, an external EVM network, or both. The plan must
not assume the external-network interpretation. Whether an EVM-facing job pays
a native Quip reward, an EVM reward, or both is also unresolved. Do not implement
reward suppression, splitting, offsets, minimum-reward exemptions or an EVM
settlement/closure mechanism on the basis of an assumed answer.

Implement everything independent of those choices now, one commit at a time:
A, B, C's shared protocol specification, and D's signed winner-payee metadata
and validation. Existing native orders keep their current funding and settlement
behavior; that is compatibility, not a decision about future EVM-facing rewards.
For D, separate payee requirements from payment execution: requiring destination
metadata must not imply that an EVM payment occurred or select a reward model.
Record any remaining dependent API/activation work explicitly for later rather
than enabling a guessed EVM-payment mode. This deferral qualifies the earlier
all-A–D-before-reset target; it does not block the independent implementation.

## Basis and current branch

Source: [the eight chain-side findings and replies](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-36bacce1),
especially [Roman's settlement decision](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-e2464260)
and [Colton's response](https://linear.app/quip-network/issue/QUI-1512/create-spec-for-evm-compute-and-interaction-with-qvm#comment-40530a42).
QUI-1512 being Done describes the evaluation, not delivery of this work.

Point 1 is implemented on `ru/fixes/qui-1512-findings`: any signed caller can pay
the transaction fee to settle all selected winners. There is no caller bounty.
Rebased settlement commit `e4481ce6adcd286f2b89e1b375da66915daa956d` is based on
main `7ac712699d3deefeb8cc70edadd75d2e1ae9aca6`, which includes MR !99's proactive
finalisation and MR !97's difficulty changes. The integration retains hook-based
expiry, rank-weighted payouts, main's reference timing as the baseline, the
settlement proof-size floor, and atomic payout rollback. Its claim database-access
model is adjusted pending measurement. Tests and benchmark setup finalise orders
through the hook before attempting settlement.
Native validation alone does not replace the Wasm CI build or reference-machine
benchmark regeneration for the changed settlement path.

### Runtime 119 launch boundary

Runtime 119 will launch with a network reset, not an in-place upgrade preserving
the current network's mempool state. Plan for fresh genesis and fresh orders,
rankings, proposer indexes and result records. Do not import pre-reset compute
state or build repair/migration machinery for it as part of these follow-ups.

Implement point 1 and the independent portions of A–D before the runtime 119 reset,
as agreed
with the user. A/B establish correctness and proposer-index invariants from the
first order; C supplies the agreed interaction specification and D supplies the
runtime payee support. Building the full external Trustee/EVM application remains
outside this plan. This is the implementation schedule, not a claim of completion.
If that schedule changes and a follow-up lands after the reset, reassess the state
created on runtime 119 and add only the migration or cleanup it needs.
Changes to fresh-genesis storage still need correct storage versions, genesis
tests, metadata and client compatibility checks. Existing in-place migration
tests do not substitute for validation of the fresh-genesis launch path.

Quip's local EVM state follows the network reset. Contracts and escrows on an
external EVM network do not reset with Quip; off-chain Trustee databases also
need explicit cutover handling. Describe both cases without choosing a deployment
model. If obligations exist at cutover, reconcile them under the eventual agreed
settlement/refund policy before admitting EVM-facing jobs on the new network.
Never replay old jobs or reuse old attestations merely because order IDs restart.

| Original point | Treatment |
| --- | --- |
| 1. Settlement liveness | Existing branch; integrate and validate first. |
| 2. Ranked score can outlive its payload | Follow-up A, before Trustee attestation. |
| 3. Stored subset can omit the best vector | Follow-up A, together with point 2. |
| 4. Final enough to attest | Follow-up C: finalised, consistent snapshots and reset policy. |
| 5. Lifetime proposer cap | Follow-up B: bounded outstanding-order index. |
| 6. External job correlation | Follow-up C: specify the agreed atomic remark-plus-proposal protocol. |
| 7. EVM winner payout address | Follow-up D: signed per-submission payee. |
| 8. Hardware/work claims | Document current limits in C; self-declared miner type remains accepted. |

## A. Keep ranked scores and stored solutions consistent

Implement [QUI-1408](https://linear.app/quip-network/issue/QUI-1408/verify-reward-settlement-against-overwritten-solver-results)
and [QUI-1675](https://linear.app/quip-network/issue/QUI-1675/fixmempool-store-the-vector-best-energy-milli-is-computed-from)
together. GRANDPA finality makes a state durable; it cannot repair an internally
inconsistent ranking and payload.

Decided submission policy (user, 2026-10-11): accept only strictly better
resubmissions and document this behavior in the pallet/API and SDK documentation.

- Retain the best accepted submission for each `(order, solver)`. Reject equal
  or worse resubmissions without changing stored vectors, ranking or tie priority.
  Append a dedicated error variant without reordering existing errors. Update
  SDK/coordinator handling to treat this rejection as a non-retryable unchanged
  submission, while allowing a genuinely improved submission later.
- On a strictly better accepted submission, update its payload and ranking
  atomically. Preserve the existing tie ordering between solvers.
- Always retain a vector attaining the submission's minimum validated energy.
  Anchor diverse-subset selection on that vector, then fill remaining slots
  deterministically. Keep at least one vector when the requested storage count
  is absent, zero or one; define these cases explicitly in the API documentation.
- Apply the existing diversity threshold to the resulting subset. Do not weaken
  acceptance criteria merely to retain the best vector. Scope the helper to the
  mempool unless changing the shared quantum-PoW selector is separately justified.

Required invariant: every ranked solver's score equals the minimum recomputed
energy of that solver's stored vectors. Cover worse/equal/better resubmissions,
all payout modes, best vector not at index zero, storage counts zero/one/multiple,
deterministic ties, diversity rejection, and atomic failure with regression tests.

With A included in the runtime 119 reset release, validate the invariant from
fresh genesis; pre-reset inconsistent orders are outside the new network's
attestation domain and require no on-chain repair. An overwritten best vector
cannot be recovered by waiting for finality or rescoring the remaining payload.
If A ships after the reset, audit orders created before A on the new network and
specify repair or exclusion for those orders before enabling Trustee attestations.

After deployment, run the SDK regression in
[QUI-1676](https://linear.app/quip-network/issue/QUI-1676/testxqsa-recheck-the-leader-pick-once-the-mempool-stores-its-ranked)
and verify `leader_energy_matches_chain=True` against fresh post-reset orders.
Do not require a legacy-order fallback for the new network when both fixes are
present from genesis. Any SDK support for historical networks must be isolated
by network/reset identity; if A lands late, retain handling for affected
post-reset orders until their explicit repair/exclusion policy is in place.

## B. Make the proposer cap bound outstanding orders

Keep the existing production value `MaxOrdersPerProposer = 32`, but redefine it
to limit orders not yet Closed, including expired orders awaiting settlement or
refund. This changes index semantics, not the constant; the mock runtime uses 8.
It must not limit the number a proposer can create over its lifetime.

- Remove an order from `ProposerOrders` only after successful settlement or
  refund, in the same transaction. Preserve historical `JobOrders` records.
- When B ships at the runtime 119 reset, indexes start empty: omit a legacy
  cleanup extrinsic or migration. If B ships later and stale Closed entries have
  accumulated on the new network, add a bounded repair path justified by that
  state. An explicit signed cleanup call processing at most the bounded index
  length is the preferred fallback; benchmark its large historical-order reads.
- Benchmark normal closure at the maximum index length and update settlement/
  refund weights and proof envelopes for the additional index write. Avoid an
  unbounded scan or migration across all orders.
- Update SDK enumeration semantics: `ProposerOrders` is an outstanding-order
  index; history requires events or a separate historical query.

Acceptance: 32 outstanding orders reject the 33rd; closing one frees a slot;
more than 32 sequential completed orders work; failed payout/refund frees no
slot; fresh-genesis proposer indexes are empty. If a post-reset repair path is
needed, test that it removes only Closed entries and preserves active/expired
orders.

Permissionless settlement does not guarantee eventual closure if payment remains
impossible. When an authorised balance lock/freeze can be removed, its owner or
authority must resolve that restriction and any signed caller can retry settlement.
Keep the slot occupied while funds remain owed; cleanup must not discard a solved
order or allow the proposer to refund winners' funds. The user selected retention
of both the slot and funds, including when payment remains impossible: this plan
adds no forced-close or forced-refund mechanism. Recovery means resolving the
underlying payment restriction and retrying; it does not guarantee eventual
closure. Document this capacity limit for Trustee operators and test successful
retry after a removable restriction and slot retention while it persists.

## C. Specify what the Trustee observes and signs

Deliver a chain-agnostic protocol specification covering points 4, 6 and 8,
explicitly distinguishing Quip-local EVM and external-EVM deployment possibilities.
Define job/reference encoding, destination requirements, attestation contents and
domain separation, observation and retry rules, and the interfaces expected of
the Trustee and EVM contract. Review those interfaces with the EVM/toolchain
owners; do not select a particular EVM scope/network or build the bridge in this
work. Specify common invariants and mark the local/external integration paths and
reward execution/closure rules as unresolved where they depend on that choice.

The initial EVM network, its source-event finality policy, and escrow timelock,
expiry and refund settings are explicitly deferred deployment decisions. The
specification must identify those required settings and their consistency
constraints. External jobs must not launch until they are supplied and validated;
there are no implicit production defaults. Deferring deployment settings does
not defer completion of C's specification before the runtime 119 reset.

### Finality and evidence

- Observe a GRANDPA-finalised block containing actual order finalisation.
  Passing the nominal expiry height is insufficient when the bounded expiry
  hook has a backlog. Require the finalised state to be Expired or Closed and
  establish that its allocation was finalised. Use `OrderExpired`, emitted by
  hook finalisation for every delivery mode, as the common discovery event;
  verify the associated state at that same finalised block hash.
- Read order, rankings, solutions and any payee data at the same block hash.
  `ResultReady` depends on callback mode; `OrderResults` exists only for
  `CallbackWithPoll`. Do not require either as a universal discovery mechanism.
- Account for result retention: `purge_result` may remove `OrderResults` after
  `settled_at + ResultTtlBlocks`. Capture durable evidence within that window or
  reconstruct the allocation from A's consistent state at an archived finalised
  block. A late observer must not assume poll results still exist; test recovery
  after purge and reject attestation if the required evidence cannot be recovered.
- Keep computed allocation separate from native reward payment. With A deployed,
  attestation need not wait for `RewardClaimed` merely to establish the result.
  Existing native orders reserve at least `MinReward` and pay natively; proposers
  must fund those rewards and fees and settle/refund to free their native-order
  slots. The funding and closure model for EVM-facing orders is explicitly
  deferred, including whether native settlement relates to EVM fulfilment.
  Never treat a native `OrderClosed` event or recorded payee as proof of EVM payment.
- Record the block hash, genesis identity, deployment/reset epoch, supported
  runtime and storage versions, and evidence needed to audit the attestation.
  Runtime upgrades or wipes require an explicit compatibility policy. Reject
  unsupported or missing evidence; never confuse reused order IDs across resets.
- For runtime 119, initialise a new observation cursor and order mapping for the
  reset network. Bind attestations and EVM verification to its genesis identity
  and an explicit deployment/reset domain; a reused chain ID or spec version is
  not sufficient isolation. Preserve external-job/payment deduplication history
  across the cutover, reconcile in-flight work, and reject pre-reset evidence.

### Correlation and retries

Decided correlation mechanism, when a contract-originated job is admitted:
atomically submit
`Utility.batch_all([System.remark_with_event(versioned_external_key), propose_job])`
and associate `JobProposed` from the same successful extrinsic with the external
job. Include source chain ID, contract and job ID in the key; retain source log
identity separately for deduplication. Do not overload the callback endpoint.
Keep the full external-job reference in the remark/Trustee mapping for now;
do not add a duplicate external-job-ID storage field to the pallet.

`remark_with_event` emits a hash rather than the raw bytes. Persist the signed
extrinsic and decoded key, or require archive access sufficient to recover them.
Define a durable state machine/outbox and reconcile inclusion and nonce before
retrying after a crash. A batch is atomic, but blindly retrying it can still create
two jobs. Define how the Trustee applies a configured source-EVM finality policy,
handles reorgs and fulfils idempotently on the EVM side. Leave the chain-specific
finality parameters to deployment configuration.

Acceptance scenarios: restart after each submission stage, duplicate logs,
failed batches, uncertain inclusion, source-EVM and pre-GRANDPA reorgs, expiry
backlog, upgrades and chain resets. Neither jobs nor external payments duplicate.
Include a runtime 119 cutover test with reused Quip order IDs, a pending old
external job, and an already fulfilled external job. Neither may be silently
resubmitted or paid again under the new network domain.

### Limits of the claim

For the current NoOpVM/Ising path, attest validation of the supplied spins and
energy under the supported chain rules. Do not describe this as proof of XQVM
program execution, particular hardware, or exclusive original work. Miner type
remains self-declared, as accepted in the reply. Hardware attestation is outside
this plan.

## D. Bind an external winner payee to the accepted solution

Implement the deployment-independent part of point 7: the winner payee accompanies
the signed solution. This binds metadata; it does not execute an EVM payout or
decide local-EVM versus external-EVM scope. Recommended API:
append a new `submit_solution_with_payee` call while retaining the existing native
submission call and its index. Share internal validation. Store a versioned
destination `(chain_id, H160)` per `(order, solver)` without rewriting existing
solution encodings unnecessarily. If D is included at the reset, initialise its
storage at genesis; no pre-reset payee backfill is needed. If D ships later,
define eligibility for existing post-reset orders and migrate only if the chosen
encoding requires it.

- The Quip solver's signature authorises the destination for that order and
  submission. Validate supported destination chains and address policy.
- Update payee, payload and ranking atomically under A's acceptance policy.
  Rejected equal/worse submissions cannot redirect payment. Reject destination
  changes once effective expiry is reached, including while the expiry hook has
  a backlog; snapshot the destination at hook finalisation into the finalised
  Trustee evidence.
  Document the consequence in SDK/miner guidance: a syntactically valid but
  mistyped payee cannot be corrected through an equal-score resubmission. Changing
  it requires a strictly better accepted solution before expiry. There is no
  independent payee-edit call in this plan; clients should clearly display and
  confirm the destination before signing.
- Preserve existing native payout behaviour. An order requiring an EVM payee
  must explicitly record its required destination chain at proposal time,
  immutably for the order's lifetime. This is a payee requirement, not an activated
  EVM-payment mode, and is separate from the external
  job reference carried by the remark. Specify its encoding and proposal API
  alongside C; do not infer an enforceable runtime policy from remark events.
  Use an appended proposal call for explicit payee requirements if needed, without
  assigning it EVM funding or settlement semantics. Preserve the
  argument encoding of existing proposal and submission calls; do not add a
  trailing parameter to an existing SCALE-encoded call as a compatibility shortcut.
- Defer EVM-payment activation and any deployment registry/admission mechanism
  that depends on local-versus-external scope or the reward model. Recording a
  payee, or using a native order that requires one, must not activate EVM payment.
  C must state that actual EVM job admission requires the unresolved decisions
  and validated deployment settings first; do not build an external-only registry
  as an assumed requirement for this independent implementation.
- Reject a solution for an order requiring an EVM payee when its payee is absent,
  invalid or for a different destination chain. Apply this rule through every
  submission entry point, including the existing native call, so it cannot be
  bypassed. Native-only orders retain their existing submission behavior.
- Record the Quip solver's authorised winner payee only. Solver/program royalty
  recipients, rates, funding and payment logic are deferred to a separate future
  specification; they are not prerequisites for A–D or runtime 119.
- Retain accepted payee records with the historical order/solution evidence after
  closure; pruning `ProposerOrders` must not delete them. No new payee TTL or
  pruning mechanism is included. Any future retention change must preserve C's
  evidence/archival requirements and define late-observer recovery explicitly.

Acceptance: authenticated destination binding, no redirect after expiry, rejected
resubmission leaves the destination untouched, all selected Top-N winners have
the required destinations, missing/invalid/wrong-chain payees fail at submission
without changing ranking or stored payload, the legacy call cannot bypass the
payee-requirement rule, and legacy native clients remain compatible on native-only
orders. Benchmark the new storage and calls.
Test that payee metadata does not alter native reward amounts, funding or closure,
and that it makes no claim to execute or confirm an EVM transfer. Explicitly list
EVM-payment activation and reward/closure decisions as deferred integration work.

[QUI-1606](https://linear.app/quip-network/issue/QUI-1606/featxqsa-payout-rules-for-solverquip-job-orders)
covers SDK payout-rule classes; it is not a substitute for this payee protocol.

## Delivery and review gates

1. Finish the existing point-1 branch validation for the runtime 119 reset release.
2. Implement A before the reset; validate fresh-genesis invariants and the
   SDK regression.
3. Implement B before the reset, with closure pruning and updated weights; add
   bounded post-reset repair only if actual post-reset state requires it.
4. Complete C's chain-agnostic Trustee/EVM interaction specification before the
   reset; D's API design can proceed alongside it. Selecting and configuring an
   external-EVM deployment is separate work and does not gate this specification.
5. Implement D's independent signed winner-payee support before the reset, with
   mandatory matching-chain payees for orders explicitly requiring them. Defer
   EVM-payment activation and any work requiring the unresolved EVM scope or
   reward/closure model. Royalties remain deferred.

`coder` implements the authorised independent work one commit at a time: first
commit this updated plan, then A, B, C and D as focused commits (split further
where useful). Validate each step and report its commit SHA before moving on;
no per-commit human approval is required. `reviewer` and `rr` independently review
the resulting commits and evidence, and `coder` addresses actionable findings.
Preserve pallet/call indices,
append new calls, and initialise new storage correctly at reset genesis. Do not
add pre-reset state migrations for this launch; evaluate migrations for later
in-place upgrades against the actual post-reset state. Follow the
repository rule for runtime version bumps: keep 119 while unshipped; reassess
against the actual deployed release at implementation time. Regenerate signing
fixtures if versions or signed extensions change.

For A/B/D, reassess the storage footprint of claim and refund paths, recompute
their proof envelopes, update affected assertions (including the current
`996_800` claim-envelope assertion), and verify the maintained 1 MiB claim floor
and dispatch limits still cover the result. Include larger order encodings and
B's proposer-index write where applicable; include payee storage if the measured
path accesses it. Do not assume today's envelope remains valid or count storage
that the path never touches. Regenerate affected reference weights before merge.

Each runtime MR needs targeted correctness tests, workspace lint/test checks,
relevant benchmark feature/preflight checks, Wasm CI and reference weight
regeneration when execution or storage access changes. Record remaining gates
explicitly rather than presenting local native tests as production validation.
For the current point-1 branch, fresh reference timing for the enlarged settlement
benchmark is a merge gate as well as a release gate: main's retained timing does
not measure this benchmark's maximum-size order and weighted payout setup.

Tracking still needed: dedicated issues for B (point 5), C's observation and
correlation protocol (points 4/6/8), and D (point 7). Link those to QUI-1512 and
record these decisions and the remaining protocol specification work when
implementation is scheduled.
Existing QUI-1408/QUI-1675/QUI-1676 cover A and its SDK verification.

The settled decisions and explicit EVM scope/reward deferral are recorded above.
Do not ask for the deferred decision again during independent implementation.
Encoding,
storage layout, API details and test implementation remain engineering work for
`coder`, reviewed by `reviewer` and `rr`; they do not need another product decision
unless implementation reveals a conflict with the agreed behavior.

The Quip-chain precompile (QUI-999), XQVM-on-chain (QUI-1475), ZK proofs (QUI-1134),
hardware attestation, royalties, external-network selection/deployment settings,
and building the full Trustee/EVM application are separate workstreams. This
document records needed tracker issues but does not itself create them.


## Implementation handoff, 2026-10-11

| Commit | Delivered scope |
| --- | --- |
| `8716519` | User decisions, reset boundary and EVM scope/reward deferral. |
| `eb34061` | A: strictly improving replacements and deterministic best-anchored subset. |
| `89e2d74` | B: outstanding proposer slots, atomic closure pruning and reserve-failure rollback. |
| `19bcacd` | C: [chain-agnostic correlation/evidence protocol](quip-evm-compute-interaction.md). |
| `af51d6d` | D: appended signed metadata APIs, all-path requirements, expiry freeze and retained evidence. |

Runtime remains spec 119 / transaction 7. Existing calls and order/solution
encodings are preserved. Sparse payee maps use storage version 4. No pre-reset
state import, payee inference, activation registry, EVM execution or native
reward/closure changes were added.

Local validation on `af51d6d`: 478 workspace tests passed (two existing ignored
PoW diagnostic/sweep tests), 95 mempool tests with runtime-benchmarks + try-runtime
passed, workspace/all-target strict Clippy and feature strict Clippy passed.
The workspace total includes 56 runtime unit tests and signing/metadata
integration tests. Formatting, diff checks, shell syntax, signature helpers,
weight-output resolver and public-wrapper guards passed. A real H4-signed atomic
batch verifies both correlation rollback on mismatched payee and successful
binding to the actual signer. Native amounts and retained metadata are covered
across all payout modes, expiry backlog, closure and polling-result purge.

Production proof unions are 1,002,055 bytes for claim, 666,169 for refund,
792,516 for submission and 686,322 for proposal. All four families use maintained
1 MiB floors, cover both weight providers, preserve larger generated proofs, and
pass Normal max_extrinsic/max_total integrity checks. Metadata is not accessed
by closure, so its keys are not included in closure envelopes.

The exact-commit release runtime-benchmarks Wasm build passed locally. Blob
SHA-256: `219cfea9ac30f8192966fb4b1e6c0e61010b8279fb26f22c435504800b254b66`.
All **13 mempool benchmarks**, including extra replacement and dust cases, passed
against this blob through the local debug runner with `--steps 2 --repeat 1
--min-duration 0 --extra --genesis-builder runtime`. Component endpoints include
production maxima of 5,000 nodes / 50,000 edges / 20 solutions and 16 bid miners /
8 types. Observed DB counts match the modeled bounds: first submission 6/7,
replacement 6/5, claim including dust 69/69, refund 2/3, and required-payee proposal
3/7 (reads/writes, with benchmark whitelists). Outputs were temporary only.

Remaining gates: independent reviewer/rr review; CI Wasm/preflight checks; fresh
reference-machine timing/weight regeneration for affected proposal, submission
(first and replacement) and closure (ordinary and dust) paths; post-deployment
QUI-1676 SDK regression. Local benchmark execution cannot substitute for reference
measurements. Retained baseline timing and new-call compositions are provisional.

Deferred product/integration work remains exactly the agreed local/external EVM
scope, native/EVM reward relationship, EVM payment activation/execution and closure,
network/contract deployment, finality/escrow/refund settings, trust/signing policy,
archive and reset cutover, and cross-language protocol codec/hash fixtures in the
future consuming integration. Royalties and the full Trustee/bridge application
are separate work. No push, merge or manual CI job was performed for this handoff.
