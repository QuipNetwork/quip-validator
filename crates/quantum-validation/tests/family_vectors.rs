//! Canonical family vectors from quip-solver-core, checked against the
//! validator's regeneration and witness checks.
//!
//! The fixtures under `tests/fixtures/solver-core/` are byte-identical copies
//! of the solver-core conformance vectors at the commit in `SOURCE_REV`.
//! `scripts/sync-family-vectors.sh` writes them. Each family file is tied to
//! the hand-maintained solver-core registry by the BLAKE3 pin of its cases.

use ising_generators::blob;
use quantum_validation::families::{
    instance_energy, AdvertisedLimits, Code, FamilyChallenge, FamilyError, Instance, CONTRACTS,
};
use quantum_validation::{derive_nonce, generate_ising_model, AllowedValueSpec, ValidationError};
use rand_chacha::ChaCha8Rng as RandChaCha8;
use rand_core::{RngCore, SeedableRng};
use serde_json::Value;

const MANIFEST: &str = include_str!("../Cargo.toml");
const LOCKFILE: &str = include_str!("../../../Cargo.lock");
const SOLVER_CORE_GIT: &str = "git+https://gitlab.com/quip.network/quip-solver-core.git";
const SOURCE_REV: &str = include_str!("fixtures/solver-core/SOURCE_REV");
const REGISTRY: &str = include_str!("fixtures/solver-core/families/registry.json");
const GOLDEN_VECTORS: &str = include_str!("fixtures/solver-core/golden_vectors.json");
const GOLDEN_LEASE: &str = include_str!("fixtures/solver-core/golden_lease.json");

/// The family vector files, by their registry file name.
const FAMILY_FILES: &[(&str, &str)] = &[
    (
        "ice_structure.json",
        include_str!("fixtures/solver-core/families/ice_structure.json"),
    ),
    (
        "twisted_dimer.json",
        include_str!("fixtures/solver-core/families/twisted_dimer.json"),
    ),
    (
        "ice_charge.json",
        include_str!("fixtures/solver-core/families/ice_charge.json"),
    ),
    (
        "ice_fields.json",
        include_str!("fixtures/solver-core/families/ice_fields.json"),
    ),
    (
        "two_commodity.json",
        include_str!("fixtures/solver-core/families/two_commodity.json"),
    ),
    (
        "polariton.json",
        include_str!("fixtures/solver-core/families/polariton.json"),
    ),
    (
        "defects.json",
        include_str!("fixtures/solver-core/families/defects.json"),
    ),
    (
        "flow.json",
        include_str!("fixtures/solver-core/families/flow.json"),
    ),
];

/// The topology files that the family cases and admission vectors reference.
const TOPOLOGY_FILES: &[(&str, &[u8])] = &[
    (
        "error-degree-above-limit.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/error-degree-above-limit.bin.gz"),
    ),
    (
        "ice-fields-self-loop-and-degree.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/ice-fields-self-loop-and-degree.bin.gz"),
    ),
    (
        "ice-fields-shortfall-mixed.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/ice-fields-shortfall-mixed.bin.gz"),
    ),
    (
        "ice-fields-shortfall.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/ice-fields-shortfall.bin.gz"),
    ),
    (
        "polariton-dead-cell.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/polariton-dead-cell.bin.gz"),
    ),
    (
        "polariton-l3-permuted.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/polariton-l3-permuted.bin.gz"),
    ),
    (
        "polariton-no-window.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/polariton-no-window.bin.gz"),
    ),
    (
        "polariton-retry.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/polariton-retry.bin.gz"),
    ),
    (
        "routing-no-arc.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-no-arc.bin.gz"),
    ),
    (
        "routing-no-instance.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-no-instance.bin.gz"),
    ),
    (
        "routing-no-pairs.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-no-pairs.bin.gz"),
    ),
    (
        "routing-self-loop.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-self-loop.bin.gz"),
    ),
    (
        "routing-small-permuted.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-small-permuted.bin.gz"),
    ),
    (
        "routing-small.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-small.bin.gz"),
    ),
    (
        "routing-z28.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/routing-z28.bin.gz"),
    ),
    (
        "twisted-dimer-adjacency-z22-sparse.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/twisted-dimer-adjacency-z22-sparse.bin.gz"),
    ),
    (
        "z12-zephyr.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/z12-zephyr.bin.gz"),
    ),
    (
        "z22-cut-edge-zephyr.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/z22-cut-edge-zephyr.bin.gz"),
    ),
    (
        "z22-permuted-order-zephyr.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/z22-permuted-order-zephyr.bin.gz"),
    ),
    (
        "z22-zephyr.bin.gz",
        include_bytes!("fixtures/solver-core/topologies/z22-zephyr.bin.gz"),
    ),
];

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is valid JSON")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    field(value, key)
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

/// An unsigned value stored as a JSON integer or as a decimal string.
fn uint(value: &Value, key: &str) -> u64 {
    match field(value, key) {
        Value::String(s) => s.parse().unwrap_or_else(|_| panic!("{key}: {s}")),
        other => other
            .as_u64()
            .unwrap_or_else(|| panic!("{key} is not unsigned")),
    }
}

/// A signed value stored as a JSON integer or as a decimal string.
fn int(value: &Value, key: &str) -> i64 {
    match field(value, key) {
        Value::String(s) => s.parse().unwrap_or_else(|_| panic!("{key}: {s}")),
        other => other
            .as_i64()
            .unwrap_or_else(|| panic!("{key} is not signed")),
    }
}

fn bytes(value: &Value, key: &str) -> Vec<u8> {
    hex::decode(text(value, key)).unwrap_or_else(|_| panic!("{key} is not hex"))
}

fn hex32(value: &Value, key: &str) -> [u8; 32] {
    bytes(value, key)
        .try_into()
        .unwrap_or_else(|_| panic!("{key} is not 32 bytes"))
}

fn i32_list(value: &Value, key: &str) -> Vec<i32> {
    array(value, key)
        .iter()
        .map(|v| i32::try_from(v.as_i64().expect("integer")).expect("fits i32"))
        .collect()
}

fn spins(value: &Value, key: &str) -> Vec<i8> {
    array(value, key)
        .iter()
        .map(|v| i8::try_from(v.as_i64().expect("integer")).expect("fits i8"))
        .collect()
}

fn family_files() -> Vec<(&'static str, Value)> {
    FAMILY_FILES
        .iter()
        .map(|(name, body)| (*name, parse(body)))
        .collect()
}

/// The topology that a vector `topology` object names, in stored order.
fn topology(reference: &Value) -> blob::Topology {
    let path = text(reference, "topology");
    let file = path
        .strip_prefix("../topologies/")
        .unwrap_or_else(|| panic!("unexpected topology path {path}"));
    let (_, data) = TOPOLOGY_FILES
        .iter()
        .find(|(name, _)| *name == file)
        .unwrap_or_else(|| panic!("topology {file} is not vendored"));
    blob::decode(data)
        .and_then(|body| body.topology())
        .unwrap_or_else(|e| panic!("{file}: {e:?}"))
}

/// The nonce of salt `index` of a vector lease, derived the way the pallet
/// derives it: the base salt with a little-endian counter in bytes 0 to 7.
fn lease_nonce(lease: &Value, index: u64) -> [u8; 32] {
    let mut salt = hex32(lease, "base_salt");
    let counter = uint(lease, "salt_start")
        .checked_add(index)
        .expect("counter fits u64");
    salt[..8].copy_from_slice(&counter.to_le_bytes());
    derive_nonce(
        &hex32(lease, "last_proof_block_hash"),
        &hex32(lease, "miner_account"),
        &salt,
    )
    .to_big_endian()
}

/// Admit a vector case with no advertised limits.
fn admit<'a>(case: &'a Value, params: &'a [u8], topo: &blob::Topology) -> FamilyChallenge<'a> {
    FamilyChallenge::admit(
        CONTRACTS,
        text(case, "family_id").as_bytes(),
        u32::try_from(uint(case, "revision")).expect("revision fits u32"),
        params,
        &topo.nodes,
        &topo.edges,
        &AdvertisedLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{}: {e:?}", text(case, "name")))
}

/// Canonical JSON, as the solver-core registry hashes it: sorted keys, no
/// whitespace, integers only.
fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            assert!(number.is_i64() || number.is_u64(), "float {number}");
            out.push_str(&number.to_string());
        }
        Value::String(s) => {
            assert!(s.is_ascii(), "non-ASCII string {s:?}");
            out.push_str(&serde_json::to_string(s).expect("string encodes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(&Value::String(key.clone()), out);
                out.push(':');
                canonical_json(item, out);
            }
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

#[test]
fn fixtures_come_from_the_pinned_generator_revision() {
    let rev = SOURCE_REV.trim();
    assert_eq!(rev.len(), 40, "SOURCE_REV holds a full commit id");
    let pins: Vec<&str> = MANIFEST
        .lines()
        .filter(|line| line.starts_with("ising-generators = "))
        .collect();
    assert_eq!(pins.len(), 2, "one dependency and one dev-dependency");
    let tag = pins[0]
        .split_once("tag = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(tag, _)| tag)
        .expect("ising-generators is pinned to a release tag");
    for line in pins {
        assert!(
            line.contains(&format!("tag = \"{tag}\"")),
            "Cargo.toml pins disagree on the ising-generators tag: {line}"
        );
    }
    let source = format!("source = \"{SOLVER_CORE_GIT}?tag={tag}#{rev}\"");
    assert!(
        LOCKFILE.lines().any(|line| line == source),
        "Cargo.lock does not resolve ising-generators {tag} to the SOURCE_REV commit {rev}"
    );
}

#[test]
fn every_contract_has_a_research_registry_entry_with_pinned_vectors() {
    let registry = parse(REGISTRY);
    let files = family_files();
    let entries = array(&registry, "registry");
    for contract in CONTRACTS {
        let name = format!("{}@{}", contract.family_id, contract.revision);
        let entry = entries
            .iter()
            .find(|e| text(e, "name") == name)
            .unwrap_or_else(|| panic!("{name}: no registry entry"));
        // Inclusion in CONTRACTS never activates a family (FAM-76, FAM-79).
        assert_eq!(text(entry, "status"), "research", "{name}");

        let vectors = text(entry, "vectors");
        let file = vectors.rsplit('/').next().expect("file name");
        let (_, doc) = files
            .iter()
            .find(|(n, _)| *n == file)
            .unwrap_or_else(|| panic!("{name}: {file} is not vendored"));
        let cases = Value::Array(
            array(doc, "cases")
                .iter()
                .filter(|c| {
                    text(c, "family_id") == contract.family_id
                        && uint(c, "revision") == u64::from(contract.revision)
                })
                .cloned()
                .collect(),
        );
        let mut json = String::new();
        canonical_json(&cases, &mut json);
        assert_eq!(
            blake3::hash(json.as_bytes()).to_hex().as_str(),
            text(entry, "vectors_blake3"),
            "{name}: vendored cases differ from the registry pin"
        );
    }
    // Every registered quip family has a contract. The `vectors/` namespace
    // holds test constructions that never reach a chain.
    for entry in entries {
        let id = text(entry, "family_id");
        if id.starts_with("quip/") {
            assert!(
                CONTRACTS.iter().any(|c| c.family_id == id),
                "{id}: registered but not in CONTRACTS"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Regeneration
// ---------------------------------------------------------------------------

#[test]
fn every_family_case_regenerates_the_canonical_bytes() {
    let mut instances = 0;
    let mut skips = 0;
    for (file, doc) in family_files() {
        for case in array(&doc, "cases") {
            let name = text(case, "name");
            let topo = topology(field(case, "topology"));
            let params = bytes(case, "params_hex");
            let challenge = admit(case, &params, &topo);

            assert_eq!(
                challenge.descriptor_digest(),
                hex32(case, "descriptor_digest"),
                "{file} {name}: descriptor digest"
            );
            assert_eq!(
                challenge.topology_order_digest(),
                hex32(field(case, "topology"), "order_digest"),
                "{file} {name}: topology order digest"
            );
            let bounds = challenge.size_bounds();
            let want = field(case, "size_bounds");
            assert_eq!(
                (bounds.nodes, bounds.edges, bounds.degree),
                (
                    uint(want, "nodes"),
                    uint(want, "edges"),
                    uint(want, "degree")
                ),
                "{file} {name}: size bounds"
            );

            let lease = field(case, "lease");
            for entry in array(case, "instances") {
                let index = uint(entry, "index");
                let nonce = lease_nonce(lease, index);
                assert_eq!(
                    nonce,
                    hex32(entry, "nonce"),
                    "{file} {name} #{index}: nonce"
                );
                let instance = challenge
                    .regenerate(nonce)
                    .unwrap_or_else(|e| panic!("{file} {name} #{index}: {e:?}"));
                let canonical = instance.canonical_bytes().expect("allocation");
                assert!(
                    canonical == bytes(entry, "canonical_hex"),
                    "{file} {name} #{index}: canonical bytes differ"
                );
                assert_eq!(
                    instance.digest(),
                    hex32(entry, "instance_digest"),
                    "{file} {name} #{index}: instance digest"
                );
                let all_up = vec![1_i8; instance.node_ids().len()];
                assert_eq!(
                    instance_energy(&instance, &all_up),
                    Ok(int(entry, "energy_all_up_milli")),
                    "{file} {name} #{index}: all-up energy"
                );
                instances += 1;
            }
            for entry in array(case, "skipped") {
                let index = uint(entry, "index");
                let nonce = lease_nonce(lease, index);
                assert_eq!(
                    nonce,
                    hex32(entry, "nonce"),
                    "{file} {name} #{index}: nonce"
                );
                let code = u8::try_from(uint(entry, "skip_code")).expect("code fits u8");
                match challenge.regenerate(nonce) {
                    Err(FamilyError::Skipped(got)) => {
                        assert_eq!(got.value(), code, "{file} {name} #{index}: skip code");
                    }
                    other => panic!("{file} {name} #{index}: want skip {code}, got {other:?}"),
                }
                skips += 1;
            }
        }
    }
    // Guard against a vacuous pass after a fixture change.
    assert!(instances >= 90, "only {instances} instances checked");
    assert!(skips >= 20, "only {skips} skipped salts checked");
}

#[test]
fn admission_vectors_give_their_expected_codes() {
    let mut checked = 0;
    for (file, doc) in family_files() {
        let Some(requests) = doc.get("admission").and_then(Value::as_array) else {
            continue;
        };
        for request in requests {
            let name = text(request, "name");
            let topo = topology(field(request, "topology"));
            let params = bytes(request, "params_hex");
            let got = FamilyChallenge::admit(
                CONTRACTS,
                text(request, "family_id").as_bytes(),
                u32::try_from(uint(request, "revision")).expect("revision fits u32"),
                &params,
                &topo.nodes,
                &topo.edges,
                &AdvertisedLimits::default(),
            );
            let expected = field(request, "expected_code").as_u64();
            match (expected, got) {
                (None, Ok(_)) => {}
                (Some(code), Err(FamilyError::Rejected(got))) => {
                    assert_eq!(u64::from(got.value()), code, "{file} {name}");
                }
                (expected, got) => panic!("{file} {name}: want {expected:?}, got {got:?}"),
            }
            checked += 1;
        }
    }
    assert!(checked >= 20, "only {checked} admission vectors checked");
}

// ---------------------------------------------------------------------------
// Witnesses
// ---------------------------------------------------------------------------

/// The parameter bytes, topology, and body of the case named `name`.
fn case_named(doc: &Value, name: &str) -> (Vec<u8>, blob::Topology, Value) {
    let case = array(doc, "cases")
        .iter()
        .find(|c| text(c, "name") == name)
        .unwrap_or_else(|| panic!("no case {name}"));
    (
        bytes(case, "params_hex"),
        topology(field(case, "topology")),
        case.clone(),
    )
}

fn regenerate_case(case: &Value, params: &[u8], topo: &blob::Topology, index: u64) -> Instance {
    let challenge = admit(case, params, topo);
    challenge
        .regenerate(lease_nonce(field(case, "lease"), index))
        .unwrap_or_else(|e| panic!("{} #{index}: {e:?}", text(case, "name")))
}

#[test]
fn hard_family_witnesses_match_feasibility_and_energy() {
    let mut checked = 0;
    for (file, section) in [
        ("polariton.json", "witnesses"),
        ("defects.json", "defect_witnesses"),
        ("flow.json", "flow_witnesses"),
    ] {
        let (_, doc) = family_files()
            .into_iter()
            .find(|(n, _)| *n == file)
            .expect("vendored");
        for witness in array(&doc, section) {
            let name = text(witness, "name");
            let index = uint(witness, "index");
            let (params, topo, case) = case_named(&doc, text(witness, "case"));
            let challenge = admit(&case, &params, &topo);
            let instance = regenerate_case(&case, &params, &topo, index);
            let spins = spins(witness, "spins");
            let energy = int(witness, "energy_milli");

            assert_eq!(
                instance_energy(&instance, &spins),
                Ok(energy),
                "{file} {name}: encoded energy"
            );
            let feasible = field(witness, "feasible").as_bool().expect("bool");
            let want = if feasible {
                Ok(energy)
            } else {
                Err(FamilyError::Infeasible)
            };
            assert_eq!(
                challenge.verify_witness(&instance, &spins),
                want,
                "{file} {name}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 30, "only {checked} witnesses checked");
}

#[test]
fn charged_ice_energies_match_the_encoded_energy() {
    let (_, doc) = family_files()
        .into_iter()
        .find(|(n, _)| *n == "ice_charge.json")
        .expect("vendored");
    let mut checked = 0;
    for entry in array(&doc, "energies") {
        let index = uint(entry, "index");
        let (params, topo, case) = case_named(&doc, text(entry, "case"));
        let challenge = admit(&case, &params, &topo);
        let instance = regenerate_case(&case, &params, &topo, index);
        assert_eq!(
            *instance.nonce().expect("lease instance"),
            hex32(entry, "nonce")
        );
        for state in array(entry, "states") {
            let spins: Vec<i8> = bytes(state, "spins_hex")
                .into_iter()
                .map(|b| b as i8)
                .collect();
            // A soft family: every witness of the right length is feasible.
            assert_eq!(
                challenge.verify_witness(&instance, &spins),
                Ok(int(state, "encoded_milli")),
                "{} {}",
                text(entry, "name"),
                text(state, "name")
            );
            checked += 1;
        }
    }
    assert!(checked >= 40, "only {checked} states checked");
}

// ---------------------------------------------------------------------------
// Bounds and tampering
// ---------------------------------------------------------------------------

/// The first registered twisted-ice dimer case and its inputs.
fn twisted_case() -> (Vec<u8>, blob::Topology, Value) {
    let (_, doc) = family_files()
        .into_iter()
        .find(|(n, _)| *n == "twisted_dimer.json")
        .expect("vendored");
    case_named(&doc, "twisted-dimer-zephyr-z22")
}

#[test]
fn tampered_descriptors_and_tight_limits_are_rejected() {
    let (params, topo, case) = twisted_case();
    let id = text(&case, "family_id").as_bytes();
    let admit_with = |id: &[u8], revision: u32, params: &[u8], limits: AdvertisedLimits| {
        FamilyChallenge::admit(
            CONTRACTS,
            id,
            revision,
            params,
            &topo.nodes,
            &topo.edges,
            &limits,
        )
        .map(|c| c.descriptor_digest())
    };
    let none = AdvertisedLimits::default();

    assert_eq!(
        admit_with(b"quip/unknown-family", 1, &params, none),
        Err(FamilyError::Rejected(Code::FamilyUnsupported))
    );
    assert_eq!(
        admit_with(id, 2, &params, none),
        Err(FamilyError::Rejected(Code::RevisionUnsupported))
    );
    assert_eq!(
        admit_with(b"example/twisted-ice-dimer", 1, &params, none),
        Err(FamilyError::Rejected(Code::FamilyIdInvalid))
    );
    let mut truncated = params.clone();
    truncated.pop();
    assert_eq!(
        admit_with(id, 1, &truncated, none),
        Err(FamilyError::Rejected(Code::ParamsMalformed))
    );

    // A different but in-bounds parameter value is a different descriptor:
    // the challenge commits to the exact bytes.
    let original = admit_with(id, 1, &params, none).expect("admitted");
    let mut altered = params.clone();
    *altered.last_mut().expect("non-empty") ^= 1;
    if let Ok(digest) = admit_with(id, 1, &altered, none) {
        assert_ne!(digest, original, "altered parameters kept the digest");
    }

    // Verifier limits below the instance size refuse the challenge before
    // any generation work.
    let tight = AdvertisedLimits {
        max_nodes: 10,
        max_edges: 0,
        max_degree: 0,
    };
    assert_eq!(
        admit_with(id, 1, &params, tight),
        Err(FamilyError::Rejected(Code::BoundsExceeded))
    );
}

#[test]
fn topology_order_is_part_of_the_challenge() {
    let (params, topo, case) = twisted_case();
    let id = text(&case, "family_id").as_bytes();
    let nonce = lease_nonce(field(&case, "lease"), 0);
    let original = admit(&case, &params, &topo);

    let mut reversed = topo.edges.clone();
    reversed.reverse();
    let reordered = FamilyChallenge::admit(
        CONTRACTS,
        id,
        1,
        &params,
        &topo.nodes,
        &reversed,
        &AdvertisedLimits::default(),
    )
    .expect("same graph admits");
    assert_ne!(
        original.topology_order_digest(),
        reordered.topology_order_digest()
    );

    // An instance built over the reordered topology is not an instance of
    // the original challenge.
    let foreign = reordered.regenerate(nonce).expect("builds");
    let spins = vec![1_i8; foreign.node_ids().len()];
    assert_eq!(
        original.verify_witness(&foreign, &spins),
        Err(FamilyError::ForeignInstance)
    );

    let instance = original.regenerate(nonce).expect("builds");
    let n = instance.node_ids().len();
    assert_eq!(
        original.verify_witness(&instance, &vec![1; n - 1]),
        Err(FamilyError::WitnessLength {
            expected: n,
            actual: n - 1
        })
    );
    let mut bad = vec![1_i8; n];
    bad[3] = 0;
    assert_eq!(
        original.verify_witness(&instance, &bad),
        Err(FamilyError::InvalidSpin { index: 3, value: 0 })
    );
}

// ---------------------------------------------------------------------------
// Legacy draws
// ---------------------------------------------------------------------------

/// The `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1` seed of `golden_vectors.json`
/// `chacha8` case 0.
const VECTOR_SEED: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];

#[test]
fn legacy_stream_and_nonce_match_the_canonical_stream() {
    let golden = parse(GOLDEN_VECTORS);
    for case in array(&golden, "chacha8") {
        let seed = hex32(case, "seed_hex");
        let words: Vec<u32> = array(case, "words")
            .iter()
            .map(|w| u32::try_from(w.as_u64().expect("u64")).expect("u32"))
            .collect();
        let mut legacy = RandChaCha8::from_seed(seed);
        let mut canonical = ising_generators::rng::ChaCha8Rng::from_seed(seed);
        for (i, want) in words.iter().enumerate() {
            assert_eq!(legacy.next_u32(), *want, "rand_chacha word {i}");
            assert_eq!(canonical.next_u32(), *want, "canonical word {i}");
        }
        // Past the golden words, the two streams stay identical.
        for i in 0..4096 {
            assert_eq!(legacy.next_u32(), canonical.next_u32(), "word {i}");
        }
    }
    for case in array(&golden, "derive_nonce") {
        let nonce = derive_nonce(
            &hex32(case, "last_proof_hex"),
            &hex32(case, "miner_hex"),
            &hex32(case, "salt_hex"),
        );
        assert_eq!(nonce.to_big_endian(), hex32(case, "nonce_hex"));
    }
}

#[test]
fn legacy_set_draws_replay_bit_for_bit() {
    let golden = parse(GOLDEN_VECTORS);
    for case in array(&golden, "ising") {
        let nodes: Vec<u32> = i32_list(case, "nodes")
            .into_iter()
            .map(|n| u32::try_from(n).expect("node"))
            .collect();
        let edges: Vec<(u32, u32)> = array(case, "edges")
            .iter()
            .map(|e| {
                let pair = e.as_array().expect("edge");
                let end = |v: &Value| u32::try_from(v.as_u64().expect("end")).expect("u32");
                (end(&pair[0]), end(&pair[1]))
            })
            .collect();
        let allowed_h = i32_list(case, "allowed_h_milli");
        let allowed_j = i32_list(case, "allowed_j_milli");
        let nonce = sp_core::U256::from_big_endian(&hex32(case, "nonce_hex"));
        let (h, j) = generate_ising_model(
            nonce,
            &nodes,
            &edges,
            &AllowedValueSpec::Set(allowed_h.as_slice()),
            &AllowedValueSpec::Set(allowed_j.as_slice()),
        )
        .expect("draws");
        assert_eq!(h, i32_list(case, "h_milli"));
        assert_eq!(j, i32_list(case, "j_milli"));
    }

    // The lease cases pin the full little-endian bytes of every draw, on
    // topologies up to the Advantage2 counts.
    let lease = parse(GOLDEN_LEASE);
    let le_hash = |values: &[i32]| {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        blake3::hash(&bytes).to_hex().to_string()
    };
    for case in array(&lease, "cases") {
        let name = text(case, "name");
        let n = u32::try_from(uint(case, "n_nodes")).expect("u32");
        let nodes: Vec<u32> = (0..n).collect();
        let edges: Vec<(u32, u32)> = (0..u32::try_from(uint(case, "n_edges")).expect("u32"))
            .map(|k| (k % n, (k + 1) % n))
            .collect();
        let allowed_h = i32_list(case, "allowed_h_milli");
        let allowed_j = i32_list(case, "allowed_j_milli");
        for (index, salt) in array(case, "salts").iter().enumerate() {
            let nonce = lease_nonce(case, u64::try_from(index).expect("index"));
            assert_eq!(nonce, hex32(salt, "nonce"), "{name} #{index}: nonce");
            let (h, j) = generate_ising_model(
                sp_core::U256::from_big_endian(&nonce),
                &nodes,
                &edges,
                &AllowedValueSpec::Set(allowed_h.as_slice()),
                &AllowedValueSpec::Set(allowed_j.as_slice()),
            )
            .expect("draws");
            assert_eq!(le_hash(&h), text(salt, "h_blake3"), "{name} #{index}: h");
            assert_eq!(le_hash(&j), text(salt, "j_blake3"), "{name} #{index}: j");
        }
    }
}

/// Draw `count` values from `spec` with the production sampler.
fn production_draw(
    spec: &AllowedValueSpec<&[i32]>,
    count: usize,
) -> Vec<Result<i32, ValidationError>> {
    let mut rng = RandChaCha8::from_seed(VECTOR_SEED);
    (0..count).map(|_| spec.sample(&mut rng)).collect()
}

/// The same draw on the canonical stream: `min + word % span`.
fn canonical_range_draw(min: i64, max: i64, scale: i64, count: usize) -> Vec<Option<i32>> {
    let mut rng = ising_generators::rng::ChaCha8Rng::from_seed(VECTOR_SEED);
    let span = u64::try_from(max - min + 1).expect("non-empty range");
    (0..count)
        .map(|_| {
            let offset = i64::try_from(u64::from(rng.next_u32()) % span).expect("fits");
            i32::try_from((min + offset) * scale).ok()
        })
        .collect()
}

#[test]
fn legacy_integer_range_keeps_its_semantics() {
    // Pins of the validator output at commit 1f8a1c0, recorded in
    // ising-generators tests/random_parity.rs.
    let spec = AllowedValueSpec::IntegerRange { min: -6, max: 6 };
    let got: Vec<i32> = production_draw(&spec, 4)
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(got, [1000, 1000, -2000, -3000]);

    let drawn: Vec<Option<i32>> = production_draw(&spec, 2048)
        .into_iter()
        .map(Result::ok)
        .collect();
    assert_eq!(drawn, canonical_range_draw(-6, 6, 1000, 2048));

    // A range whose top value overflows i32 after scaling.
    let overflow = AllowedValueSpec::IntegerRange {
        min: 2_147_483,
        max: 2_147_484,
    };
    assert_eq!(
        production_draw(&overflow, 4),
        [
            Ok(2_147_483_000),
            Ok(2_147_483_000),
            Err(ValidationError::ArithmeticOverflow),
            Err(ValidationError::ArithmeticOverflow),
        ]
    );
}

#[test]
fn legacy_continuous_range_keeps_its_semantics() {
    let spec = AllowedValueSpec::ContinuousRange {
        min: -1500,
        max: 1500,
    };
    let got: Vec<i32> = production_draw(&spec, 4)
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(got, [-100, -563, 1466, 118]);

    let drawn: Vec<Option<i32>> = production_draw(&spec, 2048)
        .into_iter()
        .map(Result::ok)
        .collect();
    assert_eq!(drawn, canonical_range_draw(-1500, 1500, 1, 2048));

    // The full i32 span: the offset is the raw word.
    let full = AllowedValueSpec::ContinuousRange {
        min: i32::MIN,
        max: i32::MAX,
    };
    assert_eq!(
        production_draw(&full, 4),
        [
            Ok(263_329_088),
            Ok(-357_887_378),
            Ok(-77_141_799),
            Ok(-1_290_615_503)
        ]
    );
}
