# Quip compute interaction protocol, version 1

This is the chain-agnostic specification for QUI-1512 C, with the independent
payee API for D. It defines observation, correlation and evidence requirements;
it is not a deployed Trustee, bridge, EVM contract or payment system.

## Scope and admission gate

“EVM” may be the EVM attached to the Quip Substrate runtime, a separate EVM
network, or both. No deployment choice is made here. The relation between native
Quip rewards and EVM rewards (native only, EVM only, or both), funding, execution
and closure remains unresolved. Do not infer a reward split, exemption, offset,
EVM transfer or EVM payment finality from any native pallet event or payee record.

Current native proposal calls still reserve at least `MinReward`; a funded
proposer also pays transaction fees. Native settlement/refund frees its bounded
outstanding-order slot only on success. The independent destination requirement
is metadata validation, not payment activation. No external-only activation
registry is assumed or implemented.

A deployment MUST settle and validate these items before admitting EVM-facing
jobs: local/external scope; network and contract identities; source finality and
reorg policy; native/EVM funding and reward relationship; EVM execution and
closure; escrow deadlines and refund arbitration; signer/Trustee trust and key
rotation; contract replay protection; archive/evidence availability; reset domain
and in-flight cutover policy. There are no production defaults. This gate does
not prevent ordinary native use of destination metadata.

## Native APIs and encoding contract

Existing call argument encodings and indices remain unchanged: proposal at 3,
submission at 4, claim at 5, refund at 6 and result purge at 7. D appends:

- `propose_job_with_payee_requirement` (8): the same native proposal arguments,
  followed by `required_chain_id: u64`. Nonzero chain IDs represent destination
  metadata; the proposer cannot change the requirement after creation.
- `submit_solution_with_payee` (9): `order_id: u64`, bounded solution vectors,
  and `WinnerPayee::V1 { chain_id: u64, address: H160 }`. SCALE encodes V1 as
  variant byte 0, chain ID as eight little-endian bytes, and the address as 20
  bytes. Zero chain IDs and the zero address are invalid. Support is syntactic
  for nonzero u64 chain IDs, not a claim that any network is deployed/approved.

A required order rejects missing or mismatching destinations through EVERY
submission entry point, including call 4. A signed solver may replace its
payload/payee only with a strictly better accepted energy before effective
expiry. Equal/worse attempts cannot correct a mistyped destination. No separate
payee-edit call exists. Clients should display chain ID and address before signing.

Native orders without a requirement accept the original submission API. The
new API may attach optional metadata; a later accepted legacy submission on such
an order clears its optional payee rather than attributing old metadata to a new
payload. `SolutionPayeeSet` is emitted only for an accepted metadata change:
addition, changed destination or clearing. None-to-None and identical-payee
replacements are silent for this event. A rejected submission leaves all prior
evidence and accepted-submission events unchanged. Accepted
payees remain queryable after native closure. A future retention policy must
specify archival/late-observer recovery before pruning evidence.

New maps store `OrderPayeeRequirements[order_id]` and
`OrderPayees[order_id, solver]`; existing JobOrder and JobSolution SCALE layouts
are unchanged. Requirements and accepted payees are immutable after effective
expiry, even during hook backlog. The finalised block's state is the destination
snapshot: the Trustee captures its exact values alongside the solutions and
allocation; no later “latest state” lookup may substitute for it.

Deployment-domain administration is deferred integration work: the authority that
assigns source/consumer/reset domains, authenticates and publishes their mappings,
and authorises rotations/cutovers must be specified before activation. Consumers
and Trustees must authenticate the same configuration; an untrusted remark cannot
establish it. Unknown or unauthenticated domains block admission/issuance. This
specification does not introduce an on-chain domain registry or choose its operator.

## Versioned job correlation

The Trustee submits exactly one successful atomic batch:

```text
Utility.batch_all([
  System.remark_with_event(job_reference_v1),
  QuantumComputeMempool.propose_job[_with_payee_requirement](...)
])
```

Choose the proposal call according to the explicit metadata requirement, never
by interpreting the remark inside the runtime. A remark alone does not enforce
eligibility, destination policy, uniqueness, funding or EVM admission.

`job_reference_v1` is this 169-byte, fixed-width encoding, in order:

| Field | Encoding |
| --- | --- |
| Domain tag | 12 ASCII bytes `QUIP/QVM/job` |
| Protocol version | u8 = 1 |
| Source chain ID | nonzero u64, little endian |
| Source deployment domain | 32 bytes, configured and immutable |
| Source contract | 20 address bytes |
| Source job ID | 32 bytes, canonical contract-adapter identifier |
| Quip genesis hash | 32 bytes |
| Quip reset/deployment domain | 32 bytes, configured and immutable |

The contract adapter MUST define a one-to-one mapping to the 32-byte source job
ID; it must not truncate or hash ambiguous textual identifiers. An integer ID is
unsigned big-endian padded to 32 bytes. Other contract schemas require an agreed
canonical adapter before admission. Source block/transaction hash and log index
are separately retained as event provenance, not an alternative business key.
The source deployment domain distinguishes contract redeployments or resets even
when chain ID, address and job ID are reused.

Verify `System.Remarked`'s runtime hash against these exact bytes, and associate
exactly one `JobProposed` from the same successful outer batch/extrinsic. Require
matching proposer and spec. Recover the full proposal parameters from the decoded
inner proposal call in the retained signed extrinsic, and crosscheck them against
`JobOrders` at the canonical finalisation block defined below. `JobProposed` alone
does not contain the full topology, timing, delivery or reward-resolution arguments
and is insufficient to establish that match. Reject failed batches,
ambiguous nested batches, multiple proposals, mismatched hashes, unsupported
versions and unconfigured domains. `remark_with_event` emits only a hash: retain
the signed extrinsic/decoded bytes or guaranteed archive retrieval. Never use a
callback URL as the job key, and do not duplicate the external job ID in pallet
storage. Native block builders do not enforce uniqueness of this business key.

## Finality and evidence

Discover all delivery modes through `OrderExpired`, emitted by `on_initialize`.
Let **B** be the actual block that emitted `OrderExpired` for the order. Wait until
B is GRANDPA-finalised, then bind the post-state and header of **that exact B**.
Nominal expiry alone is insufficient: the bounded hook may have a backlog. B's
post-state must show Expired or Closed; native settlement later in the same block
can legitimately make it Closed. Verify the event and state at B, even if the
finalised head has advanced beyond B. A later finalised block is not a substitute.

Read all evidence at this canonical finalisation block hash B:

- Genesis/reset identity, runtime spec and metadata/storage versions, block hash
  and number, and the finality proof/verification reference.
- Job reference and successful proposal provenance; order and job spec, including
  validation/transform program hashes, problem/topology and full native reward
  resolution and amount.
- Accepted solutions, ranked solvers and computed allocation. Recompute energies
  from the stored vectors and verify the ranked minimum is present. Enforce the
  supported VM/validation semantics; exclude unsupported or inconsistent evidence.
- Immutable destination requirement and each selected winner's authorised payee,
  including chain/address validation for required orders. Optional metadata must
  never become an implicit payment instruction without the deployment policy.
- The explicit distinction between allocation and any observed native payment.

`ResultReady` exists only for callback modes. `OrderResults` exists only for
CallbackWithPoll and can be purged at `settled_at + ResultTtlBlocks`; its
`settled_at` is the hook finalisation height, not proof of payment. Capture the
canonical evidence at B before it becomes unavailable, or recover that exact
post-state/header from an archive or retained verified snapshot. Recomputing the
same allocation from a later block does not recover B's evidence: status, job-spec
counters or other encoded fields may differ and change its digest. Missing canonical
evidence blocks issuance until that exact evidence is recovered. Never replace B
with a later block or use a latest partial view. Poll records are not required for
OnChainOnly discovery.

For current NoOpVM/Ising jobs, evidence establishes validation of supplied spins
and energy under the supported chain rules. It is not proof of executing an
XQVM program, particular hardware, exclusive work or solver/program royalties.
Miner hardware type remains self-declared.

## Attestation and consumer interface

A result attestation MUST bind, in a canonical versioned schema:

- Protocol version and a distinct result domain (`QUIP/QVM/result/v1`).
- Source chain/deployment/contract/job identity and provenance reference.
- Quip genesis/reset domain, canonical OrderExpired block B hash/number and order ID.
- Supported runtime/storage/validation schema versions and a content-addressed
  digest of the canonical complete evidence bundle (including transformed
  vectors, not just a claimed minimum score).
- Ordered winner records: Quip AccountId32, energy i64, native allocation u128,
  and optional/required V1 destination. Preserve rank and allocation exactly.
- The consumer deployment/verifier identity, issuance identifier and signing-key
  epoch. These prevent cross-contract and cross-key-domain reinterpretation.

The canonical result preimage is SCALE encoding of this tuple (fixed byte arrays
have no length prefix; integers are little endian; Vec lengths are SCALE compact):

```text
([u8;18] = "QUIP/QVM/result/v1", u8 = 1,
 [u8;169] = job_reference_v1, [u8;32] = finalised_block_hash,
 u32 = finalised_block_number, u64 = order_id,
 u32 = runtime_spec_version, u16 = pallet_storage_version,
 u16 = validation_schema_version, [u8;32] = evidence_digest,
 Vec<(AccountId32, i64 energy, u128 native_amount, Option<WinnerPayee>)>,
 [u8;32] = consumer_deployment_domain, u64 = signing_key_epoch,
 [u8;32] = issuance_id)
```

Winner count is at most 32, ordered by native rank, with no duplicate accounts.
`Option` uses SCALE 0/1 tags, and WinnerPayee uses the encoding above. The result
digest is BLAKE2b-256 of this preimage (32-byte output parameter, not truncated
BLAKE2b-512). The evidence digest uses the same hash over SCALE encoding of:
`(u8=1, Vec<u8> signed_proposal_extrinsic, Vec<u8> finalised_header,
Vec<u8> encoded_JobOrder, Vec<u8> encoded_JobSpec,
Vec<(AccountId32, Vec<u8> encoded_JobSolution, Option<WinnerPayee>)>,
Option<u64> required_chain_id)`. Include every accepted solver, sorted by raw
AccountId32 bytes; the header and values MUST be B's post-state/header, decoded
using the supported metadata version at B. Every retry retains B and the same
canonical evidence digest; later native settlement is never a reason to change
the evidence block. Reject duplicate/unsorted records, trailing bytes, noncanonical encodings,
unknown enum tags and inconsistent derived allocations. Finality/storage proofs
are verification attachments; their transport encoding does not alter this digest.

The deployment must select a signature/Trustee verification scheme and validate
cross-language codec/hash vectors before enabling a consumer. If an EVM signature
wrapper (for example a typed-data scheme) is chosen later, it must bind this digest
and the complete consumer domain without redefining native allocations. No
personal-message signature or native signature is implicitly an approved contract
format. Native allocation is not an EVM token amount or conversion rate; the EVM
consumer's amount/funding/closure fields are deferred.

The future consumer MUST verify the configured signer policy and all domains,
reject unsupported schemas, ensure each source business key has at most one
final outcome, and atomically couple outcome recording with any eventual payment.
Signature validity alone does not settle disputes, implement finality verification
or choose the trust model. Contract deployment and execution are separate work.

## Durable retries and recovery

Persist one durable business-key record and a write-ahead outbox BEFORE signing
or broadcasting. Store intent, full call/parameters, signed bytes, hash, signer
nonce and network/reset domain. Recommended observation states are
Observed → SourceFinal → ProposalPrepared → ProposalIncluded → ProposalFinal →
ResultFinal → EvidenceReady. Future Attested/Submitted/Fulfilled states require
the deferred consumer contract. Every transition records its causal evidence;
recovery resumes the same intent, never creates a fresh one by default. Persist
B's header/hash and verified canonical evidence digest with EvidenceReady, and
reuse them for every issuance/retry. If B's evidence cannot be recovered, keep
issuance blocked rather than taking a new snapshot at a later finalised head.

Deduplicate source events by provenance and jobs by business key. If broadcast
outcome is unknown, reconcile transaction hash, account nonce, inclusion and batch
events before replacement. Re-broadcasting identical bytes is distinct from
creating a new proposal with a new nonce. A failed/reorged inclusion may be retried
only after proving the previous proposal cannot still finalise. If that cannot
be determined, retain Pending/Uncertain and escalate; do not guess. Atomic batching
prevents half-created correlation, not duplicate jobs from blind retries.

A source reorg before configured source finality invalidates the observation.
Unexpected reorg beyond the configured policy halts admission and attestation
for reconciliation. Quip best-block reorgs do not advance finalised cursors.
Persist finalised block hashes, detect discontinuities, and resume from the last
verified cursor with idempotent processing.

Impossible native payment retains its slot/funds. Authorised unfreeze or reserve
recovery followed by retry is the supported path; no forced close/refund is
specified. The Trustee must budget for stuck outstanding slots and must not
interpret native recovery as an EVM refund or vice versa. Escrow deadlines must
cover configured finality, bounded-hook backlog, evidence retrieval and retry
margins; an unbounded outage needs an explicit deployment recovery policy.

## Runtime 119 reset and deployment cutover

A/B/D target fresh runtime 119 genesis. Start new native indexes, payees, order
IDs, observation cursor and network mapping. No pre-reset compute state is imported
or repaired by this work. A reused spec version, EIP-155 ID or order ID is not a
reset domain. Attestations bind both genesis hash and a configured reset domain,
so even a reset with identical genesis configuration cannot replay old evidence.

The local Quip EVM resets with the Substrate network. An external EVM contract
and off-chain Trustee records do not. Preserve external business-key/outcome
history across both kinds of cutover. Reconcile pending obligations under the
future agreed reward/refund policy; quarantine unresolved work. Never automatically
resubmit an old job or pay it again under a new Quip order ID/domain. Re-admission
requires an explicit cutover decision proving no duplicate outcome.

## Acceptance checklist for the future integration

| Scenario | Required outcome |
| --- | --- |
| Crash at every outbox/broadcast/inclusion transition | Reconcile same intent; no new job on uncertainty |
| Duplicate source logs or restarted cursor | One business-key mapping |
| Failed batch | No partial correlation/proposal |
| Expiry-hook backlog | Wait for finalised OrderExpired, not nominal height |
| Poll result purged / archive absent | Recover complete evidence or stop |
| Equal/worse solution with changed payee | Reject atomically; prior evidence remains |
| Deadline reached before hook drains | Every submission API rejects changes |
| Runtime reset reuses order ID; pending and fulfilled old jobs exist | New domain; no automatic resubmission or duplicate outcome |
| Local EVM reset versus external EVM continuity | Separate explicit cutover policies |
| Invalid/missing destination on required order | Reject before payload/ranking mutation |
| Native payout stuck | Keep funds/slot; no EVM outcome inferred |
| Unknown runtime, VM, key epoch or evidence codec | No attestation |

The above integration scenarios are requirements, not claims of locally executed
Trustee/EVM tests. Native pallet/runtime tests cover the on-chain invariants;
post-deployment SDK and consumer cross-language tests remain separate gates.

## Deferred integration and verification work

The unresolved local/external EVM scope and native/EVM reward relationship are
product decisions, not implicit defaults to infer while implementing native APIs.
Payment activation, admission/deployment registry policy, consumer contracts,
funding/closure semantics, source finality, escrow deadlines/refunds, Trustee trust
and signer rotation, domain administration and reset/archive cutover must be
specified and validated before EVM-facing admission. The full Trustee/bridge
application and cross-language codec/hash fixtures belong to the consuming
integration. Winner destination metadata does not specify solver/program royalty
recipients, rates or payment; royalties remain separate future work.

The Quip-chain precompile (QUI-999), XQVM execution (QUI-1475), ZK proofs
(QUI-1134) and hardware attestation are separate workstreams. Current NoOpVM
validation and self-declared miner types cannot establish those claims.

After deployment, run [QUI-1676](https://linear.app/quip-network/issue/QUI-1676)
against fresh runtime-119 orders and confirm `leader_energy_matches_chain=True`.
[QUI-1408](https://linear.app/quip-network/issue/QUI-1408) and
[QUI-1675](https://linear.app/quip-network/issue/QUI-1675) track the chain invariant;
[QUI-1606](https://linear.app/quip-network/issue/QUI-1606) covers SDK payout-rule
classes and does not replace this destination protocol. If a future change lands
after reset, audit actual post-reset state and specify only the repair/exclusion
it requires before attestation; never import pre-reset evidence into the new domain.

The native pallet still uses unnamed aggregate reserves. Per-order reserve
isolation and recovery policy remain deferred; see the
[mempool contract](quantum-compute-mempool.md). The remaining tracker inventory is
preserved in [the QUI-1512 handoff](qui-1512-follow-up-plan.md) until dedicated
issue links are recorded.
