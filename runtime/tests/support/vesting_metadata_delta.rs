//! Compare metadata graphs, not portable registry numbering. Approved deltas
//! are Vesting at index 32 (including the three outer enums) and the explicitly
//! enumerated regenerated-weight values, exact mempool VM delta, exact
//! quantum-pow family delta, and retirement of pallet index 7 below.
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
    pallet_enum_variants(metadata, MEMPOOL, "error")
}

/// The variants of a pallet's `calls`, `event`, or `error` enum, found through
/// the pallet entry so a stale outer clone cannot stand in for them.
fn pallet_enum_variants<'a>(
    metadata: &'a mut Value,
    pallet_name: &str,
    kind: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == pallet_name))
        .and_then(|pallet| pallet[kind]["ty"].as_u64())
        .ok_or_else(|| format!("missing {pallet_name} {kind} type"))?;
    let id =
        usize::try_from(id).map_err(|_| format!("{pallet_name} {kind} id does not fit usize"))?;
    metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(|types| types.get_mut(id))
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("missing {pallet_name} {kind} variants"))
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

// Exact pallet-quantum-pow family delta (QUI-2044) against the immutable
// pre-Vesting captures: the trailing `family` argument of `register_topology`,
// the `set_topology_energy_band` call at index 11, the `TopologyEnergyBandSet`
// event appended at index 12, six errors appended at 27..=32, the
// `TopologyEnergyBand` storage map, the `family` field of `TopologyMeta` and
// `MiningSnapshot`, and `QuantumPowApi` version 3 with its docs. Each piece is
// removed from the candidate only after an exact match, so every existing
// field still undergoes the graph comparison, and `check` walks the removed
// pieces so their types are accounted for. Type references are compared by
// identity with existing references, never by registry number.
const QUANTUM_POW: &str = "QuantumPow";
const TOPOLOGY_META: &[&str] = &["pallet_quantum_pow", "types", "TopologyMeta"];
const MINING_SNAPSHOT: &[&str] = &["pallet_quantum_pow", "types", "MiningSnapshot"];
const QUANTUM_POW_API_V3_DOCS: &[&str] = &[
    "",
    " Version 3 adds the `family` field to `MiningSnapshot` and",
    " `TopologyMeta`, so their encodings differ from version 2.",
];

/// `value` with every portable type reference replaced by `null`, so an exact
/// shape comparison does not depend on registry numbering.
fn without_type_ids(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let masked =
                        if matches!(key.as_str(), "type" | "key" | "value") && value.is_u64() {
                            Value::Null
                        } else {
                            without_type_ids(value)
                        };
                    (key.clone(), masked)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(without_type_ids).collect()),
        other => other.clone(),
    }
}

/// The fields of the one composite type at `path` in the registry.
fn composite_fields<'a>(
    metadata: &'a mut Value,
    path: &[&str],
) -> Result<&'a mut Vec<Value>, String> {
    let name = path.join("::");
    let registry = metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(Value::as_array_mut)
        .ok_or("missing registry")?;
    let matches: Vec<usize> = registry
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry["type"]["path"].as_array().is_some_and(|p| {
                p.iter()
                    .map(Value::as_str)
                    .eq(path.iter().map(|s| Some(*s)))
            })
        })
        .map(|(index, _)| index)
        .collect();
    let [index] = matches[..] else {
        return Err(format!("type {name} must appear exactly once"));
    };
    registry[index]
        .pointer_mut("/type/def/composite/fields")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("type {name} is not a composite"))
}

fn variant_fields<'a>(variants: &'a mut [Value], name: &str) -> Result<&'a mut Vec<Value>, String> {
    variants
        .iter_mut()
        .find(|variant| variant["name"] == name)
        .and_then(|variant| variant["fields"].as_array_mut())
        .ok_or_else(|| format!("missing {name} fields"))
}

fn storage_entries<'a>(
    metadata: &'a mut Value,
    pallet_name: &str,
) -> Result<&'a mut Vec<Value>, String> {
    metadata["pallets"]
        .as_array_mut()
        .and_then(|pallets| pallets.iter_mut().find(|p| p["name"] == pallet_name))
        .and_then(|pallet| pallet["storage"]["entries"].as_array_mut())
        .ok_or_else(|| format!("missing {pallet_name} storage entries"))
}

fn runtime_api<'a>(metadata: &'a mut Value, name: &str) -> Result<&'a mut Value, String> {
    metadata["apis"]
        .as_array_mut()
        .and_then(|apis| apis.iter_mut().find(|api| api["name"] == name))
        .ok_or_else(|| format!("missing runtime api {name}"))
}

/// Remove the one `family` field added to a composite of `baseline_len`
/// fields. It must reference the same `Option<FamilyDescriptor>` type as
/// `family_type`. The remaining fields then undergo the graph comparison, so
/// the position of the addition is checked there.
fn take_family_field(
    fields: &mut Vec<Value>,
    baseline_len: usize,
    family_type: u64,
    context: &str,
) -> Result<Value, String> {
    let expected = serde_json::json!({
        "name": "family",
        "type": family_type,
        "typeName": "Option<FamilyDescriptor>"
    });
    let position = fields
        .iter()
        .position(|field| field["name"] == "family")
        .filter(|&position| fields.len() == baseline_len + 1 && fields[position] == expected)
        .ok_or_else(|| format!("unexpected required {context} family field delta"))?;
    Ok(fields.remove(position))
}

fn normalize_quantum_pow(before: &mut Value, after: &mut Value) -> Result<Vec<Value>, String> {
    let mut pieces = Vec::new();

    // Calls: the trailing `family` argument and the new call at index 11.
    let baseline_calls = pallet_enum_variants(before, QUANTUM_POW, "calls")?;
    if baseline_calls.len() != 11
        || baseline_calls
            .iter()
            .any(|call| call["name"] == "set_topology_energy_band")
        || variant_fields(baseline_calls, "register_topology")?.len() != 5
    {
        return Err("unexpected baseline QuantumPow calls".into());
    }
    let calls = pallet_enum_variants(after, QUANTUM_POW, "calls")?;
    let fields = variant_fields(calls, "register_topology")?;
    let expected = serde_json::json!({
        "name": "family",
        "type": null,
        "typeName": "Option<types::FamilyDescriptor>"
    });
    if fields.len() != 6 || without_type_ids(&fields[5]) != expected || !fields[5]["type"].is_u64()
    {
        return Err("unexpected required register_topology family argument delta".into());
    }
    let family_type = fields[5]["type"].as_u64().ok_or("family argument type")?;
    pieces.push(fields.remove(5));
    // Identity of the new call's references: `topology_hash` is the H256 of
    // the existing `set_topology_curve`, `band` is the storage value below.
    let topology_hash_type = variant_fields(calls, "set_topology_curve")?[0]["type"].clone();
    let expected = serde_json::json!({
        "name": "set_topology_energy_band",
        "fields": [
            {"name": "topology_hash", "type": topology_hash_type, "typeName": "H256"},
            {"name": "band", "type": null, "typeName": "crate::difficulty::EnergyBand"}
        ],
        "index": 11,
        "docs": [
            "Set (or replace) the energy band of a family topology (root only).",
            "",
            "The difficulty controller eases and hardens within this band at",
            "the module's fixed rates; `set_difficulty` sets the starting",
            "point. The band must be ordered, `hard_milli < easy_milli`. A",
            "legacy topology is refused: its curve comes from `expected_gse`",
            "and `set_topology_curve`."
        ]
    });
    // Variants follow declaration order, not call index, so find it by name.
    let position = calls
        .iter()
        .position(|call| call["name"] == "set_topology_energy_band")
        .ok_or("unexpected required QuantumPow call delta")?;
    let mut masked = without_type_ids(&calls[position]);
    masked["fields"][0]["type"] = calls[position]["fields"][0]["type"].clone();
    if calls.len() != 12 || masked != expected || !calls[position]["fields"][1]["type"].is_u64() {
        return Err("unexpected required QuantumPow call delta".into());
    }
    let band_type = calls[position]["fields"][1]["type"]
        .as_u64()
        .ok_or("band type")?;
    pieces.push(calls.remove(position));

    // Events: one appended variant.
    if pallet_enum_variants(before, QUANTUM_POW, "event")?.len() != 12 {
        return Err("unexpected baseline QuantumPow events".into());
    }
    let events = pallet_enum_variants(after, QUANTUM_POW, "event")?;
    let expected = serde_json::json!({
        "name": "TopologyEnergyBandSet",
        "fields": [{"name": "topology_hash", "type": topology_hash_type, "typeName": "H256"}],
        "index": 12,
        "docs": [
            "Root set the energy band a family topology's difficulty",
            "controller runs on. Appended last so the earlier event indices",
            "stay as the pre-family metadata recorded them."
        ]
    });
    if events.len() != 13 || events[12] != expected {
        return Err("unexpected required QuantumPow event delta".into());
    }
    pieces.push(events.remove(12));

    // Errors: six appended variants.
    if pallet_enum_variants(before, QUANTUM_POW, "error")?.len() != 27 {
        return Err("unexpected baseline QuantumPow errors".into());
    }
    let expected = serde_json::json!([
        {
            "name": "FamilyRejected",
            "index": 27,
            "docs": [
                "Admission refused the family descriptor or the topology: the",
                "family is not in `PowFamilies`, the parameters are malformed or",
                "out of bounds, or the topology does not fit the family."
            ]
        },
        {
            "name": "FamilySpinSetRequired",
            "index": 28,
            "docs": [
                "A family topology must register the two-value spin set, because",
                "every family is a `±1` model."
            ]
        },
        {
            "name": "InvalidEnergyBand",
            "index": 29,
            "docs": ["The energy band is not ordered (`hard_milli < easy_milli`)."]
        },
        {
            "name": "EnergyBandMissing",
            "index": 30,
            "docs": [
                "A family topology cannot become mineable before root sets its",
                "energy band; the difficulty controller has no curve without one."
            ]
        },
        {
            "name": "FamilyNonceSkipped",
            "index": 31,
            "docs": [
                "The nonce gives no instance for this family (code 13 or 14); the",
                "miner must try another salt."
            ]
        },
        {
            "name": "FamilyWitnessInvalid",
            "index": 32,
            "docs": [
                "A witness has the wrong length, a spin outside `±1`, breaks a hard",
                "constraint of the family, or its energy overflows."
            ]
        }
    ]);
    let errors = pallet_enum_variants(after, QUANTUM_POW, "error")?;
    if errors.get(27..) != expected.as_array().map(Vec::as_slice) {
        return Err("unexpected required QuantumPow error delta".into());
    }
    errors.truncate(27);

    // Storage: the `TopologyEnergyBand` map, keyed like `TopologyCurveC` and
    // valued with the call's `EnergyBand`.
    let baseline_entries = storage_entries(before, QUANTUM_POW)?;
    if baseline_entries.len() != 16
        || baseline_entries
            .iter()
            .any(|entry| entry["name"] == "TopologyEnergyBand")
    {
        return Err("unexpected baseline QuantumPow storage".into());
    }
    let entries = storage_entries(after, QUANTUM_POW)?;
    let key_type = entries
        .iter()
        .find(|entry| entry["name"] == "TopologyCurveC")
        .map(|entry| entry["ty"]["Map"]["key"].clone())
        .ok_or("missing TopologyCurveC storage entry")?;
    let expected = serde_json::json!({
        "name": "TopologyEnergyBand",
        "modifier": "Optional",
        "ty": {"Map": {"hashers": ["Blake2_128Concat"], "key": key_type, "value": band_type}},
        "default": [0],
        "docs": [
            " Root-set energy band of a family topology, set via",
            " `set_topology_energy_band`. `energy_curve_for` runs the controller on",
            " this band because a family's coefficients do not follow the",
            " `expected_gse` model. A family topology cannot become mineable",
            " without one."
        ],
        "deprecation_info": "NotDeprecated"
    });
    let position = entries
        .iter()
        .position(|entry| entry["name"] == "TopologyEnergyBand")
        .ok_or("unexpected required QuantumPow storage delta")?;
    if entries.len() != 17 || entries[position] != expected {
        return Err("unexpected required QuantumPow storage delta".into());
    }
    pieces.push(entries.remove(position));

    // Named types: `family` appended to `TopologyMeta` and `MiningSnapshot`.
    for (path, context) in [
        (TOPOLOGY_META, "TopologyMeta"),
        (MINING_SNAPSHOT, "MiningSnapshot"),
    ] {
        let baseline = composite_fields(before, path)?;
        if baseline.iter().any(|field| field["name"] == "family") {
            return Err(format!("unexpected baseline {context} family field"));
        }
        let baseline_len = baseline.len();
        let fields = composite_fields(after, path)?;
        pieces.push(take_family_field(
            fields,
            baseline_len,
            family_type,
            context,
        )?);
    }

    // Runtime API: version 3 and its documentation.
    let baseline = runtime_api(before, "QuantumPowApi")?;
    let baseline_docs = baseline["docs"]
        .as_array()
        .ok_or("invalid baseline QuantumPowApi docs")?
        .clone();
    if baseline["version"] != 2
        || baseline_docs
            .iter()
            .any(|line| line == QUANTUM_POW_API_V3_DOCS[1])
    {
        return Err("unexpected baseline QuantumPowApi".into());
    }
    let mut expected_docs = baseline_docs.clone();
    expected_docs.extend(QUANTUM_POW_API_V3_DOCS.iter().copied().map(Value::from));
    let api = runtime_api(after, "QuantumPowApi")?;
    if api["version"] != 3 || api["docs"] != Value::Array(expected_docs) {
        return Err("unexpected required QuantumPowApi version delta".into());
    }
    api["version"] = serde_json::json!(2);
    api["docs"] = Value::Array(baseline_docs);

    // `System.Version` encodes `RuntimeVersion`, whose `apis` list carries the
    // 8-byte id and the version of every runtime API. Exactly the
    // `QuantumPowApi` version moves from 2 to 3; every other byte is equal.
    let bytes = |value: &Value| -> Result<Vec<u8>, String> {
        value
            .as_array()
            .ok_or("System.Version is not a byte array")?
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .and_then(|byte| u8::try_from(byte).ok())
                    .ok_or_else(|| "System.Version byte out of range".to_string())
            })
            .collect()
    };
    let baseline_version = constant_value(before, "System", "Version")?.clone();
    let baseline_bytes = bytes(&baseline_version)?;
    let api_id = sp_core::hashing::blake2_64(b"QuantumPowApi");
    let positions: Vec<usize> = baseline_bytes
        .windows(api_id.len())
        .enumerate()
        .filter(|(_, window)| *window == api_id)
        .map(|(position, _)| position + api_id.len())
        .collect();
    let [version_at] = positions[..] else {
        return Err("QuantumPowApi must appear exactly once in the baseline System.Version".into());
    };
    if baseline_bytes.get(version_at..version_at + 4) != Some(&2u32.to_le_bytes()[..]) {
        return Err("unexpected baseline QuantumPowApi runtime version".into());
    }
    let mut expected_bytes = baseline_bytes;
    expected_bytes[version_at..version_at + 4].copy_from_slice(&3u32.to_le_bytes());
    let candidate_version = constant_value(after, "System", "Version")?;
    if bytes(candidate_version)? != expected_bytes {
        return Err("unexpected required QuantumPowApi runtime version delta".into());
    }
    *candidate_version = baseline_version;
    Ok(pieces)
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
    for pallet in [MEMPOOL, "Xqvm", QUANTUM_POW] {
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
    let family_pieces = normalize_quantum_pow(&mut left, &mut right)?;
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
    // The family pieces removed by normalize_quantum_pow reference the new
    // descriptor and band types. Walk them the same way.
    for piece in &family_pieces {
        addition.value(piece, piece, "approved family reachability")?;
    }
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

    #[test]
    fn requires_exact_quantum_pow_family_delta() {
        let (before, after) = metadata_pair();
        check(&before, &after).unwrap();
        let mut wrong = after.clone();
        pallet_enum_variants(&mut wrong, QUANTUM_POW, "calls")
            .unwrap()
            .retain(|call| call["name"] != "set_topology_energy_band");
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required QuantumPow call delta".into())
        );
        let mut wrong = after.clone();
        variant_fields(
            pallet_enum_variants(&mut wrong, QUANTUM_POW, "calls").unwrap(),
            "register_topology",
        )
        .unwrap()
        .pop();
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required register_topology family argument delta".into())
        );
        let mut wrong = after.clone();
        pallet_enum_variants(&mut wrong, QUANTUM_POW, "event")
            .unwrap()
            .pop();
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required QuantumPow event delta".into())
        );
        for index in 27..33 {
            let mut wrong = after.clone();
            pallet_enum_variants(&mut wrong, QUANTUM_POW, "error").unwrap()[index]["docs"] =
                serde_json::json!(["mutated"]);
            assert_eq!(
                check(&before, &wrong),
                Err("unexpected required QuantumPow error delta".into()),
                "accepted mutated error {index}"
            );
        }
        let mut wrong = after.clone();
        storage_entries(&mut wrong, QUANTUM_POW)
            .unwrap()
            .retain(|entry| entry["name"] != "TopologyEnergyBand");
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required QuantumPow storage delta".into())
        );
        for (path, context) in [
            (TOPOLOGY_META, "TopologyMeta"),
            (MINING_SNAPSHOT, "MiningSnapshot"),
        ] {
            let mut wrong = after.clone();
            composite_fields(&mut wrong, path)
                .unwrap()
                .retain(|field| field["name"] != "family");
            assert_eq!(
                check(&before, &wrong),
                Err(format!("unexpected required {context} family field delta"))
            );
        }
        let mut wrong = after.clone();
        runtime_api(&mut wrong, "QuantumPowApi").unwrap()["version"] = serde_json::json!(2);
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required QuantumPowApi version delta".into())
        );
        let mut wrong = after.clone();
        *constant_value(&mut wrong, "System", "Version").unwrap() =
            constant_value(&mut before.clone(), "System", "Version")
                .unwrap()
                .clone();
        assert_eq!(
            check(&before, &wrong),
            Err("unexpected required QuantumPowApi runtime version delta".into())
        );
        // The baseline must not already carry any piece of the delta.
        let mut wrong = before.clone();
        *constant_value(&mut wrong, "System", "Version").unwrap() =
            constant_value(&mut after.clone(), "System", "Version")
                .unwrap()
                .clone();
        assert_eq!(
            check(&wrong, &after),
            Err("unexpected baseline QuantumPowApi runtime version".into())
        );
        let mut wrong = before.clone();
        let added = pallet_enum_variants(&mut after.clone(), QUANTUM_POW, "calls")
            .unwrap()
            .iter()
            .find(|call| call["name"] == "set_topology_energy_band")
            .unwrap()
            .clone();
        pallet_enum_variants(&mut wrong, QUANTUM_POW, "calls")
            .unwrap()
            .push(added);
        assert_eq!(
            check(&wrong, &after),
            Err("unexpected baseline QuantumPow calls".into())
        );
        let mut wrong = before.clone();
        runtime_api(&mut wrong, "QuantumPowApi").unwrap()["version"] = serde_json::json!(3);
        assert_eq!(
            check(&wrong, &after),
            Err("unexpected baseline QuantumPowApi".into())
        );
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
