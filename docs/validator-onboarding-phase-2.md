# Phase 2: FoundationOnly validator onboarding

Implementation base: Phase 1 `5911cfc`, on `ru/spike/validator-onboarding`.
This phase is under implementation and has not been approved for release.

## Governance and admission

Appended indices: Foundation collective 20, Foundation membership 21,
ValidatorAdmission 22, Staking 23, Historical 24. Existing indices stay fixed.
Foundation approval requires actual affirmative votes from at least two thirds
of the current collective: `ceil(2 * members / 3)` (2 of 3, or 4 of 5).
Abstentions always count as no at expiry, even when the prime voted yes or a
simple majority voted yes. Set the motion threshold to that required count;
the proposer must also explicitly vote. A lower proposal threshold can close a
motion but cannot bypass the privileged origin; inspect the inner receipt.
Membership
management accepts Root or that same collective origin. Membership synchronizes
the collective; direct `collective.set_members` is disabled to avoid two writers.
Small development/local presets bootstrap Alice as the sole Foundation member.
Fresh public-testnet genesis appoints all three existing operators. Rehearsal
appoints Alice/Bob/Charlie, assigns faucet authority to Eve, and uses their sorted
H4 transaction accounts for 2-of-3 multisig sudo. The four-validator floor remains
unchanged. Development seeds do not establish independent production custody;
see [testnet key roles](testnet-keys.md#spec-119-genesis-role-assignments).
Public-testnet genesis defaults sudo and faucet to operator 1. Before deploying
the faucet service, Foundation must use `faucetOps.set_authority` to appoint a
dedicated funded account; configure only its key in `QUIP_FAUCET_FAUCET_KEY`.
These preset changes do not migrate existing chain state.

`ValidatorAdmission.dispatch_as_root` is the explicit Root-equivalent path.
It emits the inner dispatch result; clients must check `RootDispatched.result`,
not only `System.ExtrinsicSuccess`. Root remains able to replace code or storage.
Sudo is retained until a harmless governed operation and a real governed runtime
upgrade have been rehearsed. No automatic sudo retirement is added.

Approvals are a bounded sorted set of at most 32 stashes. FixedTestnet freezes
that set. FoundationOnly allows collective-approved additions/removals. Mode
transitions cannot reverse or skip stages. PermissionlessEligibility is reserved
but graduation returns false until objective attestation/uptime/economic rules
are specified and implemented; governance cannot vote around that condition.
The future graduation trigger is permissionless (any signed caller) but always
fails its readiness check in Phase 2.

## Election and consensus

The election adapter enumerates approved stashes first, then reads staking
validator intent, registered session keys and stake. Approved candidates without
keys are excluded so an election cannot queue unusable authorities. This bounds snapshot work even when many unapproved
accounts register. Post-filtering staking's already bounded snapshot would let
unapproved intents crowd out approved candidates. Both target and self-voter
snapshots are admission constrained; normal and genesis elections use the same
Sequential Phragmen implementation. TargetList remains UseValidatorsMap and
MaxValidatorsCount remains None. The SDK constrains the election provider's
associated DataProvider to Staking; a thin provider bridge delegates actual
snapshot construction and solving to the admission-aware adapter.

Nominators remain disabled with Some(0). An initialization migration closes a
missing limit. Enabling nominators requires the later election-stack and bounds
upgrade; changing only this storage setting is not a supported activation.
Staking EraPayout is zero: the validator share of 40M/year remains unspecified.
Existing fees are not re-routed by this phase. No automatic consensus-offence
slashing is enabled; Foundation/Root staking administration is the interim path.

Historical roots are recorded from the start. Session uses Staking through
NoteHistoricalRoot; BABE uses ExternalTrigger. GRANDPA keeps 28 eras × 6
sessions of set-id history. Both consensus authority bounds and election bounds
are 32. Six sessions form an era; 28 eras is the test bonding period, not a
mainnet economic decision.

## Genesis and rollout

Fresh rehearsal genesis is the supported activation path. Session (index 12)
builds before staking/admission. Genesis therefore falls back to supplied
session keys; era-zero exposure is empty. Preset staker tuples and approved
stashes must exactly match session keys. Every validator is endowed above its
100 UNIT test bond. The first normal election populates exposures and queues
validators for the following session. Test configuration is not mainnet stake;
mainnet requires clean genesis with no faucet-funded balances.

Do not set_code this phase directly onto public testnet 20033. Preserving live
state requires a separately reviewed two-upgrade sequence: append dormant
pallets while retaining SessionManager=(), populate Foundation/admission and
operator bonds/keys, then flip consensus only after snapshot dry-run. The limit
initialization hook alone does not perform that activation. spec_version stays
119 while 119 is unshipped; Phase 2 needs the next spec if Phase 1 ships first.

## Operator runbook

1. Insert H4 BABE and H2 GRANDPA keys on the validator node. Use FN-DSA-512 hybrid
   types, not the ML-DSA alternatives.
2. Generate session keys with `author_rotateKeysWithOwner` / runtime
   `generate_session_keys(owner=stash)`; retain both `keys` and `proof` fields.
   The RPC owner is the SCALE-encoded stash AccountId, for example
   `api.createType('AccountId', stash).toHex()`, not the SS58 address text.
3. Sign `session.set_keys(keys, proof)` as the stash. KeyDeposit is zero, but the
   ownership proof is mandatory at this SDK revision.
4. Bond using `staking.bond`, then submit `staking.validate`. The Apps
   **+ Validator** flow batches `bond`, `session.setKeys(keys, proof)` and
   `validate` atomically with `utility.batchAll`. Both that form and **Session
   Key** require the matching ownership proof and stash signature on Quip.
   Changing stash or keys clears the proof. Until the updated Apps is deployed,
   use **Developer > Extrinsics** to submit these calls with the stash (or the
   same `utility.batchAll`); the older staking form sends an empty proof.
5. Foundation approves the stash with `validatorAdmission.approve`.
   Approval makes the candidate eligible; it leaves `staking.validatorCount`
   unchanged and does not guarantee a seat in an already full set.
6. To expand the active set, read finalized `staking.validatorCount`,
   `staking.minimumValidatorCount`, admission and registered keys. Choose `N`
   with `minimumValidatorCount <= N <= 32`, at least `N` eligible, bonded
   candidates and enough online nodes to maintain finality. For the four-node
   rehearsal, onboard the fifth node before requesting `N = 5`.
   Foundation proposes
   `validatorAdmission.dispatchAsRoot(staking.setValidatorCount(N))` and
   members explicitly vote and close with the required actual two-thirds ayes.
   Optionally put `setValidatorCount(N)` and `staking.forceNewEra()` inside
   `utility.batchAll` under the same `dispatchAsRoot` to accelerate the election.
   This is a separate decision from approving a stash; approval never
   automatically increments the desired count.
7. At the finalized proposal receipt, check `System.ExtrinsicSuccess`,
   `Foundation.Executed.result` and `ValidatorAdmission.RootDispatched.result`
   (and `Utility.BatchCompleted` if batched). Read the finalized desired count
   back. Wait for a successful era election and the queued session activation;
   `forceNewEra` does not change the current authorities immediately. Verify the
   newcomer's actual keys in both BABE and GRANDPA authority sets, the Session
   validator list and a new GRANDPA set id. Then verify finalized block numbers
   continue advancing after the transition; an accepted proposal alone is
   insufficient evidence. Capture finalized hashes, events, era/session and
   authority lists for the rehearsal report.
8. For removal, Foundation revokes admission; removal affects the next successful
   election, not the current session. Chill/unbond separately. Failed elections
   retain the old set, so never remove capacity below the operating threshold.

Faucet mint/disable accept either the stored operational authority (including a
multisig account) or Foundation approval. Foundation alone can call
`faucetOps.set_authority(Some(account))` or revoke it with `None` (call index 2).
Authority updates never change the Enabled/Paused/PermanentlyDisabled state or
controller budget/issued totals. Revocation stops routine signed top-ups while
Foundation can still mint if the fuse remains Enabled. Bare Root and arbitrary
signed callers are rejected by both mint and authority management in this runtime.
The sibling service must use direct signed minting, verify current authority,
fuse, budget and runtime version, and handle dispatch receipts. Sudo wrapping
still fails; the existing service remains a rollout dependency.

## Review dispositions and operator safeguards

- Local/dev/local3 keep a one-validator floor for small functional tests. The
  public test preset uses a floor of three, preserving its three operators.
  `--chain rehearsal` selects four development validators with a minimum of four.
  Replace those dev keys with independent operator keys for a real rehearsal.
  An over-aggressive removal fails the election and retains the old set; add and
  onboard a replacement before removing an authority at the safety floor.
  `MinimumValidatorCount` is genesis-only in this staking version; ordinary
  staking administration cannot lower it. Before rehearsing emergency eviction,
  onboard a fifth independent validator, raise the desired validator count to
  five through Foundation, and verify all five are active and finalizing. Then
  exercise removal with the minimum kept at four.
- For emergency removal, revoke admission then approve
  `dispatch_as_root(staking.force_new_era())`. This accelerates the election but
  is not an immediate eviction, does not override the minimum floor, and still
  requires a session to activate queued keys. Without forcing, the delay is up
  to an era plus one queued session (approximately 70 minutes). The local3
  rotation script already exercises removal followed by this governed force.
- Governed runtime upgrades use a collective proposal containing
  `dispatch_as_root(system.authorize_upgrade(code_hash))`, followed by
  permissionless `system.apply_authorized_upgrade(code)`. Verify the authorization
  event, execution result and new runtime before removing sudo. Do not put the
  full Wasm into a collective `set_code` proposal: that stores megabytes during
  voting and must fit proposal length/weight limits. The real rehearsal remains
  required; the Root-path unit test is not a runtime-upgrade demonstration.
- Snapshot calibration must measure BOTH calls to `AdmissionData::candidates`
  per election (target and voter collection), including intent/key checks and
  ledger reads at 32 approved accounts. Its fixed Mandatory weight is provisional.

## Validation and release gates

Admission unit tests cover bounds, unauthorized/Root rejection, irreversible
mode changes, disabled graduation, and inner Root-dispatch errors. Runtime tests
cover snapshots, crowding, wrapped nominations, genesis fallback, rotations and
governance. Recorded execution results are below.

Weights for first-party admission and snapshot work are provisional; upstream
staking/collective/membership weights are baseline only. Run reference-host
benchmarks before release. Runtime authority-propagation tests do not prove
network finality: a dedicated multi-node finalized-head test across a real set
change and a real Foundation runtime-upgrade rehearsal remain mandatory.
Live-snapshot try-runtime is mandatory for any future in-place path. Full pinned
faucet integration and published CI/MR remain open. rr approved this implementation
after the faucet-origin and minor fixes; nothing in that approval or this document
waives Phase 1 release gates.

Benchmark builds retain the strict abstentions-always-no Foundation policy.
The SDK's `close_approved` benchmark depends on prime-aye substitution after
expiry, a branch unreachable with this policy; runtime benchmark discovery
therefore excludes that fixture. Actual affirmative approval still exercises
`close_early_approved` (including when closed after expiry), and expiry rejection
exercises `close_disapproved`. Their benchmarks remain enabled. The conservative
upstream collective weight implementation is retained; no off-reference weights
are substituted.

The dedicated network check is `js/quip-signer/test/validator-rotation.mjs`:
start three nodes on a fresh `local3` chain with the new binary, then run it with
Node.js and the existing signer dependencies/artifacts. It verifies finalized
proposal receipts including collective and Root inner results, removes Bob,
forces a new era through Foundation, observes both consensus sets at finalized
state, then requires finality to continue beyond the transition. With 10-minute
epochs, allow 25 minutes for election plus queued activation. This script was
not executed in the original implementation checkout because Node.js was absent.
The current review validation and network limitation are recorded below.

## Validation log (implementation checkout)

Before review, the Wasm-enabled workspace suite passed 431 tests with two
pre-existing QPoW sweep tests ignored. The earlier SKIP_WASM_BUILD attempt failed
three chain-spec tests because WASM_BINARY was absent; the Wasm-enabled rerun
resolved those failures. No tests were weakened or skipped to make them pass.

After the earlier Phase 2 review fixes:

- Runtime library tests with runtime-benchmarks and try-runtime: 46 passed.
- Faucet tests and benchmarks: 15 passed; admission tests and benchmarks: 10 passed.
- Targeted all-target Clippy with both features and warnings denied: passed.
- Node/runtime tests with Wasm: 61 passed; no-std recheck and node release rebuild passed.
  The initial release build encountered stale compiled faucet Config during the
  concurrent review edits; the rebuild is against the settled source tree.
- Real finalized-head transition and authorized-upgrade rehearsals: not executed.
  At that time, Node.js was unavailable for the checked-in network script.

Commands use offline dependencies. `SKIP_PALLET_REVIVE_FIXTURES=1` avoids the
SDK's external contract fixtures. Existing multisig tests log pre/post-dispatch
weight diagnostics while passing; this phase does not recalibrate them.

### MR !98 review corrections (2026-10-01)

Newly executed in this checkout: 438 workspace tests passed, with the same two
pre-existing QPoW tests ignored; production all-target workspace Clippy, rustfmt,
the production node build and browser signing-fixture tests passed. Eight focused
Foundation tests also passed with `runtime-benchmarks`; the collective CLI
preflight passed all 11 reachable benchmarks at 2 steps / 1 repeat. The signer
package tests passed. Temporary benchmark output was not committed.

In the sibling Apps checkout, TypeScript and changed-file ESLint passed. Eleven
focused browser tests cover the shared inputs in both staking flows, invalid
proof input, clearing proof on stash/key changes, Quip stash versus legacy
controller signing, and requiring atomic `batchAll` for Quip onboarding.
The installed `react-dropzone` UMD package has a pre-existing Node named-export
error; these tests used a temporary interoperability loader exposing its real
exports and Node's `--test-force-exit` for existing open handles. This does not
claim an unmodified Apps test-runner pass or a deployed Apps update.

The following reproduction uses Node v22.23.3 and installed dependencies, from
the Apps repository at `39fd001328c7fc2c035848d659decec3f89304d7`. The normal
runner invocation was attempted first and failed before loading the UI test
files with `react-dropzone does not provide an export named useDropzone`:

```bash
node node_modules/@polkadot/dev/scripts/polkadot-dev-run-test.mjs --env browser page-staking
```

Create the temporary interoperability loader with the same contents used for
the passing run. It imports the real installed CommonJS module and exposes its
exports; it does not replace component behavior:

```bash
cat > /tmp/quip-review-cjs-interop.mjs <<'JS'
export async function resolve(specifier, context, nextResolve) {
  const result = await nextResolve(specifier, context);
  if (specifier === 'react-dropzone') {
    return { ...result, url: `${result.url}?quip-cjs-interop`, shortCircuit: true };
  }
  return result;
}
export async function load(url, context, nextLoad) {
  if (url.endsWith('?quip-cjs-interop')) {
    const original = url.slice(0, -'?quip-cjs-interop'.length);
    return { format: 'module', shortCircuit: true,
      source: `import cjs from ${JSON.stringify(original)}; export const useDropzone = cjs.useDropzone; export const ErrorCode = cjs.ErrorCode; export default cjs.default ?? cjs;` };
  }
  return nextLoad(url, context);
}
JS

node --no-warnings --enable-source-maps \
  --loader /tmp/quip-review-cjs-interop.mjs \
  --require @polkadot/dev-test/browser --loader @polkadot/dev-ts/testCached \
  --test --test-force-exit --test-timeout=60000 \
  packages/page-staking/src/Actions/NewValidator.spec.tsx \
  packages/page-staking/src/Actions/partials/sessionKeyProof.spec.ts \
  packages/page-staking/src/Actions/partials/SessionKey.spec.tsx \
  packages/page-staking/src/Actions/Account/SetSessionKey.spec.tsx

node node_modules/typescript/bin/tsc --noEmit --pretty false
node node_modules/eslint/bin/eslint.js \
  packages/page-staking/src/Actions/NewValidator.tsx \
  packages/page-staking/src/Actions/NewValidator.spec.tsx \
  packages/page-staking/src/Actions/partials/SessionKey.tsx \
  packages/page-staking/src/Actions/partials/sessionKeyProof.ts \
  packages/page-staking/src/Actions/partials/sessionKeyProof.spec.ts \
  packages/page-staking/src/Actions/partials/SessionKey.spec.tsx \
  packages/page-staking/src/Actions/Account/SetSessionKey.tsx \
  packages/page-staking/src/Actions/Account/SetSessionKey.spec.tsx
```

The direct Node test invocation reported 11 passed / 0 failed / 0 skipped.
`--test-force-exit` ends the process after its tests complete because the
existing browser dependency graph leaves open handles. It does not establish
handle-cleanup correctness. Both the normal-run failure and this assisted pass
are part of the validation evidence, rather than an unmodified runner pass.

A new local3 node startup was attempted for the 3-to-2 rehearsal. After supplying
the system certificate bundle, this sandbox still rejected socket binding with
`Operation not permitted`; the service stopped before a network could form.
Neither a new 3-to-2 finality run nor a 4-to-5 onboarding run is claimed here.
Prior reviewer network results remain separate evidence. MR CI and the full
preflight, including revive's external compiler fixtures, remain release checks.

The planner independently queried GitLab source tags and main on 2026-10-01:
`v0.3.2`, `v0.3.2-rc1`, `v0.3.2-rc2` and main contained spec 118 / transaction
version 7, with no published spec-119 release evidence. Spec 119 is retained;
transaction version, signed extensions and the signing fixture values are
unchanged. This verifies published source, not every operator's running process.
