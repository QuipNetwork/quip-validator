//! Independent metadata invariants and anchor pins; SCALE byte equality is the gate.
use serde_json::Value;

pub const ANCHOR_FILES: [&str; 4] = [
    "quip.scale",
    "quip-testnet.scale",
    "quip-storage.json",
    "quip-testnet-storage.json",
];

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
        let mut matching = variants
            .iter()
            .filter(|variant| variant["name"] == pallet_name);
        let fields = matching
            .next()
            .and_then(|variant| variant["fields"].as_array());
        if matching.next().is_some()
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

fn invariants(metadata: &Value) -> Result<(), String> {
    let registry = metadata["types"]["types"]
        .as_array()
        .ok_or("missing registry")?;
    for (index, ty) in registry.iter().enumerate() {
        if ty["id"].as_u64() != Some(index as u64) {
            return Err(format!(
                "metadata.types.types[{index}].id: noncanonical registry indexing"
            ));
        }
    }
    for pallet in ["Xqvm", "QuantumComputeMempool"] {
        check_pallet_references(metadata, pallet)?;
    }
    let pallets = metadata["pallets"].as_array().ok_or("missing pallets")?;
    check_indices(pallets, "metadata.pallets")?;
    for outer in ["call_enum_ty", "event_enum_ty", "error_enum_ty"] {
        let id = metadata["outer_enums"][outer]
            .as_u64()
            .ok_or("missing outer enum")?;
        let id = usize::try_from(id).map_err(|_| "outer type id does not fit usize")?;
        let variants = registry
            .get(id)
            .and_then(|ty| ty.pointer("/type/def/variant/variants"))
            .and_then(Value::as_array)
            .ok_or("missing outer variants")?;
        check_indices(variants, &format!("metadata.outer_enums.{outer}"))?;
    }
    Ok(())
}

fn check_indices(entries: &[Value], path: &str) -> Result<(), String> {
    for index in [6, 7] {
        if entries.iter().any(|entry| entry["index"] == index) {
            return Err(format!(
                "{path}: retired index {index} must never be reused"
            ));
        }
    }
    let vesting: Vec<_> = entries
        .iter()
        .filter(|entry| entry["name"] == "Vesting" || entry["index"] == 32)
        .collect();
    if vesting.len() != 1 || vesting[0]["name"] != "Vesting" || vesting[0]["index"] != 32 {
        return Err(format!(
            "{path}: Vesting must appear exactly once at index 32"
        ));
    }
    Ok(())
}

pub fn check_invariants(anchor: &Value, candidate: &Value) -> Result<(), String> {
    // Check both inputs independently before comparison; equality must not
    // conceal a bad anchor or a split outer reference with identical content.
    invariants(anchor).map_err(|error| format!("anchor: {error}"))?;
    invariants(candidate).map_err(|error| format!("candidate: {error}"))?;
    Ok(())
}

// Only the anchor's size and digest are enforced. Compiler and lockfile
// information describe the capture environment, not the current checkout.
pub fn check_anchor_pin(manifest: &Value, name: &str, bytes: &[u8]) -> Result<(), String> {
    let entry = manifest["files"]
        .get(name)
        .ok_or_else(|| format!("manifest: missing files.{name}"))?;
    let size = entry["bytes"]
        .as_u64()
        .ok_or_else(|| format!("manifest: missing or invalid files.{name}.bytes"))?;
    if size != bytes.len() as u64 {
        return Err(format!(
            "{name}: byte size mismatch: manifest {size}, file {}",
            bytes.len()
        ));
    }
    let expected = entry["sha256"]
        .as_str()
        .ok_or_else(|| format!("manifest: missing or invalid files.{name}.sha256"))?;
    let actual = hex::encode(sp_io::hashing::sha2_256(bytes));
    if expected != actual {
        return Err(format!(
            "{name}: sha256 mismatch: manifest {expected}, file {actual}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_wrong_or_missing_anchor_pins_but_ignores_provenance() {
        let path = super::super::native_2609_anchor_path();
        let directory = path.parent().unwrap();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap())
                .unwrap();
        for name in ANCHOR_FILES {
            let bytes = std::fs::read(directory.join(name)).unwrap();
            check_anchor_pin(&manifest, name, &bytes).unwrap();
            for mutation in ["digest", "size", "entry", "missing_digest", "missing_size"] {
                let mut wrong = manifest.clone();
                match mutation {
                    "digest" => wrong["files"][name]["sha256"] = json!("00".repeat(32)),
                    "size" => wrong["files"][name]["bytes"] = json!(bytes.len() + 1),
                    "entry" => {
                        wrong["files"].as_object_mut().unwrap().remove(name);
                    }
                    "missing_digest" => {
                        wrong["files"][name]
                            .as_object_mut()
                            .unwrap()
                            .remove("sha256");
                    }
                    "missing_size" => {
                        wrong["files"][name]
                            .as_object_mut()
                            .unwrap()
                            .remove("bytes");
                    }
                    _ => unreachable!(),
                }
                let error = check_anchor_pin(&wrong, name, &bytes).expect_err("bad pin must fail");
                assert!(error.contains(name), "{error}");
                eprintln!("{mutation} rejected: {error}");
            }
            let mut provenance = manifest.clone();
            provenance["provenance"]["rustc"] = json!("different compiler");
            provenance["provenance"]["cargo_lock_sha256"] = json!("different lockfile");
            check_anchor_pin(&provenance, name, &bytes).unwrap();
        }
    }

    fn metadata_pair() -> (Value, Value) {
        let anchor = serde_json::to_value(super::super::native_2609_metadata()).unwrap();
        let candidate = serde_json::to_value(super::super::metadata_v16()).unwrap();
        check_invariants(&anchor, &candidate).unwrap();
        (anchor, candidate)
    }

    fn pallet_index(metadata: &Value, name: &str) -> usize {
        metadata["pallets"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["name"] == name)
            .unwrap()
    }

    fn rejected_on_both_sides(anchor: &Value, candidate: &Value, wrong: &Value, expected: &str) {
        for (side, left, right) in [("anchor", wrong, candidate), ("candidate", anchor, wrong)] {
            let error = check_invariants(left, right).expect_err("mutation must be rejected");
            assert!(
                error == expected || error == format!("{side}: {expected}"),
                "{error}"
            );
            eprintln!("{side} mutation rejected: {error}");
        }
    }

    #[test]
    fn rejects_split_outer_references() {
        let (anchor, candidate) = metadata_pair();
        for pallet in ["Xqvm", "QuantumComputeMempool"] {
            for (kind, outer, name) in [
                ("calls", "call_enum_ty", "RuntimeCall"),
                ("error", "error_enum_ty", "RuntimeError"),
            ] {
                let mut wrong = candidate.clone();
                let p = pallet_index(&wrong, pallet);
                let id = wrong["pallets"][p][kind]["ty"].as_u64().unwrap() as usize;
                let outer_id = wrong["outer_enums"][outer].as_u64().unwrap() as usize;
                let registry = wrong["types"]["types"].as_array_mut().unwrap();
                let clone_id = registry.len();
                let mut cloned = registry[id].clone();
                cloned["id"] = json!(clone_id);
                registry.push(cloned);
                let variants = registry[outer_id]["type"]["def"]["variant"]["variants"]
                    .as_array_mut()
                    .unwrap();
                let variant = variants.iter_mut().find(|v| v["name"] == pallet).unwrap();
                variant["fields"][0]["type"] = json!(clone_id);
                let expected =
                    format!("{name}::{pallet} must reference the pallet {pallet}.{kind} type");
                rejected_on_both_sides(&anchor, &candidate, &wrong, &expected);
                assert_eq!(
                    check_invariants(&wrong, &wrong),
                    Err(format!("anchor: {expected}"))
                );
            }
        }
    }

    #[test]
    fn rejects_retired_indices() {
        let (anchor, candidate) = metadata_pair();
        for retired in [6, 7] {
            for outer in [
                None,
                Some("call_enum_ty"),
                Some("event_enum_ty"),
                Some("error_enum_ty"),
            ] {
                let mut wrong = candidate.clone();
                let path = if let Some(outer) = outer {
                    let id = wrong["outer_enums"][outer].as_u64().unwrap() as usize;
                    wrong["types"]["types"][id]["type"]["def"]["variant"]["variants"][0]["index"] =
                        json!(retired);
                    format!("metadata.outer_enums.{outer}")
                } else {
                    wrong["pallets"][0]["index"] = json!(retired);
                    "metadata.pallets".into()
                };
                rejected_on_both_sides(
                    &anchor,
                    &candidate,
                    &wrong,
                    &format!("{path}: retired index {retired} must never be reused"),
                );
            }
        }
    }

    #[test]
    fn rejects_noncanonical_registry_indices() {
        let (anchor, candidate) = metadata_pair();
        let mut wrong = candidate.clone();
        wrong["types"]["types"][0]["id"] = json!(1);
        rejected_on_both_sides(
            &anchor,
            &candidate,
            &wrong,
            "metadata.types.types[0].id: noncanonical registry indexing",
        );
    }

    #[test]
    fn requires_vesting_once_at_index_32() {
        let (anchor, candidate) = metadata_pair();
        for outer in [
            None,
            Some("call_enum_ty"),
            Some("event_enum_ty"),
            Some("error_enum_ty"),
        ] {
            for mutation in ["missing", "duplicate", "moved", "occupied"] {
                let mut wrong = candidate.clone();
                let (entries, path) = if let Some(outer) = outer {
                    let id = wrong["outer_enums"][outer].as_u64().unwrap() as usize;
                    (
                        &mut wrong["types"]["types"][id]["type"]["def"]["variant"]["variants"],
                        format!("metadata.outer_enums.{outer}"),
                    )
                } else {
                    (&mut wrong["pallets"], "metadata.pallets".into())
                };
                let entries = entries.as_array_mut().unwrap();
                let index = entries.iter().position(|v| v["name"] == "Vesting").unwrap();
                match mutation {
                    "missing" => {
                        entries.remove(index);
                    }
                    "duplicate" => entries.push(entries[index].clone()),
                    "moved" => entries[index]["index"] = json!(33),
                    "occupied" => {
                        let other = entries.iter_mut().find(|v| v["name"] != "Vesting").unwrap();
                        other["index"] = json!(32);
                    }
                    _ => unreachable!(),
                }
                rejected_on_both_sides(
                    &anchor,
                    &candidate,
                    &wrong,
                    &format!("{path}: Vesting must appear exactly once at index 32"),
                );
            }
        }
    }
}
