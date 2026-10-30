# W3 slashing operations

This work introduces automatic BABE/GRANDPA equivocation reports and a bounded
council discretionary slash route. It is merged and reviewed, but runtime
versions and release activation are owned by the integration release (W6), so
mainnet deployment still waits on the gates listed at the end of this document.

## What a report does

Nodes generate a historical key-ownership proof and submit unsigned equivocation
evidence through the consensus runtime APIs. The runtime checks actual hybrid
signatures and records accepted evidence in `Offences::Reports`. Staking uses
its stored exposure for the offence era; the zero exposure carried by
`DefaultExposureOf` in a membership proof is an identification compatibility
value, not the amount at risk.

`ReportLongevity = 403200` is transaction-pool validity/revalidation in blocks.
It does not guarantee that a proof remains actionable for that period. Historical
roots, bonded-era retention, and the report acceptance policy determine that.

`SlashDeferDuration` is fixed at 27 eras. The SDK schedules for
`offence_era + 28`, not 27 eras after the report. Reporters near that deadline
leave less time for appeal. Changing deferral while entries are pending also
changes the SDK's unlocking-chunk eligibility calculation at application time;
do not turn it into a mutable parameter.

The effective report-age window is **27 eras**: offences as old as
`active_era - 27` may still be reported. Calling it a 28-era reporting window is
incorrect; the 28th-era report is rejected. The oldest bonded era at age 28
would target a queue already processed at the
start of the current era. W3 uses one predicate for API preflight and the
authoritative offence-reporting guard. The latter runs before the historical
filter or any deduplication or maximum/span accounting is written. The effective
deadline is before the application era starts. APIs refuse age-28 submissions
with `None`; direct signed/unsigned report calls cannot bypass the reporter guard.

SDK `check_evidence` checks proof ownership and deduplication, not this age bound.
A direct report can therefore occupy pool space, and a report submitted at age 27
can cross the boundary before inclusion. `Some(())` confirms submission only.
Dispatch of a late report fails with the SDK consensus pallet's generic
`DuplicateOffenceReport` error, which also covers reporter rejection; it does not
prove an earlier report exists. No slash state is written on rejection.

## Council discretionary slash

`SlashPolicy::discretionary_slash(stash, era, fraction)` checks active-era
bounds and nonzero stored exposure. A conservative additive fraction budget
counts accepted motions per stash over the configured window. It includes
cancelled slashes and retains the maximum supported history, so increasing the
window does not forget older motions. Parameters cannot raise the immutable
runtime ceiling or shorten the window below the bonding duration.

No single council motion can exceed the **configured** cumulative limit for any
stash within the rolling window (default 4%, never above the immutable 10%
ceiling), and no single dispatch can exceed the configured per-dispatch limit
(default 1%, never above the immutable 5% ceiling). Raising a configured limit
is a council decision within those ceilings; raising a ceiling requires a
runtime code upgrade.

The per-dispatch limit (`MaxDiscretionarySlashPerMotion`) defaults to 1% and can be set within 0.1%–5%. The
cumulative limit defaults to 4% and can be set within 1%–10%; it must be at least
the per-dispatch limit. The window defaults to 28 eras and may be increased to 84.
History is retained for 84 eras even when the selected window is shorter.
Each stash has a bounded vector with at most one entry per era. Entries outside
the configured window stop counting toward the budget; entries older than 84
eras are pruned on the next successful slash. Inactive stash keys are not swept.
`set_limits` changes all three values atomically within those rails. The
per-dispatch limit is checked on each `discretionary_slash` dispatch; a council
proposal batching several such calls still shares the per-stash cumulative
budget. Do not interpret the 1% default as a separate aggregate cap on an
arbitrary batch.

With the default settings, four successful 1% dispatches for one stash consume
its 4% budget, including when batched into one proposal; a fifth is rejected.
Each must independently increase staking's slash maximum: repeating the same
fraction for the same offence era can be a no-op and is rolled back instead.
Council can raise configured limits through `set_limits` within the 5% per-dispatch
and 10% cumulative hard ceilings. Changing those ceilings requires new runtime code.

A `SlashScheduled` event includes remaining budget and means a new staking queue
entry contains a positive validator or nominator amount. It does not mean funds
have already been burned. The call is rolled back unless it adds a non-zero queue
entry. A repeat on another era of an already-slashed span can add a zero-value
entry; that call is refused without emitting `SlashScheduled` or charging budget.

Direct `Staking::manual_slash` is Root-only. The council Root-dispatch entry point
rejects it, including nested Utility, Proxy, and Multisig calls. Root remains available to runtime
upgrade logic. Automatic consensus fractions are independent of the council
budget and may reach 100% of the relevant exposure.

The Root-dispatch deny-list rejects `Staking::manual_slash` and
`System::{set_storage,kill_storage,kill_prefix}` at every inspected depth.
Depth above eight produces `RootCallTooDeep`; more than 256 inspected calls
produces `RootCallTooLarge`. Policy-denied leaves produce `RootCallDenied`.
Presets, node wiring, and the governance matrix have no raw-storage dependency.

Changing these hard limits requires a **runtime code upgrade**. Governance is
expected to use a **spec-versioned** upgrade; the explicitly allowed emergency
paths (`set_code_without_checks`, `authorize_upgrade_without_checks`) do not
enforce a version bump, and using them for parameter changes is a process
violation, not a supported route.

The unchecked upgrade path is reserved for emergency halt/recovery; using it
to change a capped parameter is an incident. If no version increase is supplied,
the audit trail shows an unchecked code swap with no spec bump, and migration
hooks can be skipped. The filter enforces a code-change boundary; the versioning
and emergency-use requirements are governance policy.

## Appeal, application, and removal

1. Inspect `Staking::UnappliedSlashes[application_era]`, not the offence-era key.
   Here `application_era = offence_era + SlashDeferDuration + 1 = offence_era + 28`.
2. Council 2/3 submits `ValidatorAdmission::dispatch_as_root` containing
   `Staking::cancel_deferred_slash(application_era, indices)` before that era
   starts. Indices must be nonempty, unique, sorted, and refer to the current
   queue. Earlier cancellations shift later positions; inspect the queue again.
3. Cancellation removes pending entries. It does not undo applied slashes,
   restore maximum/span accounting, remove offence deduplication, or refund the
   discretionary budget. Re-reporting the same offence does not recreate it.
4. At the application era, confirm the queue was consumed and inspect staking
   slash events and issuance. `Slash = ()` burns the residual slashed imbalance;
   reporter rewards, if configured, are paid from that imbalance first.
5. A slash does **not** revoke admission or chill the validator in this SDK.
   Ejection is a two-call council motion using `Utility::batch_all` with the
   council origin: first `ValidatorAdmission::remove(stash)`, then
   `ValidatorAdmission::dispatch_as_root(Staking::force_new_era())`.
   Removal requires the council origin, so do not put the entire batch inside
   Root dispatch. Check the Root-dispatch receipt as well as batch success.
   Removal excludes the stash from future elections; forcing an era requests
   that transition and does not immediately disable BABE authoring.
   `force_unstake` is separate: it removes staking state immediately, not a
   monetary penalty, and can affect a pending slash's ability to collect funds.

BABE's `DisabledValidators = ()` remains unchanged (QUI-2068). Do not rely on W3
for immediate BABE authoring exclusion. The offence penalty is the deferred
slash; removal from admission affects subsequent elections.

## Launch operations

A payout bot is required from launch (QUI-2065). Staking rewards are entitlements
until claimed, not immediate issuance. Claims expire after `HistoryDepth = 84`
eras; unclaimed rewards are never minted. Missing claims therefore reduce
realised APR below the 10M/year staking budget. Monitor outstanding claims and
alert before their retention deadline.

Also alert when an election is finalised through the on-chain fallback rather
than a queued multi-block election solution (W2, QUI-2065); a finalised epoch
alone does not show that the expected election pipeline operated.

The dedicated `SlashPolicy` pallet owns only slash policy and its budget ledger.
Deferral remains a compile-time constant because changing it with pending
slashes is unsafe. Quantum-pow reward and round-length settings remain in their
own pallet; they are not moved into this ledger.

## Release checks

Use the in-place migration to initialize Offences' on-chain storage version to
1. It is idempotent and preserves existing reports. Existing pallet/call indices
and signing versions remain unchanged in this work.

Regenerate the new pallet's conservative initial weights and the admission
filter overhead on the serialized reference runner before release. Run the
runtime API evidence tests, deferred application/burn test, boundary regression,
council bypass tests, pallet budget tests, benchmark preflight, no_std build,
try-runtime checks, clippy, and workspace tests against the final design.
