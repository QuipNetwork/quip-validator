//! Canonical compact witness vectors for every family revision.
//! Compact instances here only provide the context for the witness fixtures.
//! Production verification regenerates instances from the nonce.
use ising_generators::{
    descriptor::{admit, params_digest},
    registry::{self, CONTRACTS},
};
use serde_json::Value;
fn unhex(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(hex::decode(s)?)
}
fn field<'a>(v: &'a Value, names: &[&str]) -> Result<&'a str, Box<dyn std::error::Error>> {
    names
        .iter()
        .find_map(|name| v.get(name).and_then(Value::as_str))
        .ok_or_else(|| "missing golden field".into())
}
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report missing registry codec coverage"
)]
fn every_family_witness_codec_matches_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = [
        (
            "quip/polariton-static",
            include_str!("fixtures/solver-core/families/codecs/polariton.json"),
        ),
        (
            "quip/defects-static",
            include_str!("fixtures/solver-core/families/codecs/defects.json"),
        ),
        (
            "quip/flow-static",
            include_str!("fixtures/solver-core/families/codecs/flow.json"),
        ),
        (
            "quip/twisted-ice-dimer",
            include_str!("fixtures/solver-core/families/codecs/twisted.json"),
        ),
        (
            "quip/charged-ice",
            include_str!("fixtures/solver-core/families/codecs/charge.json"),
        ),
        (
            "quip/ice-glass",
            include_str!("fixtures/solver-core/families/codecs/glass.json"),
        ),
        (
            "quip/ice-fields",
            include_str!("fixtures/solver-core/families/codecs/fields.json"),
        ),
        (
            "quip/two-commodity-routing",
            include_str!("fixtures/solver-core/families/codecs/routing.json"),
        ),
    ];
    assert_eq!(fixtures.len(), CONTRACTS.len());
    for contract in CONTRACTS {
        let (_, source) = fixtures
            .iter()
            .find(|(id, _)| *id == contract.family_id)
            .ok_or("uncovered contract")?;
        let document: Value = serde_json::from_str(source)?;
        let rows = document
            .get("cases")
            .and_then(Value::as_array)
            .ok_or("cases")?;
        for row in rows.iter().filter(|r| {
            r.get("kind")
                .and_then(Value::as_str)
                .is_none_or(|k| k == "instance")
        }) {
            check_case(contract.family_id, contract.revision, row, rows).map_err(|error| {
                format!(
                    "{} {}: {error}",
                    contract.family_id,
                    field(row, &["name", "id"]).unwrap_or("unnamed")
                )
            })?;
        }
    }
    Ok(())
}

fn rows_field<'a>(row: &'a Value, keys: &[&str]) -> &'a [Value] {
    keys.iter()
        .find_map(|key| row.get(key).and_then(Value::as_array))
        .map_or(&[], Vec::as_slice)
}
#[expect(
    clippy::panic_in_result_fn,
    reason = "the linear golden checker preserves each family-local schema and error context"
)]
fn check_case(
    family: &str,
    revision: u32,
    row: &Value,
    rows: &[Value],
) -> Result<(), Box<dyn std::error::Error>> {
    let params = unhex(field(row, &["params", "params_hex"])?)?;
    let admitted = admit(
        CONTRACTS,
        family.as_bytes(),
        revision,
        &params,
        &params_digest(&params),
    )?;
    let compact = unhex(field(row, &["compact", "compact_hex", "compact_instance"])?)?;
    let instance = registry::decode_instance(&admitted, &compact)?;
    assert_eq!(
        instance.canonical_bytes()?,
        unhex(field(row, &["canonical", "canonical_hex"])?)?
    );
    let name = field(row, &["name", "id"])?;
    let related: Vec<_> = rows
        .iter()
        .filter(|r| r.get("instance").and_then(Value::as_str) == Some(name))
        .collect();
    let prepared = registry::prepare_witness_codec(&admitted, &instance);
    if (family == "quip/two-commodity-routing" && name == "wider")
        || (family == "quip/flow-static" && name == "flow-structural-nongenerated")
    {
        assert_eq!(
            prepared.err(),
            Some(ising_generators::codec::CodecError::InvalidInstance)
        );
        return Ok(());
    }
    let codec = prepared?;
    let witnesses: Vec<_> = if family == "quip/flow-static" {
        related
            .iter()
            .copied()
            .filter(|r| r.get("kind").and_then(Value::as_str) == Some("witness"))
            .collect()
    } else if row.get("witnesses").is_some() {
        rows_field(row, &["witnesses"]).iter().collect()
    } else {
        vec![row]
    };
    for witness in witnesses {
        let bytes = unhex(field(
            witness,
            &["witness", "compact", "compact_hex", "hex"],
        )?)?;
        let spins: Vec<i8> = serde_json::from_value(witness.get("spins").ok_or("spins")?.clone())?;
        assert_eq!(codec.encode(&spins)?, bytes);
        assert_eq!(codec.decode(&bytes)?, spins);
    }
    let bad_witnesses: Vec<_> = if family == "quip/flow-static" {
        related
            .iter()
            .copied()
            .filter(|r| r.get("kind").and_then(Value::as_str) == Some("witness_error"))
            .collect()
    } else {
        rows_field(
            row,
            &[
                "invalid_witnesses",
                "malformed_witnesses",
                "negative_witness",
                "bad_witnesses",
            ],
        )
        .iter()
        .collect()
    };
    for bad in bad_witnesses {
        let result = if let Some(spins) = bad.get("spins") {
            codec
                .encode(&serde_json::from_value::<Vec<i8>>(spins.clone())?)
                .map(|_| ())
        } else {
            codec
                .decode(&unhex(field(
                    bad,
                    &["compact", "compact_hex", "hex", "data", "bytes"],
                )?)?)
                .map(|_| ())
        };
        assert_eq!(
            result.err().ok_or("accepted invalid witness")?.name(),
            field(bad, &["error"])?
        );
    }
    Ok(())
}
