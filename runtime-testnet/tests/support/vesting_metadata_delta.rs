//! Compare metadata graphs, not portable registry numbering. Approved deltas
//! are Vesting at index 32 (including the three outer enums) and the explicitly
//! enumerated regenerated-weight values, exact mempool VM delta, and retirement
//! of pallet index 7 below.
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
                "A referenced program fails static verification."
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
        "value": [
            0,
            0,
            0,
            1,
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

// Preserve the historical fixture while requiring removal of exactly index 7.
// Only its three pallet enum types may become unreachable; shared types still
// undergo the full graph comparison and every other pallet index stays fixed.
fn normalize_retired_pallet(before: &mut Value, after: &Value) -> Result<BTreeSet<u64>, String> {
    let pallets = before["pallets"]
        .as_array_mut()
        .ok_or("missing baseline pallets")?;
    let retired: Vec<_> = pallets
        .iter()
        .filter(|p| p["index"] == 7)
        .cloned()
        .collect();
    if retired.len() != 1 {
        return Err("baseline must contain exactly one pallet at retired index 7".into());
    }
    let retired = &retired[0];
    let candidate = after["pallets"]
        .as_array()
        .ok_or("missing candidate pallets")?;
    if candidate
        .iter()
        .any(|p| p["index"] == 7 || p["name"] == retired["name"])
    {
        return Err("retired pallet index 7 must be absent and never reused".into());
    }
    pallets.retain(|p| p["index"] != 7);
    let mut removed_types = BTreeSet::new();
    for (kind, outer) in [
        ("calls", "call_enum_ty"),
        ("event", "event_enum_ty"),
        ("error", "error_enum_ty"),
    ] {
        let id = retired[kind]["ty"]
            .as_u64()
            .ok_or("missing retired pallet enum")?;
        removed_types.insert(id);
        let type_index = usize::try_from(id).map_err(|_| "retired type id does not fit usize")?;
        let path = before["types"]["types"]
            .get(type_index)
            .and_then(|entry| entry["type"]["path"].as_array())
            .filter(|path| !path.is_empty())
            .ok_or("missing retired enum type identity")?;
        // Portable IDs (including nested references) may be renumbered. Compare
        // each retired definition's named identity by value instead: retaining
        // that identity is forbidden even if its fields or docs were changed.
        let candidate_types = after["types"]["types"]
            .as_array()
            .ok_or("missing candidate registry")?;
        if candidate_types
            .iter()
            .any(|entry| entry["type"]["path"].as_array() == Some(path))
        {
            return Err(format!(
                "retired {kind} enum type must be absent from candidate registry"
            ));
        }
        let outer_id = before["outer_enums"][outer]
            .as_u64()
            .ok_or("missing outer enum")?;
        let outer_id = usize::try_from(outer_id).map_err(|_| "outer type id does not fit usize")?;
        let variants = before["types"]["types"][outer_id]["type"]["def"]["variant"]["variants"]
            .as_array_mut()
            .ok_or("missing baseline outer variants")?;
        let matches: Vec<_> = variants.iter().filter(|v| v["index"] == 7).collect();
        if matches.len() != 1
            || matches[0]["name"] != retired["name"]
            || matches[0]["fields"].as_array().map(Vec::len) != Some(1)
            || matches[0]["fields"][0]["type"].as_u64() != Some(id)
        {
            return Err("retired outer variant must reference its pallet enum at index 7".into());
        }
        variants.retain(|v| v["index"] != 7);
    }
    Ok(removed_types)
}

pub fn check(before: &Value, after: &Value) -> Result<(), String> {
    // A stale outer clone must not satisfy the graph comparison after only
    // the pallet target was normalized (codex_rigorous_reviewer's P2 finding).
    for pallet in [MEMPOOL, "Xqvm"] {
        check_pallet_references(before, pallet).map_err(|error| format!("baseline: {error}"))?;
        check_pallet_references(after, pallet).map_err(|error| format!("candidate: {error}"))?;
    }
    let mut left = before.clone();
    let mut right = after.clone();
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
    normalize_mempool(&mut left, &mut right)?;
    let retired_types = normalize_retired_pallet(&mut left, &right)?;
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
    let vesting = additions[0].clone();
    pallets.retain(|p| p["name"] != "Vesting");
    for &(pallet, constant) in REGENERATED_WEIGHT_VALUES {
        let baseline = constant_value(&mut left, pallet, constant)?;
        let candidate = constant_value(&mut right, pallet, constant)?;
        // Normalize ONLY this named encoded value. The rest of the constant,
        // pallet and reachable type graph still undergo the strict comparison.
        *candidate = baseline.clone();
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
    let unvisited: BTreeSet<_> = comparison
        .before
        .iter()
        .map(|entry| entry["id"].as_u64().expect("registry IDs were checked"))
        .filter(|id| !visited.contains(id))
        .collect();
    if unvisited != retired_types {
        return Err(format!(
            "unvisited baseline types must equal retired enums: {unvisited:?} != {retired_types:?}"
        ));
    }
    // Vesting is the one approved new pallet removed before comparison. Walk
    // its candidate graph with the same reference traversal, without treating
    // that self-comparison as evidence of unchanged Vesting semantics. Shared
    // types were already compared above; this accounts for the addition's types.
    let mut addition = Comparison {
        before: comparison.after.clone(),
        after: comparison.after.clone(),
        outer: BTreeSet::new(),
        seen: BTreeSet::new(),
    };
    addition.value(&vesting, &vesting, "approved Vesting reachability")?;
    let covered_candidate: BTreeSet<_> = comparison
        .seen
        .iter()
        .chain(addition.seen.iter())
        .map(|(_, id)| *id)
        .collect();
    let candidate_orphans: BTreeSet<_> = comparison
        .after
        .iter()
        .map(|entry| entry["id"].as_u64().expect("registry IDs were checked"))
        .filter(|id| !covered_candidate.contains(id))
        .collect();
    if !candidate_orphans.is_empty() {
        return Err(format!(
            "candidate registry has unaccounted types: {candidate_orphans:?}"
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
    fn rejects_retired_enum_types_left_in_candidate_registry() {
        let (before, after) = metadata_pair();
        let retired = before["pallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["index"] == 7)
            .unwrap();
        for kind in ["calls", "event", "error"] {
            let id = retired[kind]["ty"].as_u64().unwrap() as usize;
            let mut wrong = after.clone();
            let registry = wrong["types"]["types"].as_array_mut().unwrap();
            let mut orphan = before["types"]["types"][id].clone();
            orphan["id"] = serde_json::json!(registry.len());
            registry.push(orphan);
            for (baseline, candidate) in [
                (before.clone(), wrong.clone()),
                (reverse_registry(before.clone()), wrong.clone()),
                (before.clone(), reverse_registry(wrong.clone())),
                (reverse_registry(before.clone()), reverse_registry(wrong)),
            ] {
                let error =
                    format!("retired {kind} enum type must be absent from candidate registry");
                assert_eq!(check(&baseline, &candidate), Err(error.clone()));
                // Changing documentation must not disguise the same retired identity.
                let mut changed = candidate.clone();
                let path = &before["types"]["types"][id]["type"]["path"];
                let orphan = changed["types"]["types"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|entry| entry["type"]["path"] == *path)
                    .unwrap();
                orphan["type"]["docs"] = serde_json::json!(["changed orphan documentation"]);
                assert_eq!(check(&baseline, &changed), Err(error));
            }
        }
    }

    #[test]
    fn rejects_renamed_retired_and_arbitrary_candidate_orphans() {
        let (before, after) = metadata_pair();
        let retired = before["pallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["index"] == 7)
            .unwrap();
        for kind in ["calls", "event", "error", "primitive"] {
            let mut orphan = if kind == "primitive" {
                serde_json::json!({"id": 0, "type": {"path": [], "params": [], "def": {"primitive": "u8"}, "docs": []}})
            } else {
                let id = retired[kind]["ty"].as_u64().unwrap() as usize;
                let mut entry = before["types"]["types"][id].clone();
                entry["type"]["path"] = serde_json::json!(["disguised", "RetiredType"]);
                entry
            };
            let mut wrong = after.clone();
            let registry = wrong["types"]["types"].as_array_mut().unwrap();
            orphan["id"] = serde_json::json!(registry.len());
            registry.push(orphan);
            for (baseline, candidate) in [
                (before.clone(), wrong.clone()),
                (reverse_registry(before.clone()), wrong.clone()),
                (before.clone(), reverse_registry(wrong.clone())),
                (reverse_registry(before.clone()), reverse_registry(wrong)),
            ] {
                assert!(
                    check(&baseline, &candidate)
                        .unwrap_err()
                        .starts_with("candidate registry has unaccounted types:"),
                    "wrong rejection for candidate orphan {kind}"
                );
            }
        }
    }

    #[test]
    fn rejects_any_additional_unvisited_baseline_type() {
        let (mut before, after) = metadata_pair();
        let registry = before["types"]["types"].as_array_mut().unwrap();
        let mut extra = registry
            .iter()
            .find(|entry| entry["type"]["def"].get("primitive").is_some())
            .unwrap()
            .clone();
        extra["id"] = serde_json::json!(registry.len());
        registry.push(extra);
        assert!(check(&before, &after)
            .unwrap_err()
            .starts_with("unvisited baseline types must equal retired enums:"));
    }

    #[test]
    fn rejects_reusing_retired_index_or_removing_another_pallet() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut wrong = after.clone();
        wrong["pallets"][0]["index"] = serde_json::json!(7);
        assert_eq!(
            check(&before, &wrong),
            Err("retired pallet index 7 must be absent and never reused".into())
        );
        let mut wrong = after.clone();
        wrong["pallets"].as_array_mut().unwrap().remove(0);
        assert!(check(&before, &wrong).is_err());
        // Leaving any of the retired outer variants behind must also fail.
        for outer in ["call_enum_ty", "event_enum_ty", "error_enum_ty"] {
            let mut wrong = after.clone();
            let old_id = before["outer_enums"][outer].as_u64().unwrap() as usize;
            let retired = before["types"]["types"][old_id]["type"]["def"]["variant"]["variants"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["index"] == 7)
                .unwrap()
                .clone();
            let new_id = wrong["outer_enums"][outer].as_u64().unwrap() as usize;
            wrong["types"]["types"][new_id]["type"]["def"]["variant"]["variants"]
                .as_array_mut()
                .unwrap()
                .push(retired);
            assert!(check(&before, &wrong).is_err());
        }
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
        for index in 36..45 {
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
}
