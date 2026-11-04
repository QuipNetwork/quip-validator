//! Compare metadata graphs, not portable registry numbering. Approved deltas
//! are Vesting at index 32 (including the three outer enums) and the explicitly
//! enumerated regenerated-weight values and exact xqvm integration changes below.
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

// Exact xqvm 0.5.0 integration delta, audited against all 673 baseline
// types in both runtimes. These are required values, not open-ended
// exemptions for call docs or error variants. Portable type ids are resolved
// through Xqvm's metadata so registry renumbering is still irrelevant.
pub fn xqvm_variants<'a>(
    metadata: &'a mut Value,
    kind: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let id = metadata["pallets"]
        .as_array()
        .and_then(|pallets| pallets.iter().find(|p| p["name"] == "Xqvm"))
        .and_then(|pallet| pallet[kind]["ty"].as_u64())
        .ok_or_else(|| format!("missing Xqvm.{kind} type"))?;
    let id = usize::try_from(id).map_err(|_| "Xqvm type id does not fit usize")?;
    metadata
        .get_mut("types")
        .and_then(|types| types.get_mut("types"))
        .and_then(|types| types.get_mut(id))
        .and_then(|ty| ty.pointer_mut("/type/def/variant/variants"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("missing Xqvm.{kind} variants"))
}

fn normalize_xqvm(before: &mut Value, after: &mut Value) -> Result<(), String> {
    let baseline_docs = xqvm_variants(before, "calls")?
        .iter()
        .find(|call| call["name"] == "execute")
        .and_then(|call| call["docs"].as_array())
        .filter(|docs| !docs.is_empty())
        .ok_or("missing baseline Xqvm.execute docs")?
        .clone();
    let docs = xqvm_variants(after, "calls")?
        .iter_mut()
        .find(|call| call["name"] == "execute")
        .and_then(|call| call.get_mut("docs"))
        .ok_or("missing candidate Xqvm.execute docs")?;
    if *docs == Value::Array(baseline_docs.clone()) {
        return Err("missing required Xqvm.execute.docs delta".into());
    }
    let mut expected_docs = baseline_docs.clone();
    // Insert exactly this paragraph after the heading; preserve every old line.
    expected_docs.splice(
        1..1,
        [
            "",
            "Outputs are integers. Unwritten slots are reported as zero;",
            "a model, sample, or vector output fails with `UnsupportedOutputType`",
            "and emits no `ProgramExecuted` event. Programs that construct these",
            "values must derive integer results before returning them on chain.",
        ]
        .into_iter()
        .map(Value::from),
    );
    if *docs != Value::Array(expected_docs) {
        return Err("unexpected Xqvm.execute.docs delta".into());
    }
    *docs = Value::Array(baseline_docs);

    let baseline_len = xqvm_variants(before, "error")?.len();
    let variants = xqvm_variants(after, "error")?;
    if variants.len() == baseline_len {
        return Err("missing required Xqvm.UnsupportedOutputType delta".into());
    }
    let expected_error = serde_json::json!({
        "docs": [
            "A completed program returned a model, sample, or vector.",
            "",
            "On-chain outputs support integers only. Unwritten slots retain",
            "their zero representation; typed non-scalar values are rejected",
            "rather than silently replaced with zero. No success event is emitted."
        ],
        "index": 37,
        "name": "UnsupportedOutputType"
    });
    if variants.last() != Some(&expected_error) {
        return Err("unexpected Xqvm.UnsupportedOutputType delta".into());
    }
    // Remove ONLY this exact appended variant. The original error enum,
    // including every existing variant's docs/index/fields, is still compared.
    let _ = variants.pop();
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

pub fn check(before: &Value, after: &Value) -> Result<(), String> {
    let mut left = before.clone();
    let mut right = after.clone();
    normalize_xqvm(&mut left, &mut right)?;
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
        if candidate == baseline {
            return Err(format!(
                "missing required regenerated-weight delta: {pallet}.{constant}.value"
            ));
        }
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
    if visited.len() != comparison.before.len() {
        return Err(format!(
            "baseline registry not fully checked: {} of {}",
            visited.len(),
            comparison.before.len()
        ));
    }
    Ok(())
}
