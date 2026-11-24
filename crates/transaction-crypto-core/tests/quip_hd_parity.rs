//! The signer layer of the QUIP HD golden parity file (WP2 batch 1 design
//! §6).
//!
//! `vectors/quip_hd_parity.json` is pqhybridsign's parity file, vendored
//! unchanged from `grego/audit-wp2/quip-hd` 0c8eaa81
//! (`crates/pqhybridsign/tests/vectors/quip_hd_parity.json`, sha256
//! e9af023dd0df9c17a28ef7659f87c8dd3db77b68211126ac33c7e7ffff4effa2), the same
//! bytes the SDK fork vendors. Every case whose `scope` or `rejected_by`
//! names `signer` is this crate's; the signer derives H4 only.

use quip_transaction_crypto_core::{master_seed_from_secret_uri, public_key_from_seed};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PARITY: &str = include_str!("vectors/quip_hd_parity.json");
const PARITY_SHA256: &str = "e9af023dd0df9c17a28ef7659f87c8dd3db77b68211126ac33c7e7ffff4effa2";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn names(case: &Value, field: &str) -> Vec<String> {
    case[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field}"))
        .iter()
        .map(|layer| layer.as_str().expect("layer").to_owned())
        .collect()
}

#[test]
fn the_vendored_parity_file_is_pqhybridsigns() {
    assert_eq!(hex(&Sha256::digest(PARITY.as_bytes())), PARITY_SHA256);
}

#[test]
fn signer_positives_give_the_h4_seed_and_public_key() {
    let file: Value = serde_json::from_str(PARITY).expect("parity file");
    let mut checked = 0;
    for case in file["positive"].as_array().expect("positive") {
        if !names(case, "scope").iter().any(|layer| layer == "signer") {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let suri = case["suri"].as_str().expect("suri");
        let key = &case["keys"]["H4"];
        let seed = master_seed_from_secret_uri(suri).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(hex(&seed), key["seed"].as_str().expect("seed"), "{name}");
        let public = public_key_from_seed(&seed).expect("public");
        assert_eq!(
            hex(&Sha256::digest(public)),
            key["public_sha256"].as_str().expect("public_sha256"),
            "{name}"
        );
        if let Some(full) = key["public"].as_str() {
            assert_eq!(hex(&public), full, "{name}");
        }
        checked += 1;
    }
    // 9 in the file today; the floor catches a filter that matches nothing.
    assert!(checked >= 9, "{checked} signer positives");
}

#[test]
fn signer_negatives_are_refused() {
    let file: Value = serde_json::from_str(PARITY).expect("parity file");
    let mut checked = 0;
    for case in file["negative"].as_array().expect("negative") {
        if !names(case, "rejected_by")
            .iter()
            .any(|layer| layer == "signer")
        {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let suri = case["suri"].as_str().expect("suri");
        assert!(
            master_seed_from_secret_uri(suri).is_err(),
            "{name} must be refused"
        );
        checked += 1;
    }
    // 26 in the file today, every negative names the signers.
    assert!(checked >= 26, "{checked} signer negatives");
}

/// A case that only the SDK accepts (junctions, the dev phrase) is a
/// signer negative or absent from the signer scope; none may slip through.
#[test]
fn sdk_only_positives_are_refused_by_the_signer() {
    let file: Value = serde_json::from_str(PARITY).expect("parity file");
    let mut checked = 0;
    for case in file["positive"].as_array().expect("positive") {
        let scope = names(case, "scope");
        if scope.iter().any(|layer| layer == "signer") || !scope.iter().any(|l| l == "sdk") {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let suri = case["suri"].as_str().expect("suri");
        assert!(
            master_seed_from_secret_uri(suri).is_err(),
            "{name}: sdk-only, the signer must refuse it"
        );
        checked += 1;
    }
    assert!(checked >= 10, "{checked} sdk-only positives");
}
