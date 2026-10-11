# QUI-1512 follow-up plan

Status: proposed implementation sequence, 2026-10-11. This records remaining
chain-side work for the external-EVM compute MVP. It does not authorize or claim
completion of the Trustee or EVM contract implementation.

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

| Original point | Treatment |
| --- | --- |
| 1. Settlement liveness | Existing branch; integrate and validate first. |
| 2. Ranked score can outlive its payload | Follow-up A, before Trustee attestation. |
| 3. Stored subset can omit the best vector | Follow-up A, together with point 2. |
| 4. Final enough to attest | Follow-up C: finalised, consistent snapshots and reset policy. |
| 5. Lifetime proposer cap | Follow-up B: bounded outstanding-order index. |
| 6. External job correlation | Follow-up C: agree and specify a versioned protocol. |
| 7. EVM winner payout address | Follow-up D: signed per-submission payee. |
| 8. Hardware/work claims | Document current limits in C; self-declared miner type remains accepted. |

## A. Keep ranked scores and stored solutions consistent

Implement [QUI-1408](https://linear.app/quip-network/issue/QUI-1408/verify-reward-settlement-against-overwritten-solver-results)
and [QUI-1675](https://linear.app/quip-network/issue/QUI-1675/fixmempool-store-the-vector-best-energy-milli-is-computed-from)
together. GRANDPA finality makes a state durable; it cannot repair an internally
inconsistent ranking and payload.

Recommended submission policy:

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

Audit existing orders before enabling Trustee attestations. An overwritten best
vector cannot be recovered by waiting for finality or rescoring the remaining
payload. Specify repair or exclusion for legacy inconsistent orders; do not
silently claim the new invariant holds retrospectively.

After deployment, run the SDK regression in
[QUI-1676](https://linear.app/quip-network/issue/QUI-1676/testxqsa-recheck-the-leader-pick-once-the-mempool-stores-its-ranked)
and verify `leader_energy_matches_chain=True`. Retain compatibility handling for
legacy inconsistent orders until both fixes and the legacy-state policy ship.

## B. Make the proposer cap bound outstanding orders

Keep the existing production value `MaxOrdersPerProposer = 32`, but redefine it
to limit orders not yet Closed, including expired orders awaiting settlement or
refund. This changes index semantics, not the constant; the mock runtime uses 8.
It must not limit the number a proposer can create over its lifetime.

- Remove an order from `ProposerOrders` only after successful settlement or
  refund, in the same transaction. Preserve historical `JobOrders` records.
- Add a bounded cleanup path for stale Closed entries already in a proposer's
  index. Prefer an explicit signed cleanup call that processes at most the
  bounded index length, with a new call index; this makes its potentially large
  storage-proof cost visible instead of hiding it in every proposal.
- Benchmark the worst case, including reading large historical order values.
  Update settlement/refund weights and proof envelopes for the additional index
  write. Avoid an unbounded scan or migration across all orders.
- Update SDK enumeration semantics: `ProposerOrders` is an outstanding-order
  index; history requires events or a separate historical query.

Acceptance: 32 outstanding orders reject the 33rd; closing one frees a slot;
more than 32 sequential completed orders work; failed payout/refund frees no
slot; cleanup removes only Closed entries and preserves active/expired orders.

Permissionless settlement does not guarantee eventual closure if payment remains
impossible. When an authorised balance lock/freeze can be removed, its owner or
authority must resolve that restriction and any signed caller can retry settlement.
Keep the slot occupied while funds remain owed; cleanup must not discard a solved
order or allow the proposer to refund winners' funds. Permanently unpayable orders
need an explicit recovery/governance policy before relying on this cap for an
unattended Trustee. That policy remains an open design decision, not a guarantee
provided by this cleanup change. Test successful retry after a removable restriction
and retention of the slot when the restriction persists.

## C. Specify what the Trustee observes and signs

Agree the protocol with the EVM/toolchain owners before implementing the bridge.
This follow-up covers points 4, 6 and 8.

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
- Record the block hash, genesis identity, deployment/reset epoch, supported
  runtime and storage versions, and evidence needed to audit the attestation.
  Runtime upgrades or wipes require an explicit compatibility policy. Reject
  unsupported or missing evidence; never confuse reused order IDs across resets.

### Correlation and retries

Recommended MVP, still requiring agreement: atomically submit
`Utility.batch_all([System.remark_with_event(versioned_external_key), propose_job])`
and associate `JobProposed` from the same successful extrinsic with the external
job. Include source chain ID, contract and job ID in the key; retain source log
identity separately for deduplication. Do not overload the callback endpoint.

`remark_with_event` emits a hash rather than the raw bytes. Persist the signed
extrinsic and decoded key, or require archive access sufficient to recover them.
Define a durable state machine/outbox and reconcile inclusion and nonce before
retrying after a crash. A batch is atomic, but blindly retrying it can still create
two jobs. Define source-EVM finality, reorg handling and idempotent EVM fulfilment.

Acceptance scenarios: restart after each submission stage, duplicate logs,
failed batches, uncertain inclusion, source-EVM and pre-GRANDPA reorgs, expiry
backlog, upgrades and chain resets. Neither jobs nor external payments duplicate.

### Limits of the claim

For the current NoOpVM/Ising path, attest validation of the supplied spins and
energy under the supported chain rules. Do not describe this as proof of XQVM
program execution, particular hardware, or exclusive original work. Miner type
remains self-declared, as accepted in the reply. Hardware attestation is outside
this plan.

## D. Bind an external winner payee to the accepted solution

Point 7 calls for the winner payee to accompany the solution. Recommended API:
append a new `submit_solution_with_payee` call while retaining the existing native
submission call and its index. Share internal validation. Store a versioned
destination `(chain_id, H160)` per `(order, solver)` without rewriting existing
solution encodings unless a migration is deliberately selected.

- The Quip solver's signature authorises the destination for that order and
  submission. Validate supported destination chains and address policy.
- Update payee, payload and ranking atomically under A's acceptance policy.
  Rejected equal/worse submissions cannot redirect payment. Reject destination
  changes once effective expiry is reached, including while the expiry hook has
  a backlog; snapshot the destination at hook finalisation into the finalised
  Trustee evidence.
- Preserve native payout behaviour. Define external-job enrolment/eligibility
  with C so native-only submissions do not produce an unpayable external winner.
  Missing, invalid or wrong-chain destinations must block external fulfilment
  under an explicit recovery/refund policy, rather than choose an implicit payee.
- Treat a deployed solver/program's royalty payee separately from the winner's
  destination. Agree the royalty mechanism with the EVM owners; do not invent
  rates or payment rights in this runtime change.

Acceptance: authenticated destination binding, no redirect after expiry, rejected
resubmission leaves the destination untouched, all selected Top-N winners have
the required destinations, missing/invalid destinations fail safely, and legacy
native clients remain compatible. Benchmark the new storage and call.

[QUI-1606](https://linear.app/quip-network/issue/QUI-1606/featxqsa-payout-rules-for-solverquip-job-orders)
covers SDK payout-rule classes; it is not a substitute for this payee protocol.

## Delivery and review gates

1. Finish the existing point-1 branch integration and validation on current main.
2. Implement A, then validate the SDK regression and legacy-state policy.
3. Implement B with bounded cleanup and updated weights.
4. Agree C's Trustee/EVM protocol; D's API design can proceed alongside it.
5. Implement D after its destination and external-job eligibility rules are agreed.

Use separate, reviewable MRs. `coder` implements changes; `reviewer` and `rr`
independently review the resulting diff and evidence. Preserve pallet/call indices,
append new calls, and version/migrate changed storage when necessary. Follow the
repository rule for runtime version bumps: keep 119 while unshipped; reassess
against the actual deployed release at implementation time. Regenerate signing
fixtures if versions or signed extensions change.

Each runtime MR needs targeted correctness tests, workspace lint/test checks,
relevant benchmark feature/preflight checks, Wasm CI and reference weight
regeneration when execution or storage access changes. Record remaining gates
explicitly rather than presenting local native tests as production validation.
For the current point-1 branch, fresh reference timing for the enlarged settlement
benchmark is a merge gate as well as a release gate: main's retained timing does
not measure this benchmark's maximum-size order and weighted payout setup.

Tracking still needed: dedicated issues for B (point 5), C's observation and
correlation protocol (points 4/6/8), and D (point 7). Link those to QUI-1512 and
record their unresolved recovery/protocol choices when implementation is scheduled.
Existing QUI-1408/QUI-1675/QUI-1676 cover A and its SDK verification.

The Quip-chain precompile (QUI-999), XQVM-on-chain (QUI-1475), ZK proofs (QUI-1134),
hardware attestation, and building the full Trustee/EVM application are separate
workstreams. No new tracker issues are created by this document.
