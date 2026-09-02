"""Behavioral tests for the Python signer API."""

import pytest

import quip_signer

TEST_PHRASE = "bottom drive obey lake curtain smoke basket hold race lonely fit walk"
HEX_SEED = "0x" + "07" * 32


def test_sign_verify_roundtrip() -> None:
    seed = quip_signer.seed_from_mnemonic(HEX_SEED)
    public = quip_signer.public_from_seed(seed)
    account = quip_signer.account_id_from_public(public)
    envelope = quip_signer.sign_payload_from_seed(seed, b"quip-message")

    assert len(envelope) == 1660
    assert quip_signer.verify_envelope(b"quip-message", envelope, account)
    assert not quip_signer.verify_envelope(b"wrong-message", envelope, account)


def test_account_id_is_32_bytes() -> None:
    public = quip_signer.public_from_seed(bytes([7]) * 32)
    account = quip_signer.account_id_from_public(public)
    assert isinstance(account, bytes)
    assert len(account) == 32


def test_hybrid_signer_class() -> None:
    signer = quip_signer.HybridSigner.from_mnemonic(TEST_PHRASE)

    assert len(signer.public_key) == 929
    assert len(signer.account_id) == 32

    envelope = signer.sign(b"quip-message")
    assert quip_signer.verify_envelope(b"quip-message", envelope, signer.account_id)

    # The class and the free functions agree.
    seed = quip_signer.seed_from_mnemonic(TEST_PHRASE)
    assert signer.public_key == quip_signer.public_from_seed(seed)
    assert signer.sign(b"x") == quip_signer.sign_payload_from_seed(seed, b"x")


def test_from_seed_requires_32_bytes() -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.HybridSigner.from_seed(bytes([1]) * 31)
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.public_from_seed(bytes([1]) * 31)


@pytest.mark.parametrize("length", [0, 31, 33, 64])
def test_from_seed_rejects_wrong_length(length: int) -> None:
    # `from_seed` must validate the seed length before retaining it; any length
    # other than 32 bytes (empty, short, long) raises rather than constructing a
    # signer over a malformed seed.
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.HybridSigner.from_seed(bytes([1]) * length)


def test_mnemonic_rejects_derivation_junctions() -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.seed_from_mnemonic(f"{TEST_PHRASE}//0")


def test_invalid_mnemonic_raises() -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.seed_from_mnemonic("not a real mnemonic phrase at all")


def test_verify_rejects_malformed_envelope() -> None:
    with pytest.raises(quip_signer.QuipSignerError):
        quip_signer.verify_envelope(b"payload", b"too-short", bytes(32))
