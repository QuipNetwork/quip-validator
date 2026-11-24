"""QUIP HD golden parity for the Python signer (WP2 batch 1, design §6).

Checks the ``signer`` cases of the parity file that pqhybridsign, the SDK fork,
transaction-crypto-core and the wasm signer also check, vendored at
``crates/transaction-crypto-core/tests/vectors/quip_hd_parity.json``. The
signers derive H4 only.
"""

import hashlib
import json
from pathlib import Path

import pytest

import quip_signer

PARITY = json.loads(
    (
        Path(__file__).resolve().parents[2]
        / "transaction-crypto-core"
        / "tests"
        / "vectors"
        / "quip_hd_parity.json"
    ).read_text()
)

SIGNER_POSITIVES = [c for c in PARITY["positive"] if "signer" in c["scope"]]
SIGNER_NEGATIVES = [c for c in PARITY["negative"] if "signer" in c["rejected_by"]]
SDK_ONLY = [
    c
    for c in PARITY["positive"]
    if "signer" not in c["scope"] and "sdk" in c["scope"]
]


def test_the_parity_file_has_signer_cases() -> None:
    # Floors catch a filter that silently matches nothing.
    assert len(SIGNER_POSITIVES) >= 9
    assert len(SIGNER_NEGATIVES) >= 26
    assert len(SDK_ONLY) >= 15


@pytest.mark.parametrize("case", SIGNER_POSITIVES, ids=lambda c: c["name"])
def test_signer_positive_gives_the_h4_seed_and_public_key(case) -> None:
    key = case["keys"]["H4"]
    seed = quip_signer.seed_from_mnemonic(case["suri"])
    assert seed.hex() == key["seed"]
    public = quip_signer.public_from_seed(seed)
    assert hashlib.sha256(public).hexdigest() == key["public_sha256"]
    if "public" in key:
        assert public.hex() == key["public"]


@pytest.mark.parametrize("case", SIGNER_NEGATIVES, ids=lambda c: c["name"])
def test_signer_negative_is_refused(case) -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.seed_from_mnemonic(case["suri"])


@pytest.mark.parametrize("case", SDK_ONLY, ids=lambda c: c["name"])
def test_sdk_only_positive_is_refused_by_the_signer(case) -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.seed_from_mnemonic(case["suri"])
