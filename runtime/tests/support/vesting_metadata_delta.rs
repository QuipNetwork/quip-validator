//! Compare metadata graphs, not portable registry numbering. Approved deltas
//! are Vesting at index 32 (including the three outer enums) and the explicitly
//! enumerated regenerated-weight values, exact mempool changes, staking constants,
//! appended staking commission error, exact XQVM audit changes, the exact
//! MinerRegistry W1-009 retention/paging changes, and the exact QuantumPow
//! W1-007 topology summary, and the exact compute-mempool W1-006 reward escrow
//! changes below.
use serde_json::Value;
use std::collections::BTreeSet;

// Derived from a complete graph comparison of the immutable pre-Vesting
// captures against 3a98d7f in BOTH runtimes (all 673 baseline types visited).
// Each value MUST differ: this guard describes the reviewed post-regeneration
// state, not an optional exemption. New or missing deltas require review.
// BlockWeights derives from block/extrinsic overhead; Xqvm's step price and
// limit derive from its measured execute_step/execute weights and block budget.
pub const REGENERATED_WEIGHT_VALUES: &[(&str, &str)] = &[
    ("System", "BlockWeights"),
    ("Xqvm", "MaxStepLimit"),
    ("Xqvm", "WeightPerStep"),
];

// QUIP-09 / R13: pin BOTH the immutable baseline and reviewed SCALE bytes.
// BudgetWindow is a storage default derived from BondingDuration, not a pallet
// constant; keep its metadata location explicit so no other surface is waived.
enum ConstantLocation {
    PalletConstant,
    StorageDefault,
}

pub struct ApprovedConstantDelta {
    pallet: &'static str,
    name: &'static str,
    location: ConstantLocation,
    pub baseline: &'static [u8],
    pub candidate: &'static [u8],
}

impl ApprovedConstantDelta {
    pub fn path(&self) -> String {
        let field = match self.location {
            ConstantLocation::PalletConstant => "value",
            ConstantLocation::StorageDefault => "default",
        };
        format!("{}.{}.{field}", self.pallet, self.name)
    }

    pub fn value<'a>(&self, metadata: &'a mut Value) -> Result<&'a mut Value, String> {
        match self.location {
            ConstantLocation::PalletConstant => constant_value(metadata, self.pallet, self.name),
            ConstantLocation::StorageDefault => metadata["pallets"]
                .as_array_mut()
                .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == self.pallet))
                .and_then(|pallet| pallet.pointer_mut("/storage/entries"))
                .and_then(Value::as_array_mut)
                .and_then(|entries| entries.iter_mut().find(|e| e["name"] == self.name))
                .and_then(|entry| entry.get_mut("default"))
                .ok_or_else(|| {
                    format!(
                        "missing metadata storage default {}.{}",
                        self.pallet, self.name
                    )
                }),
        }
    }
}

pub const APPROVED_CONSTANT_DELTAS: &[ApprovedConstantDelta] = &[
    // QUI-2548 (owner decision of 2026-11-25): `BlockLength`, encoded as its
    // `PerDispatchClass<u32>` (Normal, Operational, Mandatory, each u32
    // little-endian) and then `max_header_size: Option<u32>` (`None`, one 0
    // byte). Baseline 3.75 / 5 / 5 MiB (Normal = 75 % of 5 MiB); candidate
    // 3.5 / 3.75 / 5 MiB.
    ApprovedConstantDelta {
        pallet: "System",
        name: "BlockLength",
        location: ConstantLocation::PalletConstant,
        baseline: &[0, 0, 60, 0, 0, 0, 80, 0, 0, 0, 80, 0, 0],
        candidate: &[0, 0, 56, 0, 0, 0, 60, 0, 0, 0, 80, 0, 0],
    },
    ApprovedConstantDelta {
        pallet: "Staking",
        name: "BondingDuration",
        location: ConstantLocation::PalletConstant,
        baseline: &[28, 0, 0, 0],
        candidate: &[30, 0, 0, 0],
    },
    ApprovedConstantDelta {
        pallet: "Grandpa",
        name: "MaxSetIdSessionEntries",
        location: ConstantLocation::PalletConstant,
        baseline: &[168, 0, 0, 0, 0, 0, 0, 0],
        candidate: &[181, 0, 0, 0, 0, 0, 0, 0],
    },
    // W1-002: the per-run VM allocation cap drops from 16 MiB to 1 MiB (2 MiB,
    // then 1.5 MiB, then 1 MiB by the owner's second decision of 2026-11-22: a
    // cap that is not a power of two reaches the next allocator order).
    // u64 little-endian: 16 MiB = 0x0100_0000, 1 MiB = 0x0010_0000.
    ApprovedConstantDelta {
        pallet: "Xqvm",
        name: "MaxVmMemory",
        location: ConstantLocation::PalletConstant,
        baseline: &[0, 0, 0, 1, 0, 0, 0, 0],
        candidate: &[0, 0, 16, 0, 0, 0, 0, 0],
    },
    ApprovedConstantDelta {
        pallet: "SlashPolicy",
        name: "BudgetWindow",
        location: ConstantLocation::StorageDefault,
        baseline: &[28, 0, 0, 0],
        candidate: &[30, 0, 0, 0],
    },
];

pub fn constant_value<'a>(
    metadata: &'a mut Value,
    pallet_name: &str,
    constant_name: &str,
) -> Result<&'a mut Value, String> {
    let pallet = metadata
        .get_mut("pallets")
        .and_then(Value::as_array_mut)
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == pallet_name))
        .ok_or_else(|| format!("missing metadata pallet {pallet_name}"))?;
    let constant = pallet
        .get_mut("constants")
        .and_then(Value::as_array_mut)
        .and_then(|constants| constants.iter_mut().find(|c| c["name"] == constant_name))
        .ok_or_else(|| format!("missing metadata constant {pallet_name}.{constant_name}"))?;
    constant
        .get_mut("value")
        .ok_or_else(|| format!("missing metadata value {pallet_name}.{constant_name}"))
}

fn check_pallet_references(metadata: &Value, pallet_name: &str) -> Result<(), String> {
    let pallet = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == pallet_name))
        .ok_or_else(|| format!("missing metadata pallet {pallet_name}"))?;
    for (kind, outer_key, outer_name) in [
        ("calls", "call_enum_ty", "RuntimeCall"),
        ("error", "error_enum_ty", "RuntimeError"),
    ] {
        let pallet_id = pallet[kind]["ty"]
            .as_u64()
            .ok_or_else(|| format!("missing {pallet_name}.{kind} type"))?;
        let outer_id = metadata["outer_enums"][outer_key]
            .as_u64()
            .ok_or_else(|| format!("missing {outer_name} type"))?;
        let outer_id = usize::try_from(outer_id)
            .map_err(|_| format!("{outer_name} type id does not fit usize"))?;
        let variants = metadata["types"]["types"]
            .get(outer_id)
            .and_then(|ty| ty.pointer("/type/def/variant/variants"))
            .and_then(Value::as_array)
            .ok_or_else(|| format!("missing {outer_name} variants"))?;
        let mut xqvm = variants
            .iter()
            .filter(|variant| variant["name"] == pallet_name);
        let fields = xqvm.next().and_then(|variant| variant["fields"].as_array());
        if xqvm.next().is_some()
            || !fields.is_some_and(|fields| {
                fields.len() == 1 && fields[0]["type"].as_u64() == Some(pallet_id)
            })
        {
            return Err(format!(
                "{outer_name}::{pallet_name} must reference the pallet {pallet_name}.{kind} type"
            ));
        }
    }
    Ok(())
}

// Exact real-VM integration delta against the immutable pre-Vesting captures.
// No fixture re-anchoring: all old fields still undergo the graph comparison.
const MEMPOOL: &str = "QuantumComputeMempool";

fn error_variants(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet["error"]["ty"].as_u64())
        .ok_or("missing mempool error type")?;
    let id = usize::try_from(id).map_err(|_| "mempool error id does not fit usize")?;
    metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(|types| types.get_mut(id))
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "missing mempool error variants".into())
}

// Required M1/QUI-2158 documentation addition; every existing call doc stays
// unchanged. Outer/pallet reference identity is checked before this allowance.
const REGISTER_PROGRAM_DOCS: &[&str] = &[
    "",
    "Store the transform program first from a team-controlled account and",
    "verify its ownership before registering the spec. Ownership belongs to",
    "the first storer; another account can front-run the team's store.",
    "The owner can remove the program, stalling every submission to the spec",
    "until someone re-stores the same bytes. Recovery is permissionless.",
    "Submitters receive `VmProgramNotFound`, pay base plus lookup weight,",
    "and leave submission storage and events unchanged; no VM steps execute.",
    "QUI-2158 tracks reference-count refusal in pallet-xqvm or copying bytes",
    "into mempool storage at registration. It must be closed or explicitly",
    "accepted before mainnet launch.",
];

fn register_program_docs(metadata: &mut Value) -> Result<&mut Value, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet["calls"]["ty"].as_u64())
        .ok_or("missing mempool call type")?;
    let id = usize::try_from(id).map_err(|_| "mempool call id does not fit usize")?;
    metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(|types| types.get_mut(id))
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .and_then(|calls| {
            calls
                .iter_mut()
                .find(|call| call["name"] == "register_job_spec")
        })
        .and_then(|call| call.get_mut("docs"))
        .ok_or_else(|| "missing register_job_spec docs".into())
}

fn mempool_storage_entries(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    metadata["pallets"]
        .as_array_mut()
        .ok_or("missing pallets")?
        .iter_mut()
        .find(|pallet| pallet["name"] == MEMPOOL)
        .and_then(|pallet| pallet.get_mut("storage"))
        .and_then(|storage| storage.get_mut("entries"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "missing mempool storage".into())
}

fn mempool_constants(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    metadata["pallets"]
        .as_array_mut()
        .ok_or("missing pallets")?
        .iter_mut()
        .find(|pallet| pallet["name"] == MEMPOOL)
        .and_then(|pallet| pallet.get_mut("constants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "missing mempool constants".into())
}

// W3-006 (job-order storage deposit). Exact deltas, each appended last: the
// constants `OrderDepositPerItem` and `OrderDepositPerByte` (the same type and
// value as pallet-revive's `DepositPerItem`/`DepositPerByte`; the runtime wires
// both pallets to the stored-program rates), the `OrderDeposit` map after
// `OrderResults` (same hashers and key, the balance value), the
// `DepositNotAvailable` error and the `OrderDepositReleased` event. Runs before
// every other mempool normaliser, each of which expects its own delta last.
const ORDER_DEPOSIT_PER_ITEM_DOCS: &[&str] = &[
    " Per-order part of a job order's storage deposit (W3-006): reserved",
    " from the proposer at `propose_job` and released when the order is",
    " pruned. The runtime uses the same rate as stored programs.",
];
const ORDER_DEPOSIT_PER_BYTE_DOCS: &[&str] = &[
    " Per-byte part of a job order's storage deposit (W3-006), charged on",
    " the order and on the per-order state kept until it is pruned, at",
    " that state's bounded maximum.",
];
const ORDER_DEPOSIT_DOCS: &[&str] = &[
    " The storage deposit reserved from each order's proposer (W3-006):",
    " unreserved, exactly this amount, when the order is pruned.",
];
const DEPOSIT_NOT_AVAILABLE_DOCS: &[&str] =
    &["The proposer cannot reserve the order's storage deposit (W3-006)."];
const ORDER_DEPOSIT_RELEASED_DOCS: &[&str] = &[
    "The order was pruned and its storage deposit unreserved (W3-006):",
    "`amount` is what was actually returned to the proposer.",
];

fn revive_constant(metadata: &Value, name: &str) -> Option<Value> {
    metadata["pallets"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == "Revive")?["constants"]
        .as_array()?
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
}

// W3-001 B5d4. Exact delta: only the docs of the mempool `DuplicateAnswer`
// error, which now name the evicting reveal. They are put back to W1-012's
// text, which its own normaliser then checks. Runs first: the newest delta.
const DUPLICATE_ANSWER_DOCS_W1_012: &[&str] = &[
    "Another solver already submitted the same answer for this order",
    "(identical, globally flipped, or differing only in isolated spins).",
];
const DUPLICATE_ANSWER_DOCS_W3_001: &[&str] = &[
    "Another solver already submitted the same answer for this order",
    "(identical, globally flipped, or differing only in isolated spins),",
    "unless this is a reveal of a commitment made before that submission",
    "was effective (see `SolutionEvicted`).",
];

fn normalize_duplicate_answer_docs(after: &mut Value) -> Result<(), String> {
    let variant = mempool_variants(after, "error")?
        .iter_mut()
        .find(|v| v["name"] == "DuplicateAnswer")
        .ok_or("missing mempool DuplicateAnswer")?;
    if variant["docs"] != serde_json::json!(DUPLICATE_ANSWER_DOCS_W3_001) {
        return Err(format!(
            "missing or incorrect required mempool DuplicateAnswer docs: {}",
            variant["docs"]
        ));
    }
    variant["docs"] = serde_json::json!(DUPLICATE_ANSWER_DOCS_W1_012);
    Ok(())
}

// W3-001 B5e (eviction). Exact delta: the `SolutionEvicted` event, appended
// last after `CommitmentForfeited`. Runs first: the newest delta.
const SOLUTION_EVICTED_DOCS: &[&str] = &[
    "A revealed answer took `answer` from a solver whose submission of it",
    "was effective later (W3-001): `evicted`'s accepted solution, payee",
    "and ranking entry are removed, and `by` is ranked at its own",
    "energy.",
];

fn normalize_eviction(before: &mut Value, after: &mut Value) -> Result<(), String> {
    if mempool_variants(before, "event")?
        .iter()
        .any(|v| v["name"] == "SolutionEvicted")
    {
        return Err("unexpected baseline mempool SolutionEvicted".into());
    }
    let types = after.clone();
    let events = mempool_variants(after, "event")?.clone();
    let field_ty = |variant: &str, field: &str| -> Value {
        events
            .iter()
            .find(|v| v["name"] == variant)
            .and_then(|v| v["fields"].as_array())
            .and_then(|fields| fields.iter().find(|f| f["name"] == field))
            .map(|f| f["type"].clone())
            .unwrap_or(Value::Null)
    };
    let order_id_ty = field_ty("OrderDepositReleased", "order_id");
    let account_ty = field_ty("OrderDepositReleased", "proposer");
    let answer_ty = field_ty("SolutionEvicted", "answer");
    let events = mempool_variants(after, "event")?;
    let len = events.len();
    let want = serde_json::json!({
        "docs": SOLUTION_EVICTED_DOCS,
        "fields": [
            {"name": "order_id", "type": order_id_ty, "typeName": "u64"},
            {"name": "evicted", "type": account_ty.clone(), "typeName": "T::AccountId"},
            {"name": "by", "type": account_ty, "typeName": "T::AccountId"},
            {"name": "answer", "type": answer_ty.clone(), "typeName": "sp_core::H256"},
        ],
        "index": len.saturating_sub(1),
        "name": "SolutionEvicted",
    });
    if len < 2
        || events[len - 2]["name"] != "CommitmentForfeited"
        || events.last() != Some(&want)
        || !is_h256(&types, &answer_ty)
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool SolutionEvicted event: {}",
            serde_json::to_string(&events.last()).unwrap_or_default()
        ));
    }
    events.pop();
    Ok(())
}

// W3-001 B5d3 (same-solver replacement by rank key). Exact delta: only the
// docs of the mempool `SolutionNotImproved` error, which now names the
// equal-energy reveal from an earlier commitment. Runs first: the newest
// delta.
const SOLUTION_NOT_IMPROVED_DOCS_BEFORE: &[&str] =
    &["A solver may replace its accepted payload only with a lower energy."];
const SOLUTION_NOT_IMPROVED_DOCS_AFTER: &[&str] = &[
    "A solver may replace its accepted payload only with a better one: a",
    "lower energy, or an equal energy revealed from an earlier",
    "commitment (W3-001).",
];

fn normalize_solution_not_improved_docs(
    before: &mut Value,
    after: &mut Value,
) -> Result<(), String> {
    let docs = |metadata: &mut Value| -> Result<Value, String> {
        mempool_variants(metadata, "error")?
            .iter()
            .find(|v| v["name"] == "SolutionNotImproved")
            .map(|v| v["docs"].clone())
            .ok_or_else(|| "missing mempool SolutionNotImproved".to_string())
    };
    if docs(before)? != serde_json::json!(SOLUTION_NOT_IMPROVED_DOCS_BEFORE) {
        return Err("unexpected baseline mempool SolutionNotImproved docs".into());
    }
    if docs(after)? != serde_json::json!(SOLUTION_NOT_IMPROVED_DOCS_AFTER) {
        return Err(format!(
            "missing or incorrect required mempool SolutionNotImproved docs: {}",
            docs(after)?
        ));
    }
    if let Some(variant) = mempool_variants(after, "error")?
        .iter_mut()
        .find(|v| v["name"] == "SolutionNotImproved")
    {
        variant["docs"] = serde_json::json!(SOLUTION_NOT_IMPROVED_DOCS_BEFORE);
    }
    Ok(())
}

// W3-001 B5d (reveal). Exact deltas, each appended last: the
// `reveal_solution` call (index 11: order id, `submit_solution`'s solutions
// type, the `Option<WinnerPayee>` of `SolutionPayeeSet`, a `[u8; 32]` salt)
// and the `CommitmentNotFound`/`RevealTooEarly`/`CommitmentMismatch` errors.
// Runs first: the newest delta.
const REVEAL_SOLUTION_DOCS: &[&str] = &[
    "Reveal a committed answer and submit it (W3-001, design §B5 v2).",
    "",
    "`solutions`, `payee` and `salt` must reproduce the solver's",
    "commitment ([`Pallet::answer_commitment`]), made in an earlier",
    "block. The commitment is consumed -- deleted, its deposit",
    "returned -- and the solutions are submitted as `submit_solution`",
    "would, but ranked at the commitment's block (`effective_at`). If",
    "another solver holds the same answer from a later effective time,",
    "the reveal takes it and that solver's submission is evicted",
    "(`SolutionEvicted`). If the submission is refused, nothing changes:",
    "the commitment, its count and its deposit stay as they were.",
];
const REVEAL_ERRORS: &[(&str, &[&str])] = &[
    (
        "CommitmentNotFound",
        &["The solver has no outstanding commitment on this order (W3-001)."],
    ),
    (
        "RevealTooEarly",
        &[
            "A commitment can be revealed only in a later block than it was",
            "made in (W3-001).",
        ],
    ),
    (
        "CommitmentMismatch",
        &[
            "The revealed solutions, payee and salt do not match the commitment",
            "(W3-001).",
        ],
    ),
];

fn normalize_reveal(before: &mut Value, after: &mut Value) -> Result<(), String> {
    if mempool_variants(before, "calls")?
        .iter()
        .any(|v| v["name"] == "reveal_solution")
    {
        return Err("unexpected baseline mempool reveal_solution".into());
    }
    let types = after.clone();
    let calls = mempool_variants(after, "calls")?.clone();
    let events = mempool_variants(after, "event")?.clone();
    let field_ty = |variants: &[Value], variant: &str, field: &str| -> Value {
        variants
            .iter()
            .find(|v| v["name"] == variant)
            .and_then(|v| v["fields"].as_array())
            .and_then(|fields| fields.iter().find(|f| f["name"] == field))
            .map(|f| f["type"].clone())
            .unwrap_or(Value::Null)
    };
    let order_id_ty = field_ty(&calls, "claim_reward", "order_id");
    let solutions_ty = field_ty(&calls, "submit_solution", "solutions");
    let payee_ty = field_ty(&events, "SolutionPayeeSet", "payee");
    let salt_ty = calls
        .last()
        .and_then(|v| v["fields"].as_array())
        .and_then(|fields| fields.iter().find(|f| f["name"] == "salt"))
        .map(|f| f["type"].clone())
        .unwrap_or(Value::Null);
    let salt_def = registry_type(&types, &salt_ty).unwrap_or(Value::Null);
    let salt_ok = salt_def["def"]["array"]["len"] == 32
        && is_primitive(&types, &salt_def["def"]["array"]["type"], "u8");
    let want = serde_json::json!({
        "docs": REVEAL_SOLUTION_DOCS,
        "fields": [
            {"name": "order_id", "type": order_id_ty, "typeName": "u64"},
            {"name": "solutions", "type": solutions_ty, "typeName": "SolutionsOf<T>"},
            {"name": "payee", "type": payee_ty, "typeName": "Option<types::WinnerPayee>"},
            {"name": "salt", "type": salt_ty, "typeName": "[u8; 32]"},
        ],
        "index": 11,
        "name": "reveal_solution",
    });
    let calls = mempool_variants(after, "calls")?;
    let len = calls.len();
    if len < 2
        || calls[len - 2]["name"] != "commit_answer"
        || calls.last() != Some(&want)
        || !salt_ok
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool reveal_solution call: {}",
            serde_json::to_string(&calls.last()).unwrap_or_default()
        ));
    }
    calls.pop();

    let errors = mempool_variants(after, "error")?;
    let len = errors.len();
    let want: Vec<Value> = REVEAL_ERRORS
        .iter()
        .enumerate()
        .map(|(i, (name, docs))| {
            serde_json::json!({"docs": docs, "index": len - 3 + i, "name": name})
        })
        .collect();
    if len < 4
        || errors[len - 4]["name"] != "CommitmentDepositNotAvailable"
        || errors[len - 3..] != want[..]
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool reveal errors: {}",
            serde_json::to_string(&errors.get(len.saturating_sub(3)..)).unwrap_or_default()
        ));
    }
    errors.truncate(len - 3);
    Ok(())
}

// W3-001 B5c (answer commitments). Exact deltas, each appended last: the
// `commit_answer` call (index 10), the `MaxCommitmentsPerOrder` constant
// (u32, 64), the `AnswerCommitments` (order, solver) -> `AnswerCommitment`
// and `CommitmentCount` storage after `NextSubmissionSeq`, the
// `CommitmentCapReached`/`CommitmentDepositNotAvailable` errors and the
// `AnswerCommitted`/`CommitmentForfeited` events. Runs first: the newest delta.
const MAX_COMMITMENTS_PER_ORDER_DOCS: &[&str] = &[
    " The most outstanding answer commitments one order holds (W3-001,",
    " design §B5 v2 item 3): 64, twice the 32-solver cap.",
];
const ANSWER_COMMITMENTS_DOCS: &[&str] = &[
    " Outstanding answer commitments (W3-001): at most one per (order,",
    " solver), at most `MaxCommitmentsPerOrder` per order, each holding its",
    " reserved deposit. Those still outstanding when the order is finalised",
    " are forfeited to its proposer.",
];
const COMMITMENT_COUNT_DOCS: &[&str] =
    &[" The number of outstanding commitments on each order (W3-001)."];
const COMMITMENT_ERRORS: &[(&str, &[&str])] = &[
    (
        "CommitmentCapReached",
        &[
            "The order already holds `MaxCommitmentsPerOrder` commitments",
            "(W3-001).",
        ],
    ),
    (
        "CommitmentDepositNotAvailable",
        &["The solver cannot reserve the commitment deposit (W3-001)."],
    ),
];
const ANSWER_COMMITTED_DOCS: &[&str] = &[
    "A solver committed to an answer on an order (W3-001), or replaced",
    "its commitment: `commit_block` is the effective time its reveal",
    "will carry.",
];
const COMMITMENT_FORFEITED_DOCS: &[&str] = &[
    "A commitment was still outstanding when its order was finalised",
    "(W3-001): its deposit went to the proposer (`to_proposer`), or was",
    "burned when the proposer could not receive it (`burned`).",
];
const COMMIT_ANSWER_DOCS: &[&str] = &[
    "Commit to an answer before revealing it (W3-001, design §B5 v2).",
    "",
    "`commitment` is `blake2_256` of the SCALE encoding of",
    "`(b\"quip-answer-commit-v1\", order_id, solver,",
    "blake2_256(SCALE(solutions)), payee, salt)`, where `salt` is 32",
    "random bytes: a low-entropy salt on a small order lets a watcher",
    "brute-force the commitment. Allowed to a registered solver eligible",
    "for the order's mode, while the order is open and not expired.",
    "Committing again replaces the commitment (and its block) in place;",
    "a first commitment reserves the commitment deposit, which the reveal",
    "returns and finalisation forfeits to the proposer if it is still",
    "outstanding.",
];

fn normalize_commitments(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let has = |list: &[Value], name: &str| list.iter().any(|v| v["name"] == name);
    if has(mempool_constants(before)?, "MaxCommitmentsPerOrder")
        || has(mempool_storage_entries(before)?, "AnswerCommitments")
        || has(mempool_storage_entries(before)?, "CommitmentCount")
        || has(mempool_variants(before, "calls")?, "commit_answer")
    {
        return Err("unexpected baseline mempool commitments".into());
    }
    let types = after.clone();
    let field_ty = |variants: &[Value], variant: &str, field: &str| -> Value {
        variants
            .iter()
            .find(|v| v["name"] == variant)
            .and_then(|v| v["fields"].as_array())
            .and_then(|fields| fields.iter().find(|f| f["name"] == field))
            .map(|f| f["type"].clone())
            .unwrap_or(Value::Null)
    };

    // The call: `commit_answer`, index 10, last.
    let calls = mempool_variants(after, "calls")?.clone();
    let order_id_ty = field_ty(&calls, "claim_reward", "order_id");
    let h256_ty = types["types"]["types"]
        .as_array()
        .ok_or("missing candidate registry")?
        .iter()
        .find(|ty| ty["type"]["path"] == serde_json::json!(["primitive_types", "H256"]))
        .map(|ty| ty["id"].clone())
        .ok_or("missing candidate H256")?;
    let want = serde_json::json!({
        "docs": COMMIT_ANSWER_DOCS,
        "fields": [
            {"name": "order_id", "type": order_id_ty, "typeName": "u64"},
            {"name": "commitment", "type": h256_ty.clone(), "typeName": "sp_core::H256"},
        ],
        "index": 10,
        "name": "commit_answer",
    });
    let calls = mempool_variants(after, "calls")?;
    if calls.last() != Some(&want) {
        return Err(format!(
            "missing or incorrect required mempool commit_answer call: {}",
            serde_json::to_string(&calls.last()).unwrap_or_default()
        ));
    }
    calls.pop();

    // The constant: `MaxCommitmentsPerOrder`, after `MinBlockWait`, `u32` 64.
    let constants = mempool_constants(after)?;
    let u32_ty = constants
        .iter()
        .find(|c| c["name"] == "MaxOrdersPerProposer")
        .map(|c| c["ty"].clone())
        .ok_or("missing candidate MaxOrdersPerProposer")?;
    let want = serde_json::json!({
        "deprecation_info": "NotDeprecated",
        "docs": MAX_COMMITMENTS_PER_ORDER_DOCS,
        "name": "MaxCommitmentsPerOrder",
        "ty": u32_ty,
        "value": [64, 0, 0, 0],
    });
    let len = constants.len();
    if len < 2 || constants[len - 2]["name"] != "MinBlockWait" || constants.last() != Some(&want) {
        return Err(format!(
            "missing, misplaced or incorrect required mempool MaxCommitmentsPerOrder: {}",
            serde_json::to_string(&constants.last()).unwrap_or_default()
        ));
    }
    constants.pop();

    // Storage: `AnswerCommitments` then `CommitmentCount`, after
    // `NextSubmissionSeq`.
    let entries = mempool_storage_entries(after)?;
    let solutions = entries
        .iter()
        .find(|e| e["name"] == "OrderSolutions")
        .cloned()
        .ok_or("missing candidate OrderSolutions")?;
    let seq = entries
        .iter()
        .find(|e| e["name"] == "NextSubmissionSeq")
        .cloned()
        .ok_or("missing candidate NextSubmissionSeq")?;
    let deposit_value = entries
        .iter()
        .find(|e| e["name"] == "OrderDeposit")
        .map(|e| e["ty"]["Map"]["value"].clone())
        .ok_or("missing candidate OrderDeposit")?;
    let len = entries.len();
    if len < 3 || entries[len - 3]["name"] != "NextSubmissionSeq" {
        return Err("misplaced required mempool commitment storage".into());
    }
    let commitments = entries[len - 2].clone();
    let value_id = commitments["ty"]["Map"]["value"].clone();
    let value = registry_type(&types, &value_id).unwrap_or(Value::Null);
    let block_number = value["params"]
        .as_array()
        .and_then(|params| params.iter().find(|p| p["name"] == "BlockNumber"))
        .map(|p| p["type"].clone())
        .unwrap_or(Value::Null);
    let want_value_fields = serde_json::json!([
        {"name": "commitment", "type": h256_ty, "typeName": "sp_core::H256"},
        {"name": "commit_block", "type": block_number.clone(), "typeName": "BlockNumber"},
        {"name": "deposit", "type": deposit_value.clone(), "typeName": "Balance"},
    ]);
    let want_commitments = serde_json::json!({
        "default": [0],
        "deprecation_info": "NotDeprecated",
        "docs": ANSWER_COMMITMENTS_DOCS,
        "modifier": "Optional",
        "name": "AnswerCommitments",
        "ty": {"Map": {
            "hashers": solutions["ty"]["Map"]["hashers"].clone(),
            "key": solutions["ty"]["Map"]["key"].clone(),
            "value": value_id,
        }},
    });
    if commitments != want_commitments
        || value["path"] != mempool_type_path("AnswerCommitment")
        || value["def"]["composite"]["fields"] != want_value_fields
        || !is_primitive(&types, &block_number, "u32")
    {
        return Err(format!(
            "missing or incorrect required mempool AnswerCommitments: {} / {}",
            serde_json::to_string(&commitments).unwrap_or_default(),
            serde_json::to_string(&value).unwrap_or_default()
        ));
    }
    let want_count = serde_json::json!({
        "default": [0, 0, 0, 0],
        "deprecation_info": "NotDeprecated",
        "docs": COMMITMENT_COUNT_DOCS,
        "modifier": "Default",
        "name": "CommitmentCount",
        "ty": seq["ty"].clone(),
    });
    if entries[len - 1] != want_count {
        return Err(format!(
            "missing or incorrect required mempool CommitmentCount: {}",
            serde_json::to_string(&entries[len - 1]).unwrap_or_default()
        ));
    }
    entries.truncate(len - 2);

    // Errors, after `BlockWaitTooShort`.
    let errors = mempool_variants(after, "error")?;
    let len = errors.len();
    let want: Vec<Value> = COMMITMENT_ERRORS
        .iter()
        .enumerate()
        .map(|(i, (name, docs))| {
            serde_json::json!({"docs": docs, "index": len - 2 + i, "name": name})
        })
        .collect();
    if len < 3 || errors[len - 3]["name"] != "BlockWaitTooShort" || errors[len - 2..] != want[..] {
        return Err(format!(
            "missing, misplaced or incorrect required mempool commitment errors: {}",
            serde_json::to_string(&errors.get(len.saturating_sub(2)..)).unwrap_or_default()
        ));
    }
    errors.truncate(len - 2);

    // Events, after `OrderDepositReleased`.
    let events = mempool_variants(after, "event")?.clone();
    let order_id_ty = field_ty(&events, "OrderDepositReleased", "order_id");
    let account_ty = field_ty(&events, "OrderDepositReleased", "proposer");
    let balance_ty = field_ty(&events, "OrderDepositReleased", "amount");
    let commitment_ty = field_ty(&events, "AnswerCommitted", "commitment");
    let events = mempool_variants(after, "event")?;
    let len = events.len();
    let want = serde_json::json!([
        {
            "docs": ANSWER_COMMITTED_DOCS,
            "fields": [
                {"name": "order_id", "type": order_id_ty.clone(), "typeName": "u64"},
                {"name": "solver", "type": account_ty.clone(), "typeName": "T::AccountId"},
                {"name": "commitment", "type": commitment_ty.clone(), "typeName": "sp_core::H256"},
                {"name": "commit_block", "type": block_number, "typeName": "BlockNumberFor<T>"},
            ],
            "index": len.saturating_sub(2),
            "name": "AnswerCommitted",
        },
        {
            "docs": COMMITMENT_FORFEITED_DOCS,
            "fields": [
                {"name": "order_id", "type": order_id_ty, "typeName": "u64"},
                {"name": "solver", "type": account_ty, "typeName": "T::AccountId"},
                {"name": "to_proposer", "type": balance_ty.clone(), "typeName": "BalanceOf<T>"},
                {"name": "burned", "type": balance_ty, "typeName": "BalanceOf<T>"},
            ],
            "index": len.saturating_sub(1),
            "name": "CommitmentForfeited",
        },
    ]);
    if len < 3
        || events[len - 3]["name"] != "OrderDepositReleased"
        || events.get(len - 2..) != want.as_array().map(Vec::as_slice)
        || !is_h256(&types, &commitment_ty)
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool commitment events: {}",
            serde_json::to_string(&events.get(len.saturating_sub(2)..)).unwrap_or_default()
        ));
    }
    events.truncate(len - 2);
    Ok(())
}

// W3-001 B5b (ordering key, N2). Exact deltas:
// - `effective_at: BlockNumber` and `seq: u32` appended to `JobSolution`,
//   `RankedSolver` and `FrontRunner`; the last two also gain a `BlockNumber`
//   type parameter (appended after `AccountId`);
// - `OrderAnswers`' value becomes `AnswerOwner { solver, best_energy_milli,
//   effective_at, seq }` (was the solver's account);
// - `NextSubmissionSeq` (u64 -> u32, `ValueQuery`) appended after B4a's
//   `OrderDeposit`.
// Runs first: the storage entry is the newest delta.
const ORDER_ANSWERS_OWNER_DOCS: &[&str] = &[
    "",
    " The value is the answer's owner with its energy and ordering key",
    " (W3-001), so a contested answer and a ranking recompute read no",
    " payloads.",
];
const NEXT_SUBMISSION_SEQ_DOCS: &[&str] = &[
    " Each open order's arrival counter (W3-001 N2): the `seq` of its next",
    " accepted submission. Cleared with the order's solutions.",
];

fn mempool_type_path(name: &str) -> Value {
    serde_json::json!(["pallet_quantum_compute_mempool", "types", name])
}

fn is_primitive(metadata: &Value, id: &Value, primitive: &str) -> bool {
    registry_type(metadata, id).is_some_and(|ty| ty["def"]["primitive"] == primitive)
}

fn normalize_ordering_key(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let types = after.clone();

    // Storage: `NextSubmissionSeq`, last, after `OrderDeposit`.
    if mempool_storage_entries(before)?
        .iter()
        .any(|e| e["name"] == "NextSubmissionSeq")
    {
        return Err("unexpected baseline mempool NextSubmissionSeq".into());
    }
    let entries = mempool_storage_entries(after)?;
    let deposit = entries
        .iter()
        .find(|e| e["name"] == "OrderDeposit")
        .cloned()
        .ok_or("missing candidate OrderDeposit")?;
    let len = entries.len();
    let last = entries.last().cloned().unwrap_or(Value::Null);
    let want = serde_json::json!({
        "default": [0, 0, 0, 0],
        "deprecation_info": "NotDeprecated",
        "docs": NEXT_SUBMISSION_SEQ_DOCS,
        "modifier": "Default",
        "name": "NextSubmissionSeq",
        "ty": {"Map": {
            "hashers": deposit["ty"]["Map"]["hashers"].clone(),
            "key": deposit["ty"]["Map"]["key"].clone(),
            "value": last["ty"]["Map"]["value"].clone(),
        }},
    });
    if len < 2
        || entries[len - 2]["name"] != "OrderDeposit"
        || last != want
        || !is_primitive(&types, &last["ty"]["Map"]["value"], "u32")
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool NextSubmissionSeq: {}",
            serde_json::to_string(&last).unwrap_or_default()
        ));
    }
    entries.pop();

    // M3 (R8 on B5b): every new `BlockNumber` parameter is `JobSolution`'s
    // own, the runtime's `u32`.
    let job_solution_block_number = types["types"]["types"]
        .as_array()
        .ok_or("missing candidate registry")?
        .iter()
        .find(|ty| ty["type"]["path"] == mempool_type_path("JobSolution"))
        .and_then(|ty| ty["type"]["params"].as_array())
        .and_then(|params| params.iter().find(|p| p["name"] == "BlockNumber"))
        .map(|p| p["type"].clone())
        .ok_or("missing candidate JobSolution BlockNumber parameter")?;
    if !is_primitive(&types, &job_solution_block_number, "u32") {
        return Err("JobSolution's BlockNumber is not u32".into());
    }

    // `OrderAnswers`' value: the exact `AnswerOwner`, restored to the account.
    let entries = mempool_storage_entries(after)?;
    let solutions = entries
        .iter()
        .find(|e| e["name"] == "OrderSolutions")
        .cloned()
        .ok_or("missing candidate OrderSolutions")?;
    let account = tuple_fields(&types, &solutions["ty"]["Map"]["key"])
        .and_then(|key| key.get(1).cloned())
        .ok_or("OrderSolutions key is not a pair")?;
    let answers = entries
        .iter_mut()
        .find(|e| e["name"] == "OrderAnswers")
        .ok_or("missing candidate OrderAnswers")?;
    let owner_id = answers["ty"]["Map"]["value"].clone();
    let owner = registry_type(&types, &owner_id).unwrap_or(Value::Null);
    let block_number = owner["params"]
        .as_array()
        .and_then(|params| params.iter().find(|p| p["name"] == "BlockNumber"))
        .map(|p| p["type"].clone())
        .unwrap_or(Value::Null);
    let want_owner = serde_json::json!([
        {"name": "solver", "type": account.clone(), "typeName": "AccountId"},
        {"name": "best_energy_milli", "type": owner["def"]["composite"]["fields"][1]["type"].clone(), "typeName": "i64"},
        {"name": "effective_at", "type": block_number.clone(), "typeName": "BlockNumber"},
        {"name": "seq", "type": owner["def"]["composite"]["fields"][3]["type"].clone(), "typeName": "u32"},
    ]);
    let want_params = serde_json::json!([
        {"name": "AccountId", "type": account.clone()},
        {"name": "BlockNumber", "type": job_solution_block_number.clone()},
    ]);
    if owner["path"] != mempool_type_path("AnswerOwner")
        || owner["params"] != want_params
        || owner["def"]["composite"]["fields"] != want_owner
        || !is_primitive(&types, &want_owner[1]["type"], "i64")
        || !is_primitive(&types, &want_owner[3]["type"], "u32")
    {
        return Err(format!(
            "missing or incorrect required OrderAnswers value AnswerOwner: {}",
            serde_json::to_string(&owner).unwrap_or_default()
        ));
    }
    answers["ty"]["Map"]["value"] = account;
    let want_docs: Vec<&str> = ORDER_ANSWERS_DOCS
        .iter()
        .chain(ORDER_ANSWERS_OWNER_DOCS)
        .copied()
        .collect();
    if answers["docs"] != serde_json::json!(want_docs) {
        return Err(format!(
            "missing or incorrect required OrderAnswers owner docs: {}",
            serde_json::to_string(&answers["docs"]).unwrap_or_default()
        ));
    }
    answers["docs"] = serde_json::json!(ORDER_ANSWERS_DOCS);

    // Struct fields and type parameters.
    for (name, new_param) in [
        ("JobSolution", false),
        ("RankedSolver", true),
        ("FrontRunner", true),
    ] {
        let path = mempool_type_path(name);
        let mut found = 0;
        for ty in after["types"]["types"]
            .as_array_mut()
            .ok_or("missing candidate registry")?
            .iter_mut()
            .filter(|ty| ty["type"]["path"] == path)
        {
            let block_number = ty["type"]["params"]
                .as_array()
                .and_then(|params| params.iter().find(|p| p["name"] == "BlockNumber"))
                .map(|p| p["type"].clone())
                .unwrap_or(Value::Null);
            let fields = ty
                .pointer_mut("/type/def/composite/fields")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| format!("{name} is not a struct"))?;
            let n = fields.len();
            let seq_ty = fields
                .last()
                .map(|f| f["type"].clone())
                .unwrap_or(Value::Null);
            let want = serde_json::json!([
                {"name": "effective_at", "type": block_number, "typeName": "BlockNumber"},
                {"name": "seq", "type": seq_ty.clone(), "typeName": "u32"},
            ]);
            if n < 2
                || fields.get(n - 2..) != want.as_array().map(Vec::as_slice)
                || !is_primitive(&types, &seq_ty, "u32")
            {
                return Err(format!(
                    "missing or incorrect required {name}.effective_at/seq fields: {}",
                    serde_json::to_string(&fields.get(n.saturating_sub(2)..)).unwrap_or_default()
                ));
            }
            fields.truncate(n - 2);
            if new_param {
                let params = ty["type"]["params"]
                    .as_array_mut()
                    .ok_or_else(|| format!("{name} has no type parameters"))?;
                if params.len() != 2
                    || params[1]["name"] != "BlockNumber"
                    || params[1]["type"] != job_solution_block_number
                {
                    return Err(format!(
                        "missing or incorrect required {name} BlockNumber parameter: {}",
                        serde_json::to_string(params).unwrap_or_default()
                    ));
                }
                params.pop();
            }
            found += 1;
        }
        if found == 0 {
            return Err(format!("missing candidate {name} type"));
        }
    }
    Ok(())
}

// W3-001 B5a (minimum block wait). Exact deltas, each appended last: the
// `MinBlockWait` constant after B4a's `OrderDepositPerByte` (the type of
// `MaxBlockWait`, value 2), and the `BlockWaitTooShort` error after B4c's
// `DeadlineTooShort`. Runs before every other mempool normaliser.
const MIN_BLOCK_WAIT_DOCS: &[&str] = &[
    " The shortest `block_wait` an order may set (W3-001, design §B5 v2",
    " item 8): at least 2, so an honest reveal that slips one block behind",
    " a copied answer at block M still lands, at M + 1, before the order",
    " closes.",
];
const BLOCK_WAIT_TOO_SHORT_DOCS: &[&str] = &["`block_wait` is below `MinBlockWait` (W3-001)."];

fn normalize_min_block_wait(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let has = |list: &[Value], name: &str| list.iter().any(|v| v["name"] == name);
    if has(mempool_constants(before)?, "MinBlockWait")
        || has(mempool_variants(before, "error")?, "BlockWaitTooShort")
    {
        return Err("unexpected baseline mempool MinBlockWait".into());
    }
    let max_wait_ty = mempool_constants(after)?
        .iter()
        .find(|c| c["name"] == "MaxBlockWait")
        .map(|c| c["ty"].clone())
        .ok_or("missing candidate MaxBlockWait")?;
    let constants = mempool_constants(after)?;
    let want = serde_json::json!({
        "deprecation_info": "NotDeprecated",
        "docs": MIN_BLOCK_WAIT_DOCS,
        "name": "MinBlockWait",
        "ty": max_wait_ty,
        "value": [2, 0, 0, 0],
    });
    let len = constants.len();
    if len < 2
        || constants[len - 2]["name"] != "OrderDepositPerByte"
        || constants.last() != Some(&want)
    {
        return Err(format!(
            "missing, misplaced or incorrect required mempool MinBlockWait: {}",
            serde_json::to_string(&constants.last()).unwrap_or_default()
        ));
    }
    constants.pop();
    let errors = mempool_variants(after, "error")?;
    let len = errors.len();
    let want = serde_json::json!({
        "docs": BLOCK_WAIT_TOO_SHORT_DOCS,
        "index": len.saturating_sub(1),
        "name": "BlockWaitTooShort",
    });
    if len < 2 || errors[len - 2]["name"] != "DeadlineTooShort" || errors.last() != Some(&want) {
        return Err(format!(
            "missing, misplaced or incorrect required mempool BlockWaitTooShort: {}",
            serde_json::to_string(&errors.last()).unwrap_or_default()
        ));
    }
    errors.pop();
    Ok(())
}

// W3-006 B4c (minimum deadline). Exact delta: the `DeadlineTooShort` error
// appended directly after B4a's `DepositNotAvailable`. Runs before
// `normalize_order_deposit`, which expects its own error last.
const DEADLINE_TOO_SHORT_DOCS: &[&str] = &["`deadline_blocks` must be at least 1 (W3-006)."];

fn normalize_deadline_too_short(before: &mut Value, after: &mut Value) -> Result<(), String> {
    if mempool_variants(before, "error")?
        .iter()
        .any(|v| v["name"] == "DeadlineTooShort")
    {
        return Err("unexpected baseline mempool DeadlineTooShort".into());
    }
    let errors = mempool_variants(after, "error")?;
    let len = errors.len();
    let want = serde_json::json!({
        "docs": DEADLINE_TOO_SHORT_DOCS,
        "index": len.saturating_sub(1),
        "name": "DeadlineTooShort",
    });
    if len < 2 || errors[len - 2]["name"] != "DepositNotAvailable" || errors.last() != Some(&want) {
        return Err(format!(
            "missing, misplaced or incorrect required mempool DeadlineTooShort: {}",
            serde_json::to_string(&errors.last()).unwrap_or_default()
        ));
    }
    errors.pop();
    Ok(())
}

fn normalize_order_deposit(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let has = |list: &[Value], name: &str| list.iter().any(|v| v["name"] == name);
    if has(mempool_constants(before)?, "OrderDepositPerItem")
        || has(mempool_constants(before)?, "OrderDepositPerByte")
        || has(mempool_storage_entries(before)?, "OrderDeposit")
        || has(mempool_variants(before, "error")?, "DepositNotAvailable")
        || has(mempool_variants(before, "event")?, "OrderDepositReleased")
    {
        return Err("unexpected baseline mempool order deposit".into());
    }
    let types = after.clone();

    // Constants: the last two, equal in type and value to revive's.
    let per_item =
        revive_constant(&types, "DepositPerItem").ok_or("missing Revive.DepositPerItem")?;
    let per_byte =
        revive_constant(&types, "DepositPerByte").ok_or("missing Revive.DepositPerByte")?;
    let constants = mempool_constants(after)?;
    let n = constants.len();
    let want = serde_json::json!([
        {
            "deprecation_info": "NotDeprecated",
            "docs": ORDER_DEPOSIT_PER_ITEM_DOCS,
            "name": "OrderDepositPerItem",
            "ty": per_item["ty"].clone(),
            "value": per_item["value"].clone(),
        },
        {
            "deprecation_info": "NotDeprecated",
            "docs": ORDER_DEPOSIT_PER_BYTE_DOCS,
            "name": "OrderDepositPerByte",
            "ty": per_byte["ty"].clone(),
            "value": per_byte["value"].clone(),
        }
    ]);
    if n < 2 || constants.get(n - 2..) != want.as_array().map(Vec::as_slice) {
        return Err(format!(
            "missing or incorrect required mempool order-deposit constants: {}",
            serde_json::to_string(&constants.get(n.saturating_sub(2)..)).unwrap_or_default()
        ));
    }
    constants.truncate(n - 2);

    // Storage: the last entry, after `OrderResults`.
    let entries = mempool_storage_entries(after)?;
    let results = entries
        .iter()
        .find(|e| e["name"] == "OrderResults")
        .cloned()
        .ok_or("missing candidate OrderResults")?;
    let want = serde_json::json!({
        "default": [0],
        "deprecation_info": "NotDeprecated",
        "docs": ORDER_DEPOSIT_DOCS,
        "modifier": "Optional",
        "name": "OrderDeposit",
        "ty": {"Map": {
            "hashers": results["ty"]["Map"]["hashers"].clone(),
            "key": results["ty"]["Map"]["key"].clone(),
            "value": per_item["ty"].clone(),
        }},
    });
    let len = entries.len();
    if len < 2 || entries[len - 2]["name"] != "OrderResults" || entries[len - 1] != want {
        return Err(format!(
            "missing, misplaced or incorrect required mempool OrderDeposit: {}",
            serde_json::to_string(&entries.last()).unwrap_or_default()
        ));
    }
    entries.pop();

    // Error: the last variant, directly after W1-012's `DuplicateAnswer`.
    let errors = mempool_variants(after, "error")?;
    let want = serde_json::json!({
        "docs": DEPOSIT_NOT_AVAILABLE_DOCS,
        "index": errors.len().saturating_sub(1),
        "name": "DepositNotAvailable",
    });
    let len = errors.len();
    if len < 2 || errors[len - 2]["name"] != "DuplicateAnswer" || errors.last() != Some(&want) {
        return Err(format!(
            "missing or incorrect required mempool DepositNotAvailable: {}",
            serde_json::to_string(&errors.last()).unwrap_or_default()
        ));
    }
    errors.pop();

    // Event: the last variant, directly after W1-006's `EscrowReleased`, its
    // field types those of `RewardReclaimed`.
    let events = mempool_variants(after, "event")?;
    let template = events
        .iter()
        .find(|v| v["name"] == "RewardReclaimed")
        .ok_or("missing RewardReclaimed event")?
        .clone();
    let field = |name: &str| -> Value {
        template["fields"]
            .as_array()
            .and_then(|fields| fields.iter().find(|f| f["name"] == name))
            .map(|f| f["type"].clone())
            .unwrap_or(Value::Null)
    };
    let want = serde_json::json!({
        "docs": ORDER_DEPOSIT_RELEASED_DOCS,
        "fields": [
            {"name": "order_id", "type": field("order_id"), "typeName": "u64"},
            {"name": "proposer", "type": field("proposer"), "typeName": "T::AccountId"},
            {"name": "amount", "type": field("amount"), "typeName": "BalanceOf<T>"}
        ],
        "index": events.len().saturating_sub(1),
        "name": "OrderDepositReleased",
    });
    let len = events.len();
    if len < 2 || events[len - 2]["name"] != "EscrowReleased" || events.last() != Some(&want) {
        return Err(format!(
            "missing or incorrect required mempool OrderDepositReleased: {}",
            serde_json::to_string(&events.last()).unwrap_or_default()
        ));
    }
    events.pop();
    Ok(())
}

fn normalize_audit_retention(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let names = ["OrdersPrunableAt", "NextPruneBucket"];
    if mempool_storage_entries(before)?
        .iter()
        .any(|entry| names.iter().any(|name| entry["name"] == *name))
    {
        return Err("unexpected baseline retention storage".into());
    }
    if mempool_constants(before)?
        .iter()
        .any(|constant| constant["name"] == "OrderRetentionBlocks")
    {
        return Err("unexpected baseline retention constant".into());
    }
    let epoch: Vec<u8> =
        serde_json::from_value(constant_value(after, "Babe", "EpochDuration")?.clone())
            .map_err(|_| "invalid epoch bytes")?;
    let epoch = u64::from_le_bytes(epoch.try_into().map_err(|_| "invalid epoch length")?);
    let retention = u32::try_from(epoch.checked_mul(28 * 6).ok_or("retention overflow")?)
        .map_err(|_| "retention exceeds BlockNumber")?;
    let entries = mempool_storage_entries(after)?;
    for (name, template, docs) in [
        (
            "OrdersPrunableAt",
            "OrdersExpiringAt",
            " Closed orders indexed by the block when their retention window ends.",
        ),
        (
            "NextPruneBucket",
            "NextExpiryBucket",
            " Earliest retention bucket to inspect, absent when no cleanup is queued.",
        ),
    ] {
        let mut expected = entries
            .iter()
            .find(|entry| entry["name"] == template)
            .ok_or("missing retention template")?
            .clone();
        expected["name"] = Value::from(name);
        expected["docs"] = serde_json::json!([docs]);
        if name == "NextPruneBucket" {
            expected["modifier"] = Value::from("Optional");
            expected["default"] = serde_json::json!([0]);
        }
        let found: Vec<_> = entries
            .iter()
            .filter(|entry| entry["name"] == name)
            .collect();
        if found.len() != 1 || *found[0] != expected {
            return Err(format!("unexpected required retention storage {name}"));
        }
    }
    entries.retain(|entry| !names.iter().any(|name| entry["name"] == *name));
    let constants = mempool_constants(after)?;
    let mut expected = constants
        .iter()
        .find(|constant| constant["name"] == "ResultTtlBlocks")
        .ok_or("missing retention constant template")?
        .clone();
    expected["name"] = Value::from("OrderRetentionBlocks");
    expected["value"] = serde_json::json!(retention.to_le_bytes());
    expected["docs"] =
        serde_json::json!([" Retention after closure before bounded automatic order cleanup."]);
    let found: Vec<_> = constants
        .iter()
        .filter(|constant| constant["name"] == "OrderRetentionBlocks")
        .collect();
    if found.len() != 1 || *found[0] != expected {
        return Err("unexpected required retention constant".into());
    }
    constants.retain(|constant| constant["name"] != "OrderRetentionBlocks");
    Ok(())
}

fn normalize_audit_expiry_storage(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let old_entries = mempool_storage_entries(before)?;
    if old_entries
        .iter()
        .any(|entry| entry["name"] == "OldestExpiryBucket")
    {
        return Err("unexpected baseline expiry anchor".into());
    }
    let old_docs = serde_json::json!([
        " Earliest `OrdersExpiringAt` bucket that `on_initialize` has not yet",
        " drained. It lags the current block only while a backlog carries over."
    ]);
    let old_cursor = old_entries
        .iter()
        .find(|entry| entry["name"] == "NextExpiryBucket")
        .ok_or("missing baseline expiry cursor")?;
    if old_cursor["docs"] != old_docs {
        return Err("unexpected baseline expiry cursor docs".into());
    }
    let entries = mempool_storage_entries(after)?;
    let cursor = entries
        .iter()
        .find(|entry| entry["name"] == "NextExpiryBucket")
        .ok_or("missing candidate expiry cursor")?;
    if cursor["docs"]
        != serde_json::json!([
            " Next bucket visited by the fairness scan, independently of the head."
        ])
    {
        return Err("unexpected fairness cursor docs".into());
    }
    // Bind the new anchor to the existing cursor's exact type, default and
    // modifier. The remaining graph comparison checks that cursor unchanged.
    let mut expected = cursor.clone();
    expected["name"] = Value::from("OldestExpiryBucket");
    expected["docs"] =
        serde_json::json!([" Oldest expiry bucket not yet drained; advanced with bounded work."]);
    let anchors: Vec<_> = entries
        .iter()
        .filter(|entry| entry["name"] == "OldestExpiryBucket")
        .collect();
    if anchors.len() != 1 || *anchors[0] != expected {
        return Err("unexpected required expiry anchor".into());
    }
    entries.retain(|entry| entry["name"] != "OldestExpiryBucket");
    entries
        .iter_mut()
        .find(|entry| entry["name"] == "NextExpiryBucket")
        .unwrap()["docs"] = old_docs;
    Ok(())
}

fn normalize_mempool(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let baseline_docs = register_program_docs(before)?
        .as_array()
        .ok_or("invalid baseline register_job_spec docs")?
        .clone();
    if baseline_docs
        .iter()
        .any(|line| line == REGISTER_PROGRAM_DOCS[1])
    {
        return Err("unexpected baseline register_job_spec M1 docs".into());
    }
    let mut expected_docs = baseline_docs.clone();
    expected_docs.extend(REGISTER_PROGRAM_DOCS.iter().copied().map(Value::from));
    let docs = register_program_docs(after)?;
    if *docs != Value::Array(expected_docs) {
        return Err("unexpected required register_job_spec M1 documentation delta".into());
    }
    *docs = Value::Array(baseline_docs);
    if error_variants(before)?.len() != 36 {
        return Err("unexpected baseline mempool errors".into());
    }
    let expected_errors = serde_json::json!([
        {
            "name": "VmProgramNotFound",
            "index": 36,
            "docs": [
                "A referenced program is absent from the configured source."
            ]
        },
        {
            "name": "VmBadBytecode",
            "index": 37,
            "docs": [
                "A referenced program cannot be decoded."
            ]
        },
        {
            "name": "VmVerificationFailed",
            "index": 38,
            "docs": [
                "Retired (QUI-2516): registration no longer re-runs the checker; it",
                "checks only that a referenced program exists and decodes. Kept so",
                "no error index moves.",
            ]
        },
        {
            "name": "VmOutputType",
            "index": 39,
            "docs": [
                "The transform must return exactly one integer vector."
            ]
        },
        {
            "name": "VmOutputLength",
            "index": 40,
            "docs": [
                "The transformed vector must preserve the input spin count."
            ]
        },
        {
            "name": "VmOutputSpin",
            "index": 41,
            "docs": [
                "Every transformed spin must be minus one or plus one."
            ]
        },
        {
            "name": "VmExecutionFailed",
            "index": 42,
            "docs": [
                "The transform faulted during execution."
            ]
        },
        {
            "name": "VmStepLimitExceeded",
            "index": 43,
            "docs": [
                "The aggregate submission step budget was exhausted."
            ]
        },
        {
            "name": "VmMemoryLimitExceeded",
            "index": 44,
            "docs": [
                "The transform exceeded the per-execution memory bound."
            ]
        },
        {
            "name": "OrderSolverCapReached",
            "index": 45,
            "docs": ["An open order already holds the maximum number of distinct solvers."]
        }
    ]);
    let errors = error_variants(after)?;
    if errors.get(36..) != expected_errors.as_array().map(Vec::as_slice) {
        return Err("unexpected required mempool VM error delta".into());
    }
    errors.truncate(36);

    // Type references must alias the existing Xqvm u64/Weight constants.
    // Their definitions remain subject to the complete graph comparison.
    let xqvm_constants = after["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == "Xqvm"))
        .and_then(|pallet| pallet["constants"].as_array())
        .ok_or("missing Xqvm constants")?;
    // The step price derives from the generated weights, so a regeneration must
    // not need a new literal: the mempool value must equal Xqvm's.
    let xqvm_value = |name: &str| {
        xqvm_constants
            .iter()
            .find(|c| c["name"] == name)
            .and_then(|c| c.get("value"))
            .cloned()
            .ok_or("missing Xqvm constant value")
    };
    let mut expected_constants = Vec::new();
    let ty = xqvm_constants
        .iter()
        .find(|c| c["name"] == "MaxStepLimit")
        .and_then(|c| c.get("ty"))
        .ok_or("missing MaxStepLimit type")?
        .clone();
    let mut constant = serde_json::json!({
        "name": "MaxTransformStepsPerSubmission",
        "value": [
            64,
            75,
            76,
            0,
            0,
            0,
            0,
            0
        ],
        "docs": [
            " Total VM steps shared by every solution in one submission."
        ],
        "deprecation_info": "NotDeprecated"
    });
    constant["ty"] = ty;
    expected_constants.push(constant);
    let ty = xqvm_constants
        .iter()
        .find(|c| c["name"] == "MaxVmMemory")
        .and_then(|c| c.get("ty"))
        .ok_or("missing MaxVmMemory type")?
        .clone();
    let mut constant = serde_json::json!({
        "name": "MaxVmMemory",
        // 1 MiB (1_048_576 = 0x10_0000), little-endian u64. Lowered from 16 MiB
        // by W1-002 (2 MiB, then 1.5 MiB, then 1 MiB: owner decisions of
        // 2026-11-22); the mempool transform cap is MaxTransformVmMemory.
        "value": [
            0,
            0,
            16,
            0,
            0,
            0,
            0,
            0
        ],
        "docs": [
            " Per-execution memory limit; VMs are dropped between solutions."
        ],
        "deprecation_info": "NotDeprecated"
    });
    constant["ty"] = ty;
    expected_constants.push(constant);
    let ty = xqvm_constants
        .iter()
        .find(|c| c["name"] == "WeightPerStep")
        .and_then(|c| c.get("ty"))
        .ok_or("missing WeightPerStep type")?
        .clone();
    let mut constant = serde_json::json!({
        "name": "VmWeightPerStep",
        "docs": [
            " Measured VM work price, shared with pallet-xqvm."
        ],
        "deprecation_info": "NotDeprecated"
    });
    constant["ty"] = ty;
    constant["value"] = xqvm_value("WeightPerStep")?;
    expected_constants.push(constant);
    let baseline = before["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet["constants"].as_array())
        .ok_or("missing baseline mempool constants")?;
    if baseline.iter().any(|old| {
        expected_constants
            .iter()
            .any(|new| old["name"] == new["name"])
    }) {
        return Err("unexpected baseline mempool VM constants".into());
    }
    let constants = after["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet["constants"].as_array_mut())
        .ok_or("missing candidate mempool constants")?;
    if constants.get(baseline.len()..) != Some(expected_constants.as_slice()) {
        return Err("unexpected required mempool VM constant delta".into());
    }
    constants.truncate(baseline.len());
    Ok(())
}

struct Comparison {
    before: Vec<Value>,
    after: Vec<Value>,
    outer: BTreeSet<u64>,
    seen: BTreeSet<(u64, u64)>,
}
impl Comparison {
    fn ty(&mut self, a: u64, b: u64) -> Result<(), String> {
        if !self.seen.insert((a, b)) {
            return Ok(());
        }
        let left = self.before.get(a as usize).ok_or("missing baseline type")?["type"].clone();
        let mut right = self.after.get(b as usize).ok_or("missing candidate type")?["type"].clone();
        if self.outer.contains(&a) {
            let variants = right["def"]["variant"]["variants"]
                .as_array_mut()
                .ok_or("outer type is not an enum")?;
            let additions: Vec<_> = variants.iter().filter(|v| v["name"] == "Vesting").collect();
            if additions.len() != 1 || additions[0]["index"] != 32 {
                return Err("outer Vesting variant must appear exactly once at index 32".into());
            }
            variants.retain(|v| v["name"] != "Vesting");
        }
        self.value(&left, &right, &format!("type[{a} => {b}]"))
    }
    fn value(&mut self, a: &Value, b: &Value, path: &str) -> Result<(), String> {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                if a.keys().collect::<Vec<_>>() != b.keys().collect::<Vec<_>>() {
                    return Err(format!("{path}: metadata fields changed"));
                }
                for (key, left) in a {
                    let right = &b[key];
                    let location = format!("{path}.{key}");
                    let reference = matches!(
                        key.as_str(),
                        "type"
                            | "ty"
                            | "address_ty"
                            | "call_ty"
                            | "signature_ty"
                            | "implicit"
                            | "output"
                            | "call_enum_ty"
                            | "event_enum_ty"
                            | "error_enum_ty"
                            | "Plain"
                            | "key"
                            | "value"
                            | "bit_store_type"
                            | "bit_order_type"
                    );
                    if reference && left.is_u64() && right.is_u64() {
                        self.ty(left.as_u64().unwrap(), right.as_u64().unwrap())?;
                    } else if key == "tuple" {
                        let left = left.as_array().ok_or("invalid tuple")?;
                        let right = right.as_array().ok_or("invalid tuple")?;
                        if left.len() != right.len() {
                            return Err(format!("{location}: tuple arity changed"));
                        }
                        for (l, r) in left.iter().zip(right) {
                            self.ty(
                                l.as_u64().ok_or("tuple type id")?,
                                r.as_u64().ok_or("tuple type id")?,
                            )?;
                        }
                    } else {
                        self.value(left, right, &location)?;
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                if a.len() != b.len() {
                    return Err(format!("{path}: metadata length changed"));
                }
                for (i, (left, right)) in a.iter().zip(b).enumerate() {
                    self.value(left, right, &format!("{path}[{i}]"))?;
                }
            }
            _ => {
                if a != b {
                    return Err(format!("{path}: metadata value changed"));
                }
            }
        }
        Ok(())
    }
}

// QUIP-12 appends one error without changing any prior index or field.
pub fn staking_error_variants(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == "Staking"))
        .and_then(|pallet| pallet["error"]["ty"].as_u64())
        .ok_or("missing staking error type")?;
    let id = usize::try_from(id).map_err(|_| "staking error id does not fit usize")?;
    metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(|types| types.get_mut(id))
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "missing staking error variants".into())
}

fn normalize_commission_error(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let baseline = staking_error_variants(before)?;
    if baseline.len() != 34
        || baseline
            .last()
            .is_none_or(|v| v["name"] != "Restricted" || v["index"] != 33)
    {
        return Err("unexpected baseline staking error surface".into());
    }
    let candidate = staking_error_variants(after)?;
    let expected = serde_json::json!({
        "name": "CommissionChangeDuringElection",
        "index": 34,
        "docs": ["Commission cannot rise while an election is holding a voter snapshot."]
    });
    if candidate.len() != 35 || candidate.last() != Some(&expected) {
        return Err("unexpected commission error delta".into());
    }
    candidate.pop();
    Ok(())
}

// Reviewed XQVM repin delta: these strings are metadata, not a blanket
// documentation exemption. Resolve by pallet/call name so registry renumbering
// remains valid; every other field still undergoes the full graph comparison.
const BASELINE_STORE_PROGRAM_DOCS: &[&str] = &[
    "Store an XQVM program on-chain.",
    "",
    "The bytecode is decoded, which checks the XQBC container, and then",
    "statically verified, which checks the instruction stream. Both must",
    "pass before anything is written, so a program that is accepted here",
    "cannot fail `execute` for a reason the verifier covers.",
    "",
    "Verifying at store time rather than at execute time puts the cost",
    "on the account that introduced the program, once, instead of on",
    "every account that runs it. It is also what lets `execute` skip",
    "re-verification later (QUI-1057).",
    "",
    "The cost has two dimensions. Decoding is linear in the byte",
    "length, which the call arguments give. Verification is linear in",
    "the basic-block count (at a bounded nesting depth, which the",
    "per-block price assumes at its maximum), and the count is only",
    "known after decoding, so it is pre-charged at `MaxProgramBlocks`",
    "and refunded to the actual count -- the shape `execute` uses for",
    "its size.",
    "",
    "The program is stored keyed by its Blake2-256 hash for",
    "deduplication.",
];

const BASELINE_MAX_LOOP_DEPTH_DOCS: &[&str] = &[
    " Maximum static loop nesting depth of a stored program.",
    "",
    " The verifier walks every loop region's blocks once per enclosing",
    " loop, so its work is the block count times the nesting depth, and",
    " nested loops are cheap to write: one `RANGE` and one `NEXT` per",
    " level. Without this cap a program under `MaxProgramBlocks` could",
    " still make verification quadratic in its length. Measured",
    " natively, 2,048 nested `RANGE`/`NEXT` pairs take 255 ms to",
    " verify and 16,384 take 20 s -- and that cost is paid before the",
    " verifier gets round to rejecting them.",
];

const CANDIDATE_STORE_PROGRAM_DOCS: &[&str] = &[
    "Store an XQVM program on-chain.",
    "",
    "The bytecode is decoded, which checks the XQBC container, and must",
    "be the program's canonical encoding. The structural gate then",
    "bounds its shape (`MaxProgramBlocks`, `MaxLoopDepth`) and runs two",
    "lexical passes: every instruction must decode",
    "(`VerifierBadInstruction`), every jump must name an existing",
    "`TARGET` (`VerifierUndefinedJumpTarget`), loop nesting must balance,",
    "and no jump may cross a loop-body boundary (W1-011). Together they",
    "bound the loop frames a run can hold by `MaxLoopDepth` (QUI-2516).",
    "",
    "The gate does not prove register, type or stack properties. A",
    "stored program can still fault at `execute` for those reasons: the",
    "VM detects them at run time and the caller pays the pre-charged",
    "weight. `remove_program` returns the deposit of a program nobody",
    "can use.",
    "",
    "Decoding and both passes are linear in byte length up to a log",
    "factor: the jump-span pass binary-searches once per jump.",
    "",
    "The weight takes the byte length and the block count: the call",
    "pre-charges at `MaxProgramBlocks` and reports the weight at the",
    "verified block count. On the reference machine the block count",
    "has no measured cost beyond the per-byte one (the generated weight",
    "drops it), so the charge is linear in byte length and nothing is",
    "refunded; the refund path stays for a regeneration that measures",
    "a block cost.",
    "",
    "The program is stored keyed by its Blake2-256 hash for",
    "deduplication.",
];

const CANDIDATE_MAX_LOOP_DEPTH_DOCS: &[&str] = &[
    " Maximum static loop nesting depth of a stored program.",
    "",
    " Also the bound on the loop frames a run can hold. The structural",
    " gate rejects unbalanced nesting and every jump across a loop-body",
    " boundary (W1-011), so the run-time loop stack equals the lexical",
    " nesting at every pc. The runtime's W1-002 VM memory bound depends",
    " on it: raising it needs the census re-run.",
];

// QUI-2516 (A4): the structural gate replaces the full checker, so the
// constant docs that described the verifier's memory say what is bounded now.
const BASELINE_MAX_PROGRAM_BLOCKS_DOCS: &[&str] = &[
    " Maximum number of basic blocks a stored program may have, counted",
    " the way the verifier's CFG builder counts them: see",
    " [`ControlFlowShape`].",
    "",
    " Bounds the verifier's memory and time, which `MaxProgramSize`",
    " does not: the verifier allocates per basic block, about 3 KiB",
    " natively, and a block can be a single byte, so a 64 KiB program",
    " can describe ~65,000 of them and ask for ~200 MiB on a runtime",
    " heap of 128 MiB. A failed allocation inside Wasm traps the whole",
    " execution rather than returning a fault this pallet could report,",
    " which is the same reason `MaxVmMemory` exists for the VM's own",
    " allocations.",
];

const CANDIDATE_MAX_PROGRAM_BLOCKS_DOCS: &[&str] = &[
    " Maximum number of basic blocks a stored program may have, counted",
    " the way xqvm's CFG builder counts them: see [`ControlFlowShape`].",
    "",
    " Bounds what `store_program` allocates before it can report a",
    " fault, which `MaxProgramSize` does not: a block can be a single",
    " byte. Decoding records every `TARGET`, and the structural gate's",
    " jump-span pass records every loop opener, `NEXT` and jump; each of",
    " those starts a block. A failed allocation inside Wasm traps the",
    " whole execution rather than returning a fault this pallet could",
    " report, which is the same reason `MaxVmMemory` exists for the VM's",
    " own allocations.",
];

// QUI-2516 (A4): error variants whose docs changed in place. Names and
// indices are unchanged (append-only, R9 Q4); each pair is checked on both
// sides and the candidate restored to the baseline before the graph diff.
const XQVM_ERROR_DOC_DELTAS: &[(&str, &[&str], &[&str])] = &[
    (
        "InvalidBytecode",
        &[
            "The bytecode is not a well-formed XQBC container: bad magic,",
            "unsupported format version, length mismatch, or CRC-32 mismatch.",
            "",
            "This covers the container only. Faults in the instruction stream",
            "itself get their own variants below, from the verifier.",
        ],
        &[
            "The bytecode is not a well-formed XQBC container: bad magic,",
            "unsupported format version, length mismatch, or CRC-32 mismatch.",
            "",
            "This covers the container only. Faults in the instruction stream",
            "itself get their own variants below, from the structural gate.",
        ],
    ),
    (
        "VerifierBadInstruction",
        &["Verifier: an instruction is truncated or uses an unknown opcode."],
        &[
            "Structural gate: an instruction is truncated or uses an unknown",
            "opcode.",
        ],
    ),
    (
        "VerifierUndefinedJumpTarget",
        &["Verifier: a jump references a target that does not exist."],
        &["Structural gate: a jump references a target that does not exist."],
    ),
    (
        "VerifierLoopImbalance",
        &[
            "Verifier: loop opens and closes do not balance, or a loop-context",
            "read occurs with no active loop.",
        ],
        &[
            "Structural gate: loop opens and closes do not balance, or a",
            "loop-context read occurs with no active loop.",
        ],
    ),
    (
        "VerifierReadUnsetRegister",
        &[
            "Verifier: a register is read before it is written, on at least one",
            "path through the program.",
        ],
        &[
            "Retired (QUI-2516): only the full checker raised this, and",
            "`store_program` no longer runs it. The VM reports the fault at",
            "`execute` as `VmUnsetRegister`. Kept so no error index moves.",
        ],
    ),
    (
        "VerifierRegisterTypeMismatch",
        &["Verifier: a register is read at a type it was not written as."],
        &[
            "Retired (QUI-2516): only the full checker raised this, and",
            "`store_program` no longer runs it. The VM reports the fault at",
            "`execute` as `VmRegisterType`. Kept so no error index moves.",
        ],
    ),
    (
        "VerifierStackFault",
        &[
            "Verifier: the program can underflow or overflow the value stack, or",
            "reaches a join point at inconsistent stack depths.",
        ],
        &[
            "Retired (QUI-2516): only the full checker raised this, and",
            "`store_program` no longer runs it. The VM reports the fault at",
            "`execute` as `VmStackUnderflow` or `VmStackOverflow`. Kept so no",
            "error index moves.",
        ],
    ),
    (
        "TooManyBlocks",
        &[
            "The program has more basic blocks than `MaxProgramBlocks`.",
            "",
            "Checked before verification, because the verifier's memory is",
            "what the bound protects and it is spent during verification.",
        ],
        &[
            "The program has more basic blocks than `MaxProgramBlocks`.",
            "",
            "Checked before the structural gate's passes, whose memory it",
            "bounds.",
        ],
    ),
    (
        "LoopNestingTooDeep",
        &[
            "The program nests loops deeper than `MaxLoopDepth`.",
            "",
            "Checked before verification for the same reason as",
            "`TooManyBlocks`: the cost it bounds is spent inside the verifier,",
            "ahead of any fault the verifier would report.",
        ],
        &[
            "The program nests loops deeper than `MaxLoopDepth`.",
            "",
            "Checked before the structural gate's passes. The depth also bounds",
            "the loop frames a run can hold: see `MaxLoopDepth`.",
        ],
    ),
    (
        "VmUnsetRegister",
        &[
            "XQVM: a register was read before anything was written to it.",
            "",
            "The verifier rejects this statically at `store_program`, so",
            "reaching it at run time means a path it could not prove.",
        ],
        &[
            "XQVM: a register was read before anything was written to it.",
            "",
            "`store_program` does not check this (QUI-2516); the VM raises it",
            "when a run reaches the read.",
        ],
    ),
];

// W1-008 part 2: the `execute` dispatchable gained an output-slot cost
// dimension, and its doc comment now documents it. Dispatchable docs are part
// of the call variant's metadata, so this is a reviewed documentation delta
// (no behavioural metadata change: no constant moved, the weight is not a
// constant). Baseline is the `execute` call docs in the pinned pre-vesting
// metadata; candidate is the current build.
const BASELINE_EXECUTE_DOCS: &[&str] = &[
    "Execute a stored XQVM program.",
    "",
    "Both dimensions of the cost are caller-influenced and neither is",
    "known from the call arguments alone, so both are pre-charged at",
    "their worst case and refunded via `PostDispatchInfo`:",
    "",
    "* **Program size.** The call carries only a hash, so the length is",
    "  unknown until storage is read. Decoding runs a full verifier scan",
    "  over every instruction, which is linear in the byte length, so",
    "  `MaxProgramSize` is pre-charged and the actual length refunded.",
    "* **Steps.** Pre-charged on the caller's `step_limit`, refunded to",
    "  the steps actually executed.",
    "",
    "The error path deliberately does not refund: over-charging a failed",
    "execution is the conservative direction.",
];

const CANDIDATE_EXECUTE_DOCS: &[&str] = &[
    "Execute a stored XQVM program.",
    "",
    "Both dimensions of the cost are caller-influenced and neither is",
    "known from the call arguments alone, so both are pre-charged at",
    "their worst case and refunded via `PostDispatchInfo`:",
    "",
    "* **Program size.** The call carries only a hash, so the length is",
    "  unknown until storage is read. Decoding runs xqvm's instruction",
    "  scan over every instruction, which is linear in the byte length, so",
    "  `MaxProgramSize` is pre-charged and the actual length refunded.",
    "* **Steps.** Pre-charged on the caller's `step_limit`, refunded to",
    "  the steps actually executed.",
    "",
    "A third dimension is pre-charged but deliberately **not** refunded:",
    "",
    "* **Output slots (W1-008 part 2).** The caller declares",
    "  `output_slots`, which sizes the VM's output vector up front. The",
    "  declared count is known from the call arguments, so it is charged",
    "  exactly — pre-charged and kept in the actual weight — rather than",
    "  charged at `MaxOutputSlots` and refunded. Charging the declared",
    "  count on both sides means a caller cannot declare a large output",
    "  area, pay the base weight, and have the output cost refunded away.",
    "",
    "The error path deliberately does not refund: over-charging a failed",
    "execution is the conservative direction.",
];

fn xqvm_variants<'a>(metadata: &'a mut Value, kind: &str) -> Result<&'a mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == "Xqvm"))
        .and_then(|pallet| pallet[kind]["ty"].as_u64())
        .ok_or_else(|| format!("missing Xqvm.{kind} type"))?;
    let id = usize::try_from(id).map_err(|_| "Xqvm type id does not fit usize")?;
    metadata["types"]["types"]
        .get_mut(id)
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("missing Xqvm.{kind} variants"))
}

fn xqvm_docs<'a>(metadata: &'a mut Value, name: &str) -> Result<&'a mut Value, String> {
    // Dispatchable docs live on the call variant; constant docs on the
    // constant. Resolve by name against the calls first (`store_program`,
    // `execute`), then fall back to the constants (e.g. `MaxLoopDepth`).
    // Resolving by name keeps this valid under registry renumbering.
    let is_call = xqvm_variants(metadata, "calls")?
        .iter()
        .any(|call| call["name"] == name);
    if is_call {
        return xqvm_variants(metadata, "calls")?
            .iter_mut()
            .find(|call| call["name"] == name)
            .and_then(|call| call.get_mut("docs"))
            .ok_or_else(|| format!("missing Xqvm.{name} call docs"));
    }
    metadata["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == "Xqvm"))
        .and_then(|pallet| pallet["constants"].as_array_mut())
        .and_then(|constants| {
            constants
                .iter_mut()
                .find(|constant| constant["name"] == name)
        })
        .and_then(|constant| constant.get_mut("docs"))
        .ok_or_else(|| format!("missing Xqvm.{name} docs"))
}

fn normalize_xqvm(before: &mut Value, after: &mut Value) -> Result<(), String> {
    for (name, baseline, candidate) in [
        (
            "store_program",
            BASELINE_STORE_PROGRAM_DOCS,
            CANDIDATE_STORE_PROGRAM_DOCS,
        ),
        (
            "MaxLoopDepth",
            BASELINE_MAX_LOOP_DEPTH_DOCS,
            CANDIDATE_MAX_LOOP_DEPTH_DOCS,
        ),
        ("execute", BASELINE_EXECUTE_DOCS, CANDIDATE_EXECUTE_DOCS),
        (
            "MaxProgramBlocks",
            BASELINE_MAX_PROGRAM_BLOCKS_DOCS,
            CANDIDATE_MAX_PROGRAM_BLOCKS_DOCS,
        ),
    ] {
        if *xqvm_docs(before, name)? != serde_json::json!(baseline) {
            return Err(format!("unexpected baseline Xqvm.{name} docs"));
        }
        let docs = xqvm_docs(after, name)?;
        if *docs != serde_json::json!(candidate) {
            return Err(format!(
                "unexpected required Xqvm.{name} documentation delta"
            ));
        }
        *docs = serde_json::json!(baseline);
    }
    for &(name, baseline_docs, candidate_docs) in XQVM_ERROR_DOC_DELTAS {
        let before_docs = xqvm_variants(before, "error")?
            .iter()
            .find(|variant| variant["name"] == name)
            .map(|variant| variant["docs"].clone())
            .ok_or_else(|| format!("missing baseline Xqvm.{name} error"))?;
        if before_docs != serde_json::json!(baseline_docs) {
            return Err(format!("unexpected baseline Xqvm.{name} error docs"));
        }
        let variant = xqvm_variants(after, "error")?
            .iter_mut()
            .find(|variant| variant["name"] == name)
            .ok_or_else(|| format!("missing Xqvm.{name} error"))?;
        if variant["docs"] != serde_json::json!(candidate_docs) {
            return Err(format!(
                "unexpected required Xqvm.{name} error documentation delta"
            ));
        }
        variant["docs"] = before_docs;
    }
    let baseline = xqvm_variants(before, "error")?;
    if baseline.len() != 37
        || baseline
            .last()
            .is_none_or(|v| v["name"] != "VmSampleOutOfDomain" || v["index"] != 36)
    {
        return Err("unexpected baseline Xqvm error surface".into());
    }
    let candidate = xqvm_variants(after, "error")?;
    let analysis_budget = serde_json::json!({
        "name": "VerifierAnalysisBudgetExceeded",
        "index": 37,
        "docs": [
            "Retired (QUI-2516): only the full checker's analyses raised this,",
            "and `store_program` no longer runs them. Kept so no error index",
            "moves.",
        ]
    });
    // W1-003a: a second appended error, index preserved after the budget error.
    let non_canonical = serde_json::json!({
        "name": "NonCanonicalProgram",
        "index": 38,
        "docs": [
            "The stored bytes are not the program's canonical encoding (W1-003a):",
            "`Program::decode` recomputes the XQBC header's slot bytes, so bytes",
            "that differ only in the header decode to the same program yet hash",
            "differently. Rejected so `hash(bytecode)` is a stable identity.",
            "Appended to preserve every existing pallet error index."
        ]
    });
    // W1-011 (xqvm 0.6.0): a third appended error for the jump-span phase.
    let jump_across_loop = serde_json::json!({
        "name": "VerifierJumpAcrossLoopBoundary",
        "index": 39,
        "docs": [
            "A jump's target lies in a different loop body from the jump",
            "(W1-011, xqvm 0.6.0): a back-edge that re-enters a loop opener",
            "without its `NEXT`, or a jump into or out of a loop body.",
            "Appended to preserve every existing pallet error index."
        ]
    });
    // xqvm 0.6.0 (A4): a fourth appended error for `NonBinaryModel`.
    let non_binary_model = serde_json::json!({
        "name": "VmNonBinaryModel",
        "index": 40,
        "docs": [
            "XQVM: a constraint opcode (`ONEHOTR`, `ONEHOTC`, `EXCLUDE`,",
            "`IMPLIES`, `EQUALITY`, `ATLEAST`, `ATLEASTW`, `REDUCE`) was applied",
            "to a model whose declared domain is not binary (xqvm 0.6.0).",
            "Appended to preserve every existing pallet error index.",
        ]
    });
    if candidate.len() != 41
        || candidate.get(37) != Some(&analysis_budget)
        || candidate.get(38) != Some(&non_canonical)
        || candidate.get(39) != Some(&jump_across_loop)
        || candidate.get(40) != Some(&non_binary_model)
    {
        return Err("unexpected Xqvm appended-error delta".into());
    }
    candidate.truncate(37);
    Ok(())
}

// W3-009 (QuantumPow: one accepted proof per miner per block). Exact deltas:
// one error appended at the end of the QuantumPow error enum,
// `ProofAlreadySubmittedThisBlock`, and one new storage value,
// `BlockSubmitters`, right after `BlockBestProof`: a bounded set of the same
// account type as the `Miners` key, empty by default.
const PROOF_ALREADY_SUBMITTED_DOCS: &[&str] = &[
    "This miner already had a proof accepted in this block: one",
    "`submit_proof` per miner per block (W3-009).",
];
const BLOCK_SUBMITTERS_DOCS: &[&str] = &[
    " The miners that submitted an accepted proof in this block (W3-009):",
    " one `submit_proof` per miner per block, checked before the",
    " `MaxProofsPerBlock` limit so one miner cannot take every slot. Cleared",
    " in `on_initialize`; bounded by `MaxProofsPerBlock`, since each accepted",
    " proof adds one miner and the block accepts at most that many.",
];

fn pow_errors(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == "QuantumPow"))
        .and_then(|pallet| pallet["error"]["ty"].as_u64())
        .ok_or("missing QuantumPow.error type")?;
    let id = usize::try_from(id).map_err(|_| "QuantumPow error type id does not fit usize")?;
    metadata["types"]["types"]
        .get_mut(id)
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "missing QuantumPow.error variants".into())
}

fn normalize_block_submitters(before: &mut Value, after: &mut Value) -> Result<(), String> {
    // The appended error.
    let baseline = pow_errors(before)?;
    if baseline
        .iter()
        .any(|v| v["name"] == "ProofAlreadySubmittedThisBlock")
    {
        return Err("unexpected baseline QuantumPow ProofAlreadySubmittedThisBlock".into());
    }
    let base_len = baseline.len();
    let candidate = pow_errors(after)?;
    let expected = serde_json::json!({
        "name": "ProofAlreadySubmittedThisBlock",
        "index": base_len,
        "docs": PROOF_ALREADY_SUBMITTED_DOCS,
    });
    if candidate.len() != base_len + 1 || candidate.last() != Some(&expected) {
        return Err(format!(
            "missing or incorrect required QuantumPow appended error: {}",
            serde_json::to_string(&candidate.last()).unwrap_or_default()
        ));
    }
    candidate.pop();

    // The new storage value.
    if pow_storage_entries(before)?
        .iter()
        .any(|entry| entry["name"] == "BlockSubmitters")
    {
        return Err("unexpected baseline QuantumPow BlockSubmitters".into());
    }
    let types = after.clone();
    let entries = pow_storage_entries(after)?;
    let best_at = entries
        .iter()
        .position(|entry| entry["name"] == "BlockBestProof")
        .ok_or("missing candidate BlockBestProof")?;
    let found: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry["name"] == "BlockSubmitters")
        .map(|(index, _)| index)
        .collect();
    if found != [best_at + 1] {
        return Err("missing or misplaced required QuantumPow BlockSubmitters".into());
    }
    let submitters = entries[best_at + 1].clone();
    let account = entries
        .iter()
        .find(|entry| entry["name"] == "Miners")
        .map(|miners| miners["ty"]["Map"]["key"].clone())
        .ok_or("missing candidate Miners")?;
    let expected = serde_json::json!({
        "default": [0],
        "deprecation_info": "NotDeprecated",
        "docs": BLOCK_SUBMITTERS_DOCS,
        "modifier": "Default",
        "name": "BlockSubmitters",
        "ty": {"Plain": submitters["ty"]["Plain"].clone()},
    });
    if submitters != expected {
        return Err(format!(
            "incorrect required QuantumPow BlockSubmitters entry: {}",
            serde_json::to_string(&submitters).unwrap_or_default()
        ));
    }
    // The value: a `BoundedBTreeSet` of the miner account type, followed
    // through its wrapper composites down to the element sequence.
    let set = registry_type(&types, &submitters["ty"]["Plain"])
        .ok_or("missing BlockSubmitters value type")?;
    if set["path"].as_array().and_then(|p| p.last()) != Some(&Value::from("BoundedBTreeSet")) {
        return Err(format!(
            "BlockSubmitters is not a BoundedBTreeSet: {}",
            serde_json::to_string(&set["path"]).unwrap_or_default()
        ));
    }
    let mut ty = set;
    let element = loop {
        if let Some(element) = ty.pointer("/def/sequence/type") {
            break element.clone();
        }
        let inner = ty
            .pointer("/def/composite/fields")
            .and_then(Value::as_array)
            .filter(|fields| fields.len() == 1)
            .map(|fields| fields[0]["type"].clone())
            .ok_or("BlockSubmitters value does not reduce to a sequence")?;
        ty = registry_type(&types, &inner).ok_or("missing BlockSubmitters inner type")?;
    };
    if element != account {
        return Err("BlockSubmitters element is not the Miners account type".into());
    }
    pow_storage_entries(after)?.remove(best_at + 1);
    Ok(())
}

// W1-009 (MinerRegistry retention pruning and bounded paging). Exact deltas:
// two constants appended to the pallet, a new `PruneCursor` storage value, and
// MinerRegistryApi moving to version 2 with the storage-order paging docs.
const MINER_REGISTRY: &str = "MinerRegistry";
const MINER_REGISTRY_API: &str = "MinerRegistryApi";

const BASELINE_PARTICIPANTS_DOCS: &[&str] = &[
    " Participants of `qblock_id`, sorted by account id for stable",
    " pagination. `start_after` returns only accounts strictly greater",
    " than the cursor; `limit` is capped server-side.",
];

const CANDIDATE_PARTICIPANTS_DOCS: &[&str] = &[
    " Participants of `qblock_id` with their records, in the double map's",
    " storage order — the Blake2_128Concat order of the account keys, not",
    " account-id order (the version-2 contract, O-D2). `start_after`",
    " returns only entries strictly after that account's storage key, and",
    " works even when the cursor account no longer has a record; `limit` is",
    " capped server-side. The page is read directly from the second-key",
    " iterator, so a call costs `O(limit)`, not `O(participants)` (W1-009).",
];

const PARTICIPATION_RETENTION_DOCS: &[&str] = &[
    " Number of most-recent qblocks of participation history to retain.",
    " `on_idle` prunes `ParticipantsByQBlock` / `ParticipantCountByQBlock`",
    " for every qblock id older than `candidate - ParticipationRetention`.",
    "",
    " Testnet is 5,040 (O-D1). The subnet coordinator that consumes this",
    " index reads only the current candidate qblock's participants, so the",
    " on-chain record exists purely for recent-era auditing, not for the",
    " live protocol. 5,040 = 28 x 180: 180 qblocks form per day (one every",
    " 80 blocks x 6 s = 480 s, and 86,400 s / 480 s = 180), retained for 28",
    " days. Beyond that window no on-chain or off-chain reader consults the",
    " record, so it is pruned to bound unbounded growth (W1-009).",
];

const MAX_PARTICIPATION_PRUNE_DOCS: &[&str] = &[
    " Maximum participation entries `on_idle` removes in one block — the",
    " `clear_prefix` limit. Bounds the per-block pruning cost, so a qblock",
    " with many participants is cleared over several blocks rather than in",
    " a single oversized `on_idle`.",
];

const PRUNE_CURSOR_DOCS: &[&str] = &[
    " The next qblock id `on_idle` will consider for retention pruning. Starts",
    " at zero and only advances, so pruning never revisits a cleared qblock.",
    " Only the qblock id is persisted: `clear_prefix` deletes keys in place, so",
    " a qblock that is only partially cleared in one block resumes from its",
    " surviving keys next block, with no sub-cursor to store (W1-009).",
];

fn miner_registry_section<'a>(
    metadata: &'a mut Value,
    section: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let pallet = metadata["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == MINER_REGISTRY))
        .ok_or("missing MinerRegistry pallet")?;
    let list = if section == "storage" {
        pallet["storage"]["entries"].as_array_mut()
    } else {
        pallet[section].as_array_mut()
    };
    list.ok_or_else(|| format!("missing MinerRegistry {section}"))
}

fn participants_api<'a>(metadata: &'a mut Value) -> Result<&'a mut Value, String> {
    metadata["apis"]
        .as_array_mut()
        .and_then(|apis| {
            apis.iter_mut()
                .find(|api| api["name"] == MINER_REGISTRY_API)
        })
        .ok_or_else(|| "missing MinerRegistryApi".into())
}

/// The primitive a portable type id resolves to, if it is a primitive.
fn primitive_of(metadata: &Value, ty: &Value) -> Option<String> {
    let id = usize::try_from(ty.as_u64()?).ok()?;
    metadata["types"]["types"]
        .get(id)?
        .pointer("/type/def/primitive")?
        .as_str()
        .map(str::to_owned)
}

fn normalize_miner_registry(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let new_constants = ["ParticipationRetention", "MaxParticipationPrunePerBlock"];
    if miner_registry_section(before, "constants")?
        .iter()
        .any(|c| new_constants.iter().any(|name| c["name"] == *name))
    {
        return Err("unexpected baseline MinerRegistry retention constant".into());
    }
    if miner_registry_section(before, "storage")?
        .iter()
        .any(|e| e["name"] == "PruneCursor")
    {
        return Err("unexpected baseline MinerRegistry prune cursor".into());
    }

    // Constants: exactly these two, appended in this order, exact values/docs.
    let types = after.clone();
    let constants = miner_registry_section(after, "constants")?;
    let expected: [(&str, &str, Vec<u8>, &[&str]); 2] = [
        (
            "ParticipationRetention",
            "u64",
            5_040u64.to_le_bytes().to_vec(),
            PARTICIPATION_RETENTION_DOCS,
        ),
        (
            "MaxParticipationPrunePerBlock",
            "u32",
            1_000u32.to_le_bytes().to_vec(),
            MAX_PARTICIPATION_PRUNE_DOCS,
        ),
    ];
    let tail = constants
        .len()
        .checked_sub(2)
        .ok_or("missing required MinerRegistry retention constants")?;
    for (offset, (name, primitive, value, docs)) in expected.iter().enumerate() {
        let found = &constants[tail + offset];
        let want = serde_json::json!({
            "deprecation_info": "NotDeprecated",
            "docs": docs,
            "name": name,
            "ty": found["ty"].clone(),
            "value": value,
        });
        if *found != want || primitive_of(&types, &found["ty"]).as_deref() != Some(*primitive) {
            return Err(format!(
                "missing or incorrect required MinerRegistry constant {name}"
            ));
        }
    }
    constants.truncate(tail);

    // Storage: one new PruneCursor value, u64, default 0, exact docs.
    let entries = miner_registry_section(after, "storage")?;
    let found: Vec<&Value> = entries
        .iter()
        .filter(|e| e["name"] == "PruneCursor")
        .collect();
    let ok = found.len() == 1 && {
        let cursor = found[0];
        let want = serde_json::json!({
            "default": [0, 0, 0, 0, 0, 0, 0, 0],
            "deprecation_info": "NotDeprecated",
            "docs": PRUNE_CURSOR_DOCS,
            "modifier": "Default",
            "name": "PruneCursor",
            "ty": {"Plain": cursor["ty"]["Plain"].clone()},
        });
        *cursor == want && primitive_of(&types, &cursor["ty"]["Plain"]).as_deref() == Some("u64")
    };
    if !ok {
        return Err("missing or incorrect required MinerRegistry prune cursor".into());
    }
    entries.retain(|e| e["name"] != "PruneCursor");

    // Runtime API: version 1 -> 2, and the paging docs of participants_by_qblock.
    let docs_of = |api: &Value| -> Option<Value> {
        api["methods"]
            .as_array()?
            .iter()
            .find(|m| m["name"] == "participants_by_qblock")
            .map(|m| m["docs"].clone())
    };
    let old_api = participants_api(before)?;
    if old_api["version"] != 1
        || docs_of(old_api) != Some(serde_json::json!(BASELINE_PARTICIPANTS_DOCS))
    {
        return Err("unexpected baseline MinerRegistryApi".into());
    }
    let new_api = participants_api(after)?;
    if new_api["version"] != 2 {
        return Err("missing required MinerRegistryApi version 2".into());
    }
    if docs_of(new_api) != Some(serde_json::json!(CANDIDATE_PARTICIPANTS_DOCS)) {
        return Err("missing or incorrect required MinerRegistryApi paging docs".into());
    }
    // System::Version encodes the runtime API list as (blake2_64(name),
    // version) pairs, so the bump also moves exactly one u32 there.
    let api_id = sp_core::hashing::blake2_64(MINER_REGISTRY_API.as_bytes());
    let bytes = |value: &Value| -> Result<Vec<u8>, String> {
        serde_json::from_value(value.clone()).map_err(|_| "invalid System::Version bytes".into())
    };
    let old_version = bytes(constant_value(before, "System", "Version")?)?;
    let new_version = bytes(constant_value(after, "System", "Version")?)?;
    let entry = |version: u32| -> Vec<u8> {
        let mut entry = api_id.to_vec();
        entry.extend(version.to_le_bytes());
        entry
    };
    let (old_entry, new_entry) = (entry(1), entry(2));
    let at: Vec<usize> = old_version
        .windows(old_entry.len())
        .enumerate()
        .filter(|(_, w)| *w == old_entry.as_slice())
        .map(|(i, _)| i)
        .collect();
    let [at] = at.as_slice() else {
        return Err("unexpected baseline System::Version MinerRegistryApi entry".into());
    };
    let mut expected = old_version.clone();
    expected[*at..*at + new_entry.len()].copy_from_slice(&new_entry);
    if new_version != expected {
        return Err("missing or incorrect required System::Version MinerRegistryApi entry".into());
    }
    *constant_value(after, "System", "Version")? = serde_json::json!(old_version);
    let new_api = participants_api(after)?;
    new_api["version"] = Value::from(1);
    if let Some(method) = new_api["methods"].as_array_mut().and_then(|methods| {
        methods
            .iter_mut()
            .find(|m| m["name"] == "participants_by_qblock")
    }) {
        method["docs"] = serde_json::json!(BASELINE_PARTICIPANTS_DOCS);
    }
    Ok(())
}

// W1-007 (QuantumPow topology summary). Exact delta: one new optional map,
// `TopologySummary`, right after `RegisteredTopologies` with the same hasher and
// key, whose value is `TopologySummaryInfo { nodes: u32, edges: u32,
// allowed_h_values, allowed_j_values }` using the meta's own value-spec type.
const TOPOLOGY_SUMMARY_DOCS: &[&str] = &[
    " Dimensions and h/j value specs of each registered topology (W1-007),",
    " written with `RegisteredTopologies` and never removed. The source of",
    " truth for registration: weights, difficulty curves and the difficulty",
    " API read only this, so they never decode a full topology.",
];

fn pow_storage_entries(metadata: &mut Value) -> Result<&mut Vec<Value>, String> {
    metadata["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == "QuantumPow"))
        .and_then(|pallet| pallet["storage"]["entries"].as_array_mut())
        .ok_or_else(|| "missing QuantumPow storage".into())
}

fn registry_type(metadata: &Value, id: &Value) -> Option<Value> {
    let id = usize::try_from(id.as_u64()?).ok()?;
    metadata["types"]["types"]
        .get(id)
        .map(|ty| ty["type"].clone())
}

fn normalize_topology_summary(before: &mut Value, after: &mut Value) -> Result<(), String> {
    if pow_storage_entries(before)?
        .iter()
        .any(|entry| entry["name"] == "TopologySummary")
    {
        return Err("unexpected baseline QuantumPow topology summary".into());
    }
    let types = after.clone();
    let entries = pow_storage_entries(after)?;
    let meta_at = entries
        .iter()
        .position(|entry| entry["name"] == "RegisteredTopologies")
        .ok_or("missing candidate RegisteredTopologies")?;
    let found: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry["name"] == "TopologySummary")
        .map(|(index, _)| index)
        .collect();
    if found != [meta_at + 1] {
        return Err("missing or misplaced required QuantumPow topology summary".into());
    }
    let meta = entries[meta_at].clone();
    let summary = entries[meta_at + 1].clone();
    let expected = serde_json::json!({
        "default": [0],
        "deprecation_info": "NotDeprecated",
        "docs": TOPOLOGY_SUMMARY_DOCS,
        "modifier": "Optional",
        "name": "TopologySummary",
        "ty": {"Map": {
            "hashers": meta["ty"]["Map"]["hashers"].clone(),
            "key": meta["ty"]["Map"]["key"].clone(),
            "value": summary["ty"]["Map"]["value"].clone(),
        }},
    });
    if summary != expected {
        return Err("incorrect required QuantumPow topology summary entry".into());
    }
    // The value type: the exact struct, its u32 dimensions, and the same
    // value-spec type the meta uses for its allowed_h_values.
    let value = registry_type(&types, &summary["ty"]["Map"]["value"])
        .ok_or("missing topology summary value type")?;
    let meta_value = registry_type(&types, &meta["ty"]["Map"]["value"])
        .ok_or("missing topology meta value type")?;
    let spec = meta_value
        .pointer("/def/composite/fields")
        .and_then(Value::as_array)
        .and_then(|fields| fields.iter().find(|f| f["name"] == "allowed_h_values"))
        .map(|field| field["type"].clone())
        .ok_or("missing meta allowed_h_values type")?;
    let fields = value
        .pointer("/def/composite/fields")
        .and_then(Value::as_array)
        .ok_or("topology summary value is not a struct")?;
    let shape: Vec<(String, Option<String>, Value)> = fields
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap_or_default().to_owned(),
                registry_type(&types, &f["type"]).and_then(|ty| {
                    ty.pointer("/def/primitive")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
                f["type"].clone(),
            )
        })
        .collect();
    let ok = value["path"]
        == serde_json::json!(["pallet_quantum_pow", "types", "TopologySummaryInfo"])
        && shape.len() == 4
        && shape[0].0 == "nodes"
        && shape[0].1.as_deref() == Some("u32")
        && shape[1].0 == "edges"
        && shape[1].1.as_deref() == Some("u32")
        && shape[2].0 == "allowed_h_values"
        && shape[2].2 == spec
        && shape[3].0 == "allowed_j_values"
        && shape[3].2 == spec;
    if !ok {
        return Err("incorrect required QuantumPow topology summary value type".into());
    }
    pow_storage_entries(after)?.remove(meta_at + 1);
    Ok(())
}

// W1-006 (compute-mempool reward escrow). Exact deltas: the appended
// `RewardNotSpendable` error, the reworded `RewardReserveMissing` docs, the
// appended `EscrowFunded` / `EscrowReleased` events (field types bound to the
// existing `RewardReclaimed` event's), and the appended `EscrowPalletId`
// constant (`frame_support::PalletId`, `b"qp/escrw"`).
const BASELINE_RESERVE_MISSING_DOCS: &[&str] =
    &["Reserved funds are insufficient; retain the order and retry after recovery."];
const ESCROW_RESERVE_MISSING_DOCS: &[&str] = &[
    "The order's escrow account does not hold its reward; the order is",
    "kept and nothing is paid.",
];

fn mempool_variants<'a>(metadata: &'a mut Value, kind: &str) -> Result<&'a mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet[kind]["ty"].as_u64())
        .ok_or_else(|| format!("missing mempool {kind} type"))?;
    let id = usize::try_from(id).map_err(|_| "mempool type id does not fit usize")?;
    metadata["types"]["types"]
        .get_mut(id)
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("missing mempool {kind} variants"))
}

fn normalize_reward_escrow(before: &mut Value, after: &mut Value) -> Result<(), String> {
    // Errors.
    let old_errors = mempool_variants(before, "error")?.clone();
    if old_errors.iter().any(|v| v["name"] == "RewardNotSpendable")
        || old_errors
            .iter()
            .find(|v| v["name"] == "RewardReserveMissing")
            .map(|v| v["docs"].clone())
            != Some(serde_json::json!(BASELINE_RESERVE_MISSING_DOCS))
    {
        return Err("unexpected baseline mempool escrow errors".into());
    }
    let errors = mempool_variants(after, "error")?;
    let expected = serde_json::json!({
        "docs": [
            "The reward is not spendable from the proposer's balance: it is",
            "unavailable, frozen (for example vesting), or would leave the",
            "proposer below the existential deposit."
        ],
        "index": 46,
        "name": "RewardNotSpendable",
    });
    if errors.len() != 47 || errors[46] != expected {
        return Err("missing or incorrect required RewardNotSpendable error".into());
    }
    errors.truncate(46);
    let missing = errors
        .iter_mut()
        .find(|v| v["name"] == "RewardReserveMissing")
        .ok_or("missing RewardReserveMissing")?;
    if missing["docs"] != serde_json::json!(ESCROW_RESERVE_MISSING_DOCS) {
        return Err("missing or incorrect required RewardReserveMissing docs".into());
    }
    missing["docs"] = serde_json::json!(BASELINE_RESERVE_MISSING_DOCS);

    // Events.
    let old_events = mempool_variants(before, "event")?.len();
    let events = mempool_variants(after, "event")?;
    let template = events
        .iter()
        .find(|v| v["name"] == "RewardReclaimed")
        .ok_or("missing RewardReclaimed event")?
        .clone();
    let field = |name: &str| -> Value {
        template["fields"]
            .as_array()
            .and_then(|fields| fields.iter().find(|f| f["name"] == name))
            .map(|f| f["type"].clone())
            .unwrap_or(Value::Null)
    };
    let expected_events = serde_json::json!([
        {
            "docs": ["The order's reward moved from the proposer into its escrow account."],
            "fields": [
                {"name": "order_id", "type": field("order_id"), "typeName": "u64"},
                {"name": "escrow", "type": field("proposer"), "typeName": "T::AccountId"},
                {"name": "amount", "type": field("amount"), "typeName": "BalanceOf<T>"}
            ],
            "index": old_events,
            "name": "EscrowFunded"
        },
        {
            "docs": [
                "The order closed and its escrow account was emptied: `to_proposer`",
                "went back to the proposer and the rest to the paid winners."
            ],
            "fields": [
                {"name": "order_id", "type": field("order_id"), "typeName": "u64"},
                {"name": "escrow", "type": field("proposer"), "typeName": "T::AccountId"},
                {"name": "to_proposer", "type": field("amount"), "typeName": "BalanceOf<T>"}
            ],
            "index": old_events + 1,
            "name": "EscrowReleased"
        }
    ]);
    if events.get(old_events..) != expected_events.as_array().map(Vec::as_slice) {
        return Err("missing or incorrect required escrow events".into());
    }
    events.truncate(old_events);

    // Constant.
    let types = after.clone();
    let constants = after["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == MEMPOOL))
        .and_then(|pallet| pallet["constants"].as_array_mut())
        .ok_or("missing candidate mempool constants")?;
    let last = constants
        .last()
        .cloned()
        .ok_or("missing mempool constants")?;
    let path = usize::try_from(last["ty"].as_u64().unwrap_or(u64::MAX))
        .ok()
        .and_then(|id| types["types"]["types"].get(id))
        .map(|ty| ty["type"]["path"].clone());
    let want = serde_json::json!({
        "deprecation_info": "NotDeprecated",
        "docs": [
            " Root of the per-order reward escrow accounts (W1-006): order `id`'s",
            " reward is held in `EscrowPalletId.into_sub_account_truncating(id)`",
            " from proposal until the order closes."
        ],
        "name": "EscrowPalletId",
        "ty": last["ty"].clone(),
        "value": b"qp/escrw".to_vec(),
    });
    if last != want || path != Some(serde_json::json!(["frame_support", "PalletId"])) {
        return Err("missing or incorrect required EscrowPalletId constant".into());
    }
    constants.pop();
    Ok(())
}

// W1-012 (rank distinct answers). Exact deltas: the appended `DuplicateAnswer`
// error (after W1-006's `RewardNotSpendable`), the `answer: H256` field
// appended to `JobSolution` and `RankedSolver`, and the `OrderAnswers` index
// directly after `OrderSolutions`, keyed (order id, answer) -> solver with the
// same order-id, account and hasher types as `OrderSolutions`.
const ORDER_ANSWERS_DOCS: &[&str] = &[
    " Answer index (W1-012): for each order, every solver's current canonical",
    " answer, mapped to that solver. At most one entry per solver, so at most",
    " `MAX_SOLVERS_PER_OPEN_ORDER` per order; cleared with the order's",
    " solutions. Makes the duplicate-answer check a single read instead of a",
    " scan of the (large) solution payloads.",
];

fn is_h256(metadata: &Value, id: &Value) -> bool {
    registry_type(metadata, id)
        .is_some_and(|ty| ty["path"] == serde_json::json!(["primitive_types", "H256"]))
}

fn tuple_fields(metadata: &Value, id: &Value) -> Option<Vec<Value>> {
    registry_type(metadata, id)?
        .pointer("/def/tuple")
        .and_then(Value::as_array)
        .cloned()
}

fn normalize_answer_index(before: &mut Value, after: &mut Value) -> Result<(), String> {
    // Error.
    if mempool_variants(before, "error")?
        .iter()
        .any(|v| v["name"] == "DuplicateAnswer")
    {
        return Err("unexpected baseline DuplicateAnswer error".into());
    }
    let errors = mempool_variants(after, "error")?;
    let expected = serde_json::json!({
        "docs": [
            "Another solver already submitted the same answer for this order",
            "(identical, globally flipped, or differing only in isolated spins)."
        ],
        "index": 47,
        "name": "DuplicateAnswer",
    });
    if errors.len() != 48 || errors[47] != expected {
        return Err("missing or incorrect required DuplicateAnswer error".into());
    }
    errors.pop();

    // Struct fields.
    let types = after.clone();
    // Field docs are not emitted in this metadata, so the field is name, type
    // and type name only.
    for name in ["JobSolution", "RankedSolver"] {
        let path = serde_json::json!(["pallet_quantum_compute_mempool", "types", name]);
        let mut found = 0;
        for ty in after["types"]["types"]
            .as_array_mut()
            .ok_or("missing candidate registry")?
            .iter_mut()
            .filter(|ty| ty["type"]["path"] == path)
        {
            let fields = ty
                .pointer_mut("/type/def/composite/fields")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| format!("{name} is not a struct"))?;
            let last = fields.last().cloned().unwrap_or(Value::Null);
            let want = serde_json::json!({
                "name": "answer",
                "type": last["type"].clone(),
                "typeName": "sp_core::H256",
            });
            if last != want || !is_h256(&types, &last["type"]) {
                return Err(format!("missing or incorrect required {name}.answer field"));
            }
            fields.pop();
            found += 1;
        }
        if found == 0 {
            return Err(format!("missing candidate {name} type"));
        }
    }

    // Storage.
    if mempool_storage_entries(before)?
        .iter()
        .any(|entry| entry["name"] == "OrderAnswers")
    {
        return Err("unexpected baseline OrderAnswers index".into());
    }
    let entries = mempool_storage_entries(after)?;
    let solutions_at = entries
        .iter()
        .position(|entry| entry["name"] == "OrderSolutions")
        .ok_or("missing candidate OrderSolutions")?;
    let found: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry["name"] == "OrderAnswers")
        .map(|(index, _)| index)
        .collect();
    if found != [solutions_at + 1] {
        return Err("missing or misplaced required OrderAnswers index".into());
    }
    let solutions = entries[solutions_at].clone();
    let index = entries[solutions_at + 1].clone();
    let solutions_key = tuple_fields(&types, &solutions["ty"]["Map"]["key"])
        .ok_or("OrderSolutions key is not a tuple")?;
    let index_key = tuple_fields(&types, &index["ty"]["Map"]["key"])
        .ok_or("OrderAnswers key is not a tuple")?;
    let expected = serde_json::json!({
        "default": [0],
        "deprecation_info": "NotDeprecated",
        "docs": ORDER_ANSWERS_DOCS,
        "modifier": "Optional",
        "name": "OrderAnswers",
        "ty": {"Map": {
            "hashers": solutions["ty"]["Map"]["hashers"].clone(),
            "key": index["ty"]["Map"]["key"].clone(),
            "value": solutions_key.get(1).cloned().unwrap_or(Value::Null),
        }},
    });
    let key_ok = solutions_key.len() == 2
        && index_key.len() == 2
        && index_key[0] == solutions_key[0]
        && is_h256(&types, &index_key[1]);
    if index != expected || !key_ok {
        return Err("incorrect required OrderAnswers index entry".into());
    }
    entries.remove(solutions_at + 1);
    Ok(())
}

pub fn check(before: &Value, after: &Value) -> Result<(), String> {
    // A stale outer clone must not satisfy the graph comparison after only
    // the pallet target was normalized (codex_rigorous_reviewer's P2 finding).
    for pallet in [MEMPOOL, "Xqvm", "Staking"] {
        check_pallet_references(before, pallet).map_err(|error| format!("baseline: {error}"))?;
        check_pallet_references(after, pallet).map_err(|error| format!("candidate: {error}"))?;
    }
    let mut left = before.clone();
    let mut right = after.clone();
    for delta in APPROVED_CONSTANT_DELTAS {
        if *delta.value(&mut left)? != serde_json::json!(delta.baseline) {
            return Err(format!("unexpected baseline constant: {}", delta.path()));
        }
        if *delta.value(&mut right)? != serde_json::json!(delta.candidate) {
            return Err(format!(
                "missing or incorrect approved constant delta: {}",
                delta.path()
            ));
        }
    }
    // Before normalize_mempool: restoring an Xqvm value would otherwise surface
    // as a mempool mismatch, because the mempool values are compared to Xqvm's.
    for &(pallet, constant) in REGENERATED_WEIGHT_VALUES {
        if constant_value(&mut left, pallet, constant)?
            == constant_value(&mut right, pallet, constant)?
        {
            return Err(format!(
                "missing required regenerated-weight delta: {pallet}.{constant}.value"
            ));
        }
    }
    // First: W3-001 then W3-006 append after every other mempool delta,
    // newest first.
    normalize_duplicate_answer_docs(&mut right)?;
    normalize_eviction(&mut left, &mut right)?;
    normalize_solution_not_improved_docs(&mut left, &mut right)?;
    normalize_reveal(&mut left, &mut right)?;
    normalize_commitments(&mut left, &mut right)?;
    normalize_ordering_key(&mut left, &mut right)?;
    normalize_min_block_wait(&mut left, &mut right)?;
    normalize_deadline_too_short(&mut left, &mut right)?;
    normalize_order_deposit(&mut left, &mut right)?;
    normalize_audit_retention(&mut left, &mut right)?;
    normalize_audit_expiry_storage(&mut left, &mut right)?;
    // Before normalize_reward_escrow, which expects RewardNotSpendable last.
    normalize_answer_index(&mut left, &mut right)?;
    normalize_reward_escrow(&mut left, &mut right)?;
    normalize_mempool(&mut left, &mut right)?;
    normalize_commission_error(&mut left, &mut right)?;
    normalize_xqvm(&mut left, &mut right)?;
    normalize_miner_registry(&mut left, &mut right)?;
    normalize_topology_summary(&mut left, &mut right)?;
    normalize_block_submitters(&mut left, &mut right)?;
    let before_types = left["types"]["types"]
        .as_array()
        .ok_or("missing baseline registry")?
        .clone();
    let after_types = right["types"]["types"]
        .as_array()
        .ok_or("missing candidate registry")?
        .clone();
    for registry in [&before_types, &after_types] {
        for (index, ty) in registry.iter().enumerate() {
            if ty["id"].as_u64() != Some(index as u64) {
                return Err("noncanonical registry indexing".into());
            }
        }
    }
    let outer = left["outer_enums"]
        .as_object()
        .ok_or("missing outer enums")?
        .values()
        .map(|x| x.as_u64().ok_or("invalid outer type id"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let pallets = right["pallets"].as_array_mut().ok_or("missing pallets")?;
    let additions: Vec<_> = pallets.iter().filter(|p| p["name"] == "Vesting").collect();
    if additions.len() != 1 || additions[0]["index"] != 32 {
        return Err("Vesting must appear exactly once at index 32".into());
    }
    let call_type = additions[0]["calls"]["ty"]
        .as_u64()
        .ok_or("missing Vesting calls")?;
    let call_type =
        usize::try_from(call_type).map_err(|_| "Vesting call type does not fit usize")?;
    let calls = after_types
        .get(call_type)
        .ok_or("Vesting call type is outside candidate registry")?["type"]["def"]["variant"]
        ["variants"]
        .as_array()
        .ok_or("missing Vesting call enum")?;
    let expected = [
        "vest",
        "vest_other",
        "vested_transfer",
        "force_vested_transfer",
        "merge_schedules",
        "force_remove_vesting_schedule",
    ];
    if calls.len() != expected.len()
        || calls
            .iter()
            .zip(expected)
            .enumerate()
            .any(|(i, (call, name))| {
                call["name"] != name || call["index"].as_u64() != Some(i as u64)
            })
    {
        return Err("unexpected Vesting call indices".into());
    }
    pallets.retain(|p| p["name"] != "Vesting");
    for &(pallet, constant) in REGENERATED_WEIGHT_VALUES {
        let baseline = constant_value(&mut left, pallet, constant)?;
        let candidate = constant_value(&mut right, pallet, constant)?;
        // Normalize ONLY this named encoded value. The rest of the constant,
        // pallet and reachable type graph still undergo the strict comparison.
        *candidate = baseline.clone();
    }
    for delta in APPROVED_CONSTANT_DELTAS {
        *delta.value(&mut right)? = delta.value(&mut left)?.clone();
    }
    left.as_object_mut().unwrap().remove("types");
    right.as_object_mut().unwrap().remove("types");
    let mut comparison = Comparison {
        before: before_types,
        after: after_types,
        outer,
        seen: BTreeSet::new(),
    };
    comparison.value(&left, &right, "metadata")?;
    let visited: BTreeSet<_> = comparison.seen.iter().map(|(id, _)| *id).collect();
    if visited.len() != comparison.before.len() {
        return Err(format!(
            "baseline registry not fully checked: {} of {}",
            visited.len(),
            comparison.before.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_pair() -> (Value, Value) {
        (
            serde_json::to_value(super::super::before_vesting_metadata()).unwrap(),
            serde_json::to_value(super::super::metadata_v16()).unwrap(),
        )
    }

    #[test]
    fn requires_exact_xqvm_documentation_on_both_sides() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for name in ["store_program", "execute", "MaxLoopDepth"] {
            // Changed, missing, extra and restored old text must all fail.
            for candidate in [true, false] {
                for mutation in 0..4 {
                    let mut old = before.clone();
                    let mut new = after.clone();
                    let docs = xqvm_docs(if candidate { &mut new } else { &mut old }, name)
                        .unwrap()
                        .as_array_mut()
                        .unwrap();
                    match mutation {
                        0 => docs[0] = Value::from("unreviewed documentation"),
                        1 => {
                            docs.pop();
                        }
                        2 => docs.push(Value::from("unreviewed addition")),
                        _ => docs.clear(),
                    }
                    assert!(check(&old, &new).is_err(), "accepted {name} docs mutation");
                }
            }
            let mut old = before.clone();
            let mut restored = after.clone();
            *xqvm_docs(&mut restored, name).unwrap() = xqvm_docs(&mut old, name).unwrap().clone();
            assert!(
                check(&before, &restored).is_err(),
                "accepted missing {name} delta"
            );
        }
        let mut changed_call = after.clone();
        xqvm_variants(&mut changed_call, "calls").unwrap()[0]["index"] = Value::from(99);
        assert!(check(&before, &changed_call).is_err());
        let mut changed_constant = after.clone();
        *constant_value(&mut changed_constant, "Xqvm", "MaxLoopDepth").unwrap() =
            serde_json::json!([31, 0, 0, 0]);
        assert!(check(&before, &changed_constant).is_err());
    }

    #[test]
    fn requires_exact_appended_xqvm_error_and_preserves_existing_errors() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for field in ["name", "index", "docs", "fields", "deprecation_info"] {
            let mut wrong = after.clone();
            xqvm_variants(&mut wrong, "error").unwrap()[37][field] = Value::from("mutated");
            assert!(
                check(&before, &wrong).is_err(),
                "accepted budget error {field}"
            );
        }
        for mutation in 0..4 {
            let mut wrong = after.clone();
            let errors = xqvm_variants(&mut wrong, "error").unwrap();
            match mutation {
                0 => {
                    errors.pop();
                }
                1 => errors.push(serde_json::json!({"name":"extra", "index":38})),
                2 => errors.swap(36, 37),
                _ => errors[0]["docs"] = serde_json::json!(["unreviewed existing error"]),
            }
            assert!(
                check(&before, &wrong).is_err(),
                "accepted Xqvm error mutation"
            );
        }
        let mut wrong_baseline = before.clone();
        let appended = xqvm_variants(&mut after.clone(), "error").unwrap()[37].clone();
        xqvm_variants(&mut wrong_baseline, "error")
            .unwrap()
            .push(appended);
        assert!(check(&wrong_baseline, &after).is_err());
    }

    // Clone the pallet target at the next canonical id and redirect only the
    // outer mempool field. No hardcoded registry positions are involved.
    fn split_outer_reference(metadata: &mut Value, kind: &str, outer_key: &str) -> usize {
        let pallet_id = metadata["pallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "QuantumComputeMempool")
            .unwrap()[kind]["ty"]
            .as_u64()
            .unwrap() as usize;
        let outer_id = metadata["outer_enums"][outer_key].as_u64().unwrap() as usize;
        let registry = metadata["types"]["types"].as_array_mut().unwrap();
        let clone_id = registry.len();
        let mut cloned = registry[pallet_id].clone();
        cloned["id"] = serde_json::json!(clone_id);
        registry.push(cloned);
        let variant = registry[outer_id]["type"]["def"]["variant"]["variants"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "QuantumComputeMempool")
            .unwrap();
        variant["fields"][0]["type"] = serde_json::json!(clone_id);
        clone_id
    }

    #[test]
    fn rejects_split_call_and_error_references_in_both_graphs() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for (kind, outer, label) in [
            ("calls", "call_enum_ty", "RuntimeCall"),
            ("error", "error_enum_ty", "RuntimeError"),
        ] {
            let message = format!("{label}::QuantumComputeMempool must reference the pallet QuantumComputeMempool.{kind} type");
            let mut wrong = after.clone();
            let id = split_outer_reference(&mut wrong, kind, outer);
            if kind == "error" {
                wrong["types"]["types"][id]["type"]["def"]["variant"]["variants"]
                    .as_array_mut()
                    .unwrap()
                    .truncate(36);
            } else {
                // Stale outer call clone omits only the newly required docs.
                let docs = wrong["types"]["types"][id]["type"]["def"]["variant"]["variants"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|call| call["name"] == "register_job_spec")
                    .unwrap()["docs"]
                    .as_array_mut()
                    .unwrap();
                docs.truncate(docs.len() - REGISTER_PROGRAM_DOCS.len());
            }
            assert_eq!(check(&before, &wrong), Err(format!("candidate: {message}")));
            let mut wrong = before.clone();
            split_outer_reference(&mut wrong, kind, outer);
            assert_eq!(check(&wrong, &after), Err(format!("baseline: {message}")));
        }
    }

    #[test]
    fn requires_exact_registration_liveness_docs() {
        let (mut before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let baseline = register_program_docs(&mut before).unwrap().clone();
        let message = "unexpected required register_job_spec M1 documentation delta";
        let mut wrong = after.clone();
        *register_program_docs(&mut wrong).unwrap() = baseline;
        assert_eq!(check(&before, &wrong), Err(message.into()));
        let mut wrong = after.clone();
        register_program_docs(&mut wrong)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(check(&before, &wrong), Err(message.into()));
        let mut wrong = after.clone();
        register_program_docs(&mut wrong)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(Value::from("unapproved addition"));
        assert_eq!(check(&before, &wrong), Err(message.into()));
        let mut wrong = after.clone();
        register_program_docs(&mut wrong).unwrap()[0] = Value::from("changed existing docs");
        assert_eq!(check(&before, &wrong), Err(message.into()));
        *register_program_docs(&mut before).unwrap() =
            register_program_docs(&mut after.clone()).unwrap().clone();
        assert_eq!(
            check(&before, &after),
            Err("unexpected baseline register_job_spec M1 docs".into())
        );
    }

    #[test]
    fn requires_exact_errors_and_constants_on_both_sides() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for index in 36..46 {
            for field in ["name", "index", "docs", "fields"] {
                let mut wrong = after.clone();
                error_variants(&mut wrong).unwrap()[index][field] = serde_json::json!("mutated");
                assert!(
                    check(&before, &wrong).is_err(),
                    "accepted error {index}.{field}"
                );
            }
            let mut wrong = after.clone();
            error_variants(&mut wrong).unwrap().remove(index);
            assert!(check(&before, &wrong).is_err());
        }
        let mut wrong = after.clone();
        error_variants(&mut wrong)
            .unwrap()
            .push(serde_json::json!({"name": "extra", "index": 45}));
        assert!(check(&before, &wrong).is_err());
        let mut wrong = before.clone();
        error_variants(&mut wrong)
            .unwrap()
            .push(error_variants(&mut after.clone()).unwrap()[36].clone());
        assert!(check(&wrong, &after).is_err());
        let mut wrong = before.clone();
        let added = after["pallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == MEMPOOL)
            .unwrap()["constants"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        wrong["pallets"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["name"] == MEMPOOL)
            .unwrap()["constants"]
            .as_array_mut()
            .unwrap()
            .push(added);
        assert!(check(&wrong, &after).is_err());
        for index in 0..3 {
            for field in ["name", "ty", "value", "docs", "deprecation_info"] {
                let mut wrong = after.clone();
                let constants = &mut wrong["pallets"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|p| p["name"] == MEMPOOL)
                    .unwrap()["constants"];
                let constants = constants.as_array_mut().unwrap();
                let len = constants.len();
                constants[len - 3 + index][field] = serde_json::json!("mutated");
                assert!(
                    check(&before, &wrong).is_err(),
                    "accepted constant {index}.{field}"
                );
            }
            let mut wrong = after.clone();
            let constants = wrong["pallets"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|p| p["name"] == MEMPOOL)
                .unwrap()["constants"]
                .as_array_mut()
                .unwrap();
            constants.remove(constants.len() - 3 + index);
            assert!(check(&before, &wrong).is_err());
        }
    }

    fn reverse_registry(mut metadata: Value) -> Value {
        // Rewrite every portable type reference, including tuples and registry
        // type definitions, then restore canonical id == array-index ordering.
        fn references(value: &mut Value, count: u64) {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        let type_reference = matches!(
                            key.as_str(),
                            "type"
                                | "ty"
                                | "address_ty"
                                | "call_ty"
                                | "signature_ty"
                                | "implicit"
                                | "output"
                                | "call_enum_ty"
                                | "event_enum_ty"
                                | "error_enum_ty"
                                | "Plain"
                                | "key"
                                | "value"
                                | "bit_store_type"
                                | "bit_order_type"
                        );
                        if type_reference && value.is_u64() {
                            *value = serde_json::json!(count - 1 - value.as_u64().unwrap());
                        } else if key == "tuple" {
                            for id in value.as_array_mut().unwrap() {
                                *id = serde_json::json!(count - 1 - id.as_u64().unwrap());
                            }
                        } else {
                            references(value, count);
                        }
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        references(value, count);
                    }
                }
                _ => {}
            }
        }
        let count = metadata["types"]["types"].as_array().unwrap().len();
        references(&mut metadata, count as u64);
        let registry = metadata["types"]["types"].as_array_mut().unwrap();
        registry.reverse();
        for (id, ty) in registry.iter_mut().enumerate() {
            ty["id"] = serde_json::json!(id);
        }
        metadata
    }

    #[test]
    fn accepts_full_canonical_registry_renumbering() {
        let (before, after) = metadata_pair();
        let renumbered_before = reverse_registry(before.clone());
        let renumbered_after = reverse_registry(after.clone());
        assert_ne!(before["outer_enums"], renumbered_before["outer_enums"]);
        assert_ne!(after["outer_enums"], renumbered_after["outer_enums"]);
        check(&before, &renumbered_after).unwrap();
        check(&renumbered_before, &after).unwrap();
        check(&renumbered_before, &renumbered_after).unwrap();
    }

    #[test]
    fn requires_exact_expiry_anchor_and_cursor_delta() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for field in ["name", "modifier", "type", "default", "docs"] {
            let mut wrong = after.clone();
            mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|entry| entry["name"] == "OldestExpiryBucket")
                .unwrap()[field] = Value::from("mutated");
            assert!(check(&before, &wrong).is_err(), "accepted anchor {field}");
        }
        let mut missing = after.clone();
        mempool_storage_entries(&mut missing)
            .unwrap()
            .retain(|entry| entry["name"] != "OldestExpiryBucket");
        assert!(check(&before, &missing).is_err());
        let mut wrong_before = before.clone();
        mempool_storage_entries(&mut wrong_before)
            .unwrap()
            .iter_mut()
            .find(|entry| entry["name"] == "NextExpiryBucket")
            .unwrap()["docs"] = Value::from("mutated");
        assert!(check(&wrong_before, &after).is_err());
    }

    #[test]
    fn requires_exact_reward_escrow_delta() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(&str, Value)> = Vec::new();
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "error")
            .unwrap()
            .retain(|v| v["name"] != "RewardNotSpendable");
        cases.push(("missing RewardNotSpendable", wrong));
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "error")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "RewardNotSpendable")
            .unwrap()["docs"] = Value::from("mutated");
        cases.push(("altered RewardNotSpendable", wrong));
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "error")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "RewardReserveMissing")
            .unwrap()["docs"] = serde_json::json!(BASELINE_RESERVE_MISSING_DOCS);
        cases.push(("stale RewardReserveMissing docs", wrong));
        for index in [0usize, 1] {
            let mut wrong = after.clone();
            let events = mempool_variants(&mut wrong, "event").unwrap();
            let at = events.len() - 2 + index;
            events[at]["fields"][1]["type"] = Value::from(0);
            cases.push(("altered escrow event field", wrong));
        }
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "event").unwrap().pop();
        cases.push(("missing EscrowReleased", wrong));
        let mut wrong = after.clone();
        wrong["pallets"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["name"] == MEMPOOL)
            .unwrap()["constants"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["value"] = serde_json::json!(b"qp/other".to_vec());
        cases.push(("altered EscrowPalletId", wrong));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_answer_index_delta() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "error").unwrap().pop();
        cases.push(("missing DuplicateAnswer".into(), wrong));
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "error")
            .unwrap()
            .last_mut()
            .unwrap()["docs"] = Value::from("mutated");
        cases.push(("altered DuplicateAnswer".into(), wrong));
        let u64_id = mempool_storage_entries(&mut after.clone())
            .unwrap()
            .iter()
            .find(|e| e["name"] == "NextOrderId")
            .unwrap()["ty"]["Plain"]
            .clone();
        for name in ["JobSolution", "RankedSolver"] {
            let path = serde_json::json!(["pallet_quantum_compute_mempool", "types", name]);
            for (what, mutate) in [
                ("missing", 0),
                ("renamed", 1),
                ("retyped", 2),
                ("type name", 3),
            ] {
                let mut wrong = after.clone();
                let ty = wrong["types"]["types"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|ty| ty["type"]["path"] == path)
                    .unwrap();
                let fields = ty
                    .pointer_mut("/type/def/composite/fields")
                    .and_then(Value::as_array_mut)
                    .unwrap();
                match mutate {
                    0 => {
                        fields.pop();
                    }
                    1 => fields.last_mut().unwrap()["name"] = Value::from("other"),
                    2 => fields.last_mut().unwrap()["type"] = u64_id.clone(),
                    _ => fields.last_mut().unwrap()["typeName"] = Value::from("[u8; 32]"),
                }
                cases.push((format!("{what} {name}.answer"), wrong));
            }
        }
        for field in [
            "name",
            "modifier",
            "default",
            "docs",
            "deprecation_info",
            "ty",
        ] {
            let mut wrong = after.clone();
            mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "OrderAnswers")
                .unwrap()[field] = Value::from("mutated");
            cases.push((format!("altered OrderAnswers.{field}"), wrong));
        }
        let solutions = mempool_storage_entries(&mut after.clone())
            .unwrap()
            .iter()
            .find(|e| e["name"] == "OrderSolutions")
            .unwrap()
            .clone();
        for (what, pointer, value) in [
            ("key", "/ty/Map/key", solutions["ty"]["Map"]["key"].clone()),
            ("value", "/ty/Map/value", u64_id.clone()),
            ("hasher", "/ty/Map/hashers/1", Value::from("Twox64Concat")),
        ] {
            let mut wrong = after.clone();
            *mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "OrderAnswers")
                .unwrap()
                .pointer_mut(pointer)
                .unwrap() = value;
            cases.push((format!("altered OrderAnswers {what}"), wrong));
        }
        let mut wrong = after.clone();
        mempool_storage_entries(&mut wrong)
            .unwrap()
            .retain(|e| e["name"] != "OrderAnswers");
        cases.push(("missing OrderAnswers".into(), wrong));
        let mut wrong = after.clone();
        let entries = mempool_storage_entries(&mut wrong).unwrap();
        let at = entries
            .iter()
            .position(|e| e["name"] == "OrderAnswers")
            .unwrap();
        let entry = entries.remove(at);
        entries.push(entry);
        cases.push(("misplaced OrderAnswers".into(), wrong));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_quantum_pow_topology_summary() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for field in [
            "name",
            "modifier",
            "default",
            "docs",
            "deprecation_info",
            "ty",
        ] {
            let mut wrong = after.clone();
            pow_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "TopologySummary")
                .unwrap()[field] = Value::from("mutated");
            assert!(
                check(&before, &wrong).is_err(),
                "accepted TopologySummary.{field}"
            );
        }
        let mut missing = after.clone();
        pow_storage_entries(&mut missing)
            .unwrap()
            .retain(|e| e["name"] != "TopologySummary");
        assert!(
            check(&before, &missing).is_err(),
            "accepted a missing summary"
        );
        let mut misplaced = after.clone();
        let entries = pow_storage_entries(&mut misplaced).unwrap();
        let at = entries
            .iter()
            .position(|e| e["name"] == "TopologySummary")
            .unwrap();
        let entry = entries.remove(at);
        entries.push(entry);
        assert!(
            check(&before, &misplaced).is_err(),
            "accepted a misplaced summary"
        );
        // A different value type: rename the struct's path.
        let mut wrong_type = after.clone();
        let value = pow_storage_entries(&mut wrong_type)
            .unwrap()
            .iter()
            .find(|e| e["name"] == "TopologySummary")
            .unwrap()["ty"]["Map"]["value"]
            .as_u64()
            .unwrap() as usize;
        wrong_type["types"]["types"][value]["type"]["path"] =
            serde_json::json!(["pallet_quantum_pow", "types", "Other"]);
        assert!(
            check(&before, &wrong_type).is_err(),
            "accepted a different value type"
        );
    }

    /// Every registry type at `path` in `metadata`.
    fn types_at<'a>(metadata: &'a mut Value, name: &str) -> Vec<&'a mut Value> {
        let path = mempool_type_path(name);
        metadata["types"]["types"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter(|ty| ty["type"]["path"] == path)
            .collect()
    }

    #[test]
    fn requires_exact_mempool_duplicate_answer_docs() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let set_docs = |docs: Value| -> Value {
            let mut wrong = after.clone();
            mempool_variants(&mut wrong, "error")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "DuplicateAnswer")
                .unwrap()["docs"] = docs;
            wrong
        };
        for (name, wrong) in [
            (
                "W1-012 docs",
                set_docs(serde_json::json!(DUPLICATE_ANSWER_DOCS_W1_012)),
            ),
            ("edited docs", set_docs(serde_json::json!(["mutated"]))),
            (
                "truncated docs",
                set_docs(serde_json::json!(&DUPLICATE_ANSWER_DOCS_W3_001[..3])),
            ),
        ] {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
        let mut renamed = after.clone();
        mempool_variants(&mut renamed, "error")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "DuplicateAnswer")
            .unwrap()["name"] = Value::from("AnswerTaken");
        assert!(
            check(&before, &renamed).is_err(),
            "accepted a renamed error"
        );
    }

    #[test]
    fn requires_exact_mempool_solution_evicted_event() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        let edit = |metadata: &Value, f: &dyn Fn(&mut Value)| -> Value {
            let mut wrong = metadata.clone();
            f(mempool_variants(&mut wrong, "event")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "SolutionEvicted")
                .unwrap());
            wrong
        };
        for field in ["docs", "index", "name"] {
            cases.push((
                format!("SolutionEvicted.{field}"),
                edit(&after, &|v| v[field] = Value::from(999)),
            ));
        }
        for index in 0..4 {
            cases.push((
                format!("SolutionEvicted field {index} typeName"),
                edit(&after, &|v| {
                    v["fields"][index]["typeName"] = Value::from("mutated")
                }),
            ));
            cases.push((
                format!("SolutionEvicted field {index} name"),
                edit(&after, &|v| {
                    v["fields"][index]["name"] = Value::from("mutated")
                }),
            ));
        }
        // The answer retyped to the account type, with its typeName kept.
        let mut retyped = after.clone();
        let events = mempool_variants(&mut retyped, "event").unwrap();
        let account_ty = events
            .iter()
            .find(|v| v["name"] == "SolutionEvicted")
            .unwrap()["fields"][1]["type"]
            .clone();
        events
            .iter_mut()
            .find(|v| v["name"] == "SolutionEvicted")
            .unwrap()["fields"][3]["type"] = account_ty;
        cases.push(("SolutionEvicted answer retyped".into(), retyped));
        cases.push((
            "SolutionEvicted without answer".into(),
            edit(&after, &|v| {
                v["fields"].as_array_mut().unwrap().pop();
            }),
        ));
        let mut misplaced = after.clone();
        let events = mempool_variants(&mut misplaced, "event").unwrap();
        let len = events.len();
        events.swap(len - 1, len - 2);
        cases.push(("misplaced SolutionEvicted".into(), misplaced));
        let mut missing = after.clone();
        mempool_variants(&mut missing, "event").unwrap().pop();
        cases.push(("missing SolutionEvicted".into(), missing));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_solution_not_improved_docs() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let set_docs = |metadata: &Value, docs: Value| -> Value {
            let mut wrong = metadata.clone();
            mempool_variants(&mut wrong, "error")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "SolutionNotImproved")
                .unwrap()["docs"] = docs;
            wrong
        };
        for (name, wrong) in [
            (
                "unchanged docs",
                set_docs(&after, serde_json::json!(SOLUTION_NOT_IMPROVED_DOCS_BEFORE)),
            ),
            (
                "edited docs",
                set_docs(&after, serde_json::json!(["mutated"])),
            ),
            (
                "truncated docs",
                set_docs(
                    &after,
                    serde_json::json!(&SOLUTION_NOT_IMPROVED_DOCS_AFTER[..2]),
                ),
            ),
        ] {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
        let mut renamed = after.clone();
        mempool_variants(&mut renamed, "error")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "SolutionNotImproved")
            .unwrap()["name"] = Value::from("SolutionNotBetter");
        assert!(
            check(&before, &renamed).is_err(),
            "accepted a renamed error"
        );
    }

    #[test]
    fn requires_exact_mempool_reveal() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        let call = |metadata: &mut Value| -> Value {
            mempool_variants(metadata, "calls")
                .unwrap()
                .iter()
                .find(|v| v["name"] == "reveal_solution")
                .cloned()
                .unwrap()
        };
        for field in ["docs", "index", "name"] {
            let mut wrong = after.clone();
            mempool_variants(&mut wrong, "calls")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "reveal_solution")
                .unwrap()[field] = Value::from(999);
            cases.push((format!("reveal_solution.{field}"), wrong));
        }
        let fields = call(&mut after.clone())["fields"].as_array().unwrap().len();
        for index in 0..fields {
            let mut wrong = after.clone();
            mempool_variants(&mut wrong, "calls")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "reveal_solution")
                .unwrap()["fields"][index]["typeName"] = Value::from("mutated");
            cases.push((format!("reveal_solution field {index}"), wrong));
        }
        let mut dropped = after.clone();
        mempool_variants(&mut dropped, "calls")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "reveal_solution")
            .unwrap()["fields"]
            .as_array_mut()
            .unwrap()
            .pop();
        cases.push(("reveal_solution without salt".into(), dropped));
        let mut misplaced = after.clone();
        let calls = mempool_variants(&mut misplaced, "calls").unwrap();
        let len = calls.len();
        calls.swap(len - 1, len - 2);
        cases.push(("misplaced reveal_solution".into(), misplaced));
        for name in ["CommitmentNotFound", "RevealTooEarly", "CommitmentMismatch"] {
            for field in ["docs", "index", "name"] {
                let mut wrong = after.clone();
                mempool_variants(&mut wrong, "error")
                    .unwrap()
                    .iter_mut()
                    .find(|v| v["name"] == name)
                    .unwrap()[field] = Value::from(999);
                cases.push((format!("{name}.{field}"), wrong));
            }
        }
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_commitments() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        let variant = |metadata: &mut Value, kind: &str, name: &str, field: &str, value: Value| {
            mempool_variants(metadata, kind)
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == name)
                .unwrap()[field] = value;
        };
        // The call.
        for field in ["docs", "index"] {
            let mut wrong = after.clone();
            variant(
                &mut wrong,
                "calls",
                "commit_answer",
                field,
                Value::from(999),
            );
            cases.push((format!("commit_answer.{field}"), wrong));
        }
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "calls")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "commit_answer")
            .unwrap()["fields"][1]["typeName"] = Value::from("H256");
        cases.push(("commit_answer field".into(), wrong));
        // The constant.
        for (field, value) in [
            ("value", serde_json::json!([63, 0, 0, 0])),
            ("docs", Value::from("mutated")),
            ("ty", Value::from(0)),
        ] {
            let mut wrong = after.clone();
            mempool_constants(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|c| c["name"] == "MaxCommitmentsPerOrder")
                .unwrap()[field] = value;
            cases.push((format!("MaxCommitmentsPerOrder.{field}"), wrong));
        }
        // The storage.
        for (name, field) in [
            ("AnswerCommitments", "docs"),
            ("AnswerCommitments", "modifier"),
            ("AnswerCommitments", "ty"),
            ("CommitmentCount", "docs"),
            ("CommitmentCount", "modifier"),
            ("CommitmentCount", "default"),
        ] {
            let mut wrong = after.clone();
            mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == name)
                .unwrap()[field] = Value::from("mutated");
            cases.push((format!("{name}.{field}"), wrong));
        }
        let mut wrong = after.clone();
        for ty in types_at(&mut wrong, "AnswerCommitment") {
            ty["type"]["def"]["composite"]["fields"][2]["name"] = Value::from("amount");
        }
        cases.push(("AnswerCommitment.deposit renamed".into(), wrong));
        let mut misplaced = after.clone();
        let entries = mempool_storage_entries(&mut misplaced).unwrap();
        let len = entries.len();
        entries.swap(len - 1, len - 3);
        cases.push(("misplaced commitment storage".into(), misplaced));
        // The errors and events.
        for (kind, name) in [
            ("error", "CommitmentCapReached"),
            ("error", "CommitmentDepositNotAvailable"),
            ("event", "AnswerCommitted"),
            ("event", "CommitmentForfeited"),
        ] {
            for field in ["docs", "index", "name"] {
                let mut wrong = after.clone();
                variant(&mut wrong, kind, name, field, Value::from(999));
                cases.push((format!("{name}.{field}"), wrong));
            }
        }
        let mut wrong = after.clone();
        mempool_variants(&mut wrong, "event")
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "CommitmentForfeited")
            .unwrap()["fields"]
            .as_array_mut()
            .unwrap()
            .pop();
        cases.push(("CommitmentForfeited without burned".into(), wrong));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_ordering_key() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        // NextSubmissionSeq.
        for field in [
            "name",
            "modifier",
            "default",
            "docs",
            "deprecation_info",
            "ty",
        ] {
            let mut wrong = after.clone();
            mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "NextSubmissionSeq")
                .unwrap()[field] = Value::from("mutated");
            cases.push((format!("NextSubmissionSeq.{field}"), wrong));
        }
        let mut missing = after.clone();
        mempool_storage_entries(&mut missing)
            .unwrap()
            .retain(|e| e["name"] != "NextSubmissionSeq");
        cases.push(("missing NextSubmissionSeq".into(), missing));
        let mut misplaced = after.clone();
        let entries = mempool_storage_entries(&mut misplaced).unwrap();
        let len = entries.len();
        entries.swap(len - 1, len - 2);
        cases.push(("misplaced NextSubmissionSeq".into(), misplaced));
        // OrderAnswers: the value and the docs.
        let mut docs = after.clone();
        mempool_storage_entries(&mut docs)
            .unwrap()
            .iter_mut()
            .find(|e| e["name"] == "OrderAnswers")
            .unwrap()["docs"] = Value::from("mutated");
        cases.push(("OrderAnswers.docs".into(), docs));
        for (index, field) in ["name", "type", "typeName"].into_iter().enumerate() {
            let mut wrong = after.clone();
            for ty in types_at(&mut wrong, "AnswerOwner") {
                ty["type"]["def"]["composite"]["fields"][index % 4][field] = Value::from(999);
            }
            cases.push((format!("AnswerOwner field {index}.{field}"), wrong));
        }
        let mut dropped = after.clone();
        for ty in types_at(&mut dropped, "AnswerOwner") {
            ty["type"]["def"]["composite"]["fields"]
                .as_array_mut()
                .unwrap()
                .pop();
        }
        cases.push(("AnswerOwner without seq".into(), dropped));
        // The appended fields and the new parameter.
        for name in ["JobSolution", "RankedSolver", "FrontRunner"] {
            for (label, mutate) in [
                ("seq dropped", 0usize),
                ("effective_at renamed", 1),
                ("seq retyped", 2),
            ] {
                let mut wrong = after.clone();
                for ty in types_at(&mut wrong, name) {
                    let fields = ty["type"]["def"]["composite"]["fields"]
                        .as_array_mut()
                        .unwrap();
                    let n = fields.len();
                    match mutate {
                        0 => {
                            fields.pop();
                        }
                        1 => fields[n - 2]["name"] = Value::from("effective"),
                        _ => fields[n - 1]["typeName"] = Value::from("u64"),
                    }
                }
                cases.push((format!("{name}: {label}"), wrong));
            }
        }
        for name in ["RankedSolver", "FrontRunner"] {
            let mut wrong = after.clone();
            for ty in types_at(&mut wrong, name) {
                ty["type"]["params"].as_array_mut().unwrap().pop();
            }
            cases.push((format!("{name} without the BlockNumber parameter"), wrong));
        }
        // M3: a `BlockNumber` parameter that is not `JobSolution`'s `u32`, e.g.
        // `AnswerOwner<AccountId, u64>`.
        let u64_id = after["types"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ty| ty["type"]["def"]["primitive"] == "u64")
            .map(|ty| ty["id"].clone())
            .expect("a u64 in the registry");
        // The parameter and its `effective_at` field retyped together, as a
        // real `AnswerOwner<AccountId, u64>` would be: only the M3 pin to
        // `JobSolution`'s `BlockNumber` catches it.
        for name in ["AnswerOwner", "RankedSolver", "FrontRunner"] {
            let mut wrong = after.clone();
            for ty in types_at(&mut wrong, name) {
                let params = ty["type"]["params"].as_array_mut().unwrap();
                let last = params.len() - 1;
                params[last]["type"] = u64_id.clone();
                for field in ty["type"]["def"]["composite"]["fields"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .filter(|f| f["name"] == "effective_at")
                {
                    field["type"] = u64_id.clone();
                }
            }
            cases.push((format!("{name}<_, u64>"), wrong));
        }
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_min_block_wait() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        for field in ["name", "ty", "value", "docs", "deprecation_info"] {
            let mut wrong = after.clone();
            mempool_constants(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|c| c["name"] == "MinBlockWait")
                .unwrap()[field] = Value::from("mutated");
            cases.push((format!("MinBlockWait.{field}"), wrong));
        }
        let mut one = after.clone();
        mempool_constants(&mut one)
            .unwrap()
            .iter_mut()
            .find(|c| c["name"] == "MinBlockWait")
            .unwrap()["value"] = serde_json::json!([1, 0, 0, 0]);
        cases.push(("MinBlockWait = 1".into(), one));
        for field in ["docs", "name", "index"] {
            let mut wrong = after.clone();
            mempool_variants(&mut wrong, "error")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "BlockWaitTooShort")
                .unwrap()[field] = Value::from(999);
            cases.push((format!("BlockWaitTooShort.{field}"), wrong));
        }
        let mut misplaced = after.clone();
        let constants = mempool_constants(&mut misplaced).unwrap();
        let len = constants.len();
        constants.swap(len - 1, len - 2);
        cases.push(("misplaced MinBlockWait".into(), misplaced));
        let mut misplaced = after.clone();
        let errors = mempool_variants(&mut misplaced, "error").unwrap();
        let len = errors.len();
        errors.swap(len - 1, len - 2);
        cases.push(("misplaced BlockWaitTooShort".into(), misplaced));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_deadline_too_short() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        for field in ["docs", "name", "index"] {
            let mut wrong = after.clone();
            mempool_variants(&mut wrong, "error")
                .unwrap()
                .iter_mut()
                .find(|v| v["name"] == "DeadlineTooShort")
                .unwrap()[field] = Value::from(999);
            cases.push((format!("DeadlineTooShort.{field}"), wrong));
        }
        let mut missing = after.clone();
        mempool_variants(&mut missing, "error")
            .unwrap()
            .retain(|v| v["name"] != "DeadlineTooShort");
        cases.push(("missing DeadlineTooShort".into(), missing));
        let mut misplaced = after.clone();
        let errors = mempool_variants(&mut misplaced, "error").unwrap();
        let len = errors.len();
        errors.swap(len - 1, len - 2);
        cases.push(("misplaced DeadlineTooShort".into(), misplaced));
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_mempool_order_deposit() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut cases: Vec<(String, Value)> = Vec::new();
        for name in ["OrderDepositPerItem", "OrderDepositPerByte"] {
            for field in ["name", "ty", "value", "docs", "deprecation_info"] {
                let mut wrong = after.clone();
                mempool_constants(&mut wrong)
                    .unwrap()
                    .iter_mut()
                    .find(|c| c["name"] == name)
                    .unwrap()[field] = Value::from("mutated");
                cases.push((format!("{name}.{field}"), wrong));
            }
        }
        for field in [
            "name",
            "modifier",
            "default",
            "docs",
            "deprecation_info",
            "ty",
        ] {
            let mut wrong = after.clone();
            mempool_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "OrderDeposit")
                .unwrap()[field] = Value::from("mutated");
            cases.push((format!("OrderDeposit.{field}"), wrong));
        }
        let mut missing = after.clone();
        mempool_storage_entries(&mut missing)
            .unwrap()
            .retain(|e| e["name"] != "OrderDeposit");
        cases.push(("missing OrderDeposit".into(), missing));
        for (kind, name) in [
            ("error", "DepositNotAvailable"),
            ("event", "OrderDepositReleased"),
        ] {
            for field in ["docs", "name", "index"] {
                let mut wrong = after.clone();
                mempool_variants(&mut wrong, kind)
                    .unwrap()
                    .iter_mut()
                    .find(|v| v["name"] == name)
                    .unwrap()[field] = Value::from(999);
                cases.push((format!("{name}.{field}"), wrong));
            }
            // Out of position: swapped with its predecessor.
            let mut misplaced = after.clone();
            let variants = mempool_variants(&mut misplaced, kind).unwrap();
            let len = variants.len();
            variants.swap(len - 1, len - 2);
            cases.push((format!("misplaced {name}"), misplaced));
        }
        for (name, wrong) in cases {
            assert!(check(&before, &wrong).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn requires_exact_quantum_pow_block_submitters_and_error() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for field in [
            "name",
            "modifier",
            "default",
            "docs",
            "deprecation_info",
            "ty",
        ] {
            let mut wrong = after.clone();
            pow_storage_entries(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "BlockSubmitters")
                .unwrap()[field] = Value::from("mutated");
            assert!(
                check(&before, &wrong).is_err(),
                "accepted BlockSubmitters.{field}"
            );
        }
        let mut missing = after.clone();
        pow_storage_entries(&mut missing)
            .unwrap()
            .retain(|e| e["name"] != "BlockSubmitters");
        assert!(
            check(&before, &missing).is_err(),
            "accepted a missing BlockSubmitters"
        );
        let mut misplaced = after.clone();
        let entries = pow_storage_entries(&mut misplaced).unwrap();
        let at = entries
            .iter()
            .position(|e| e["name"] == "BlockSubmitters")
            .unwrap();
        let entry = entries.remove(at);
        entries.push(entry);
        assert!(
            check(&before, &misplaced).is_err(),
            "accepted a misplaced BlockSubmitters"
        );
        // A set of a different element type: point the innermost sequence at
        // another registry type.
        let mut wrong_element = after.clone();
        let mut id = pow_storage_entries(&mut wrong_element)
            .unwrap()
            .iter()
            .find(|e| e["name"] == "BlockSubmitters")
            .unwrap()["ty"]["Plain"]
            .as_u64()
            .unwrap() as usize;
        loop {
            let ty = &wrong_element["types"]["types"][id]["type"];
            if ty.pointer("/def/sequence/type").is_some() {
                break;
            }
            id = ty["def"]["composite"]["fields"][0]["type"]
                .as_u64()
                .unwrap() as usize;
        }
        let element = wrong_element["types"]["types"][id]["type"]["def"]["sequence"]["type"]
            .as_u64()
            .unwrap();
        wrong_element["types"]["types"][id]["type"]["def"]["sequence"]["type"] =
            Value::from(if element == 0 { 1 } else { 0 });
        assert!(
            check(&before, &wrong_element).is_err(),
            "accepted a BlockSubmitters of another element type"
        );
        // The appended error: its docs, its name, its index, or its absence.
        for (what, field, value) in [
            ("docs", "docs", Value::from("mutated")),
            ("name", "name", Value::from("Other")),
            ("index", "index", Value::from(999)),
        ] {
            let mut wrong = after.clone();
            pow_errors(&mut wrong).unwrap().last_mut().unwrap()[field] = value;
            assert!(
                check(&before, &wrong).is_err(),
                "accepted ProofAlreadySubmittedThisBlock with a different {what}"
            );
        }
        let mut no_error = after.clone();
        pow_errors(&mut no_error).unwrap().pop();
        assert!(
            check(&before, &no_error).is_err(),
            "accepted a missing ProofAlreadySubmittedThisBlock"
        );
    }

    #[test]
    fn requires_exact_miner_registry_retention_and_api_v2() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for name in ["ParticipationRetention", "MaxParticipationPrunePerBlock"] {
            for field in ["name", "ty", "value", "docs", "deprecation_info"] {
                let mut wrong = after.clone();
                miner_registry_section(&mut wrong, "constants")
                    .unwrap()
                    .iter_mut()
                    .find(|c| c["name"] == name)
                    .unwrap()[field] = Value::from("mutated");
                assert!(check(&before, &wrong).is_err(), "accepted {name}.{field}");
            }
            let mut missing = after.clone();
            miner_registry_section(&mut missing, "constants")
                .unwrap()
                .retain(|c| c["name"] != name);
            assert!(check(&before, &missing).is_err(), "accepted missing {name}");
        }
        for field in ["name", "modifier", "ty", "default", "docs"] {
            let mut wrong = after.clone();
            miner_registry_section(&mut wrong, "storage")
                .unwrap()
                .iter_mut()
                .find(|e| e["name"] == "PruneCursor")
                .unwrap()[field] = Value::from("mutated");
            assert!(
                check(&before, &wrong).is_err(),
                "accepted PruneCursor.{field}"
            );
        }
        let mut missing = after.clone();
        miner_registry_section(&mut missing, "storage")
            .unwrap()
            .retain(|e| e["name"] != "PruneCursor");
        assert!(
            check(&before, &missing).is_err(),
            "accepted missing PruneCursor"
        );

        let mut old_version = after.clone();
        participants_api(&mut old_version).unwrap()["version"] = Value::from(1);
        assert_eq!(
            check(&before, &old_version),
            Err("missing required MinerRegistryApi version 2".into())
        );
        for (side, metadata) in [("candidate", after.clone()), ("baseline", before.clone())] {
            let mut wrong = metadata;
            participants_api(&mut wrong).unwrap()["methods"][0]["docs"] = Value::from("mutated");
            let result = if side == "candidate" {
                check(&before, &wrong)
            } else {
                check(&wrong, &after)
            };
            assert!(result.is_err(), "accepted mutated {side} paging docs");
        }
        // System::Version must carry the API bump too, and nothing else.
        let mut stale = after.clone();
        let old = constant_value(&mut before.clone(), "System", "Version")
            .unwrap()
            .clone();
        *constant_value(&mut stale, "System", "Version").unwrap() = old;
        assert!(
            check(&before, &stale).is_err(),
            "accepted a stale System::Version"
        );
        // An unrelated MinerRegistry change is still caught by the graph.
        let mut unrelated = after.clone();
        miner_registry_section(&mut unrelated, "constants")
            .unwrap()
            .iter_mut()
            .find(|c| c["name"] == "MaxGpus")
            .unwrap()["value"] = serde_json::json!([17, 0, 0, 0]);
        assert!(
            check(&before, &unrelated).is_err(),
            "accepted an unrelated change"
        );
    }

    #[test]
    fn requires_exact_retention_storage_and_constant() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        for name in ["OrdersPrunableAt", "NextPruneBucket"] {
            for field in ["name", "modifier", "type", "default", "docs"] {
                let mut wrong = after.clone();
                mempool_storage_entries(&mut wrong)
                    .unwrap()
                    .iter_mut()
                    .find(|entry| entry["name"] == name)
                    .unwrap()[field] = Value::from("mutated");
                assert!(check(&before, &wrong).is_err(), "accepted {name}.{field}");
            }
            let mut missing = after.clone();
            mempool_storage_entries(&mut missing)
                .unwrap()
                .retain(|entry| entry["name"] != name);
            assert!(check(&before, &missing).is_err());
        }
        for field in ["name", "type", "value", "docs"] {
            let mut wrong = after.clone();
            mempool_constants(&mut wrong)
                .unwrap()
                .iter_mut()
                .find(|entry| entry["name"] == "OrderRetentionBlocks")
                .unwrap()[field] = Value::from("mutated");
            assert!(
                check(&before, &wrong).is_err(),
                "accepted retention {field}"
            );
        }
        let mut missing = after.clone();
        mempool_constants(&mut missing)
            .unwrap()
            .retain(|constant| constant["name"] != "OrderRetentionBlocks");
        assert!(check(&before, &missing).is_err());
    }
}
