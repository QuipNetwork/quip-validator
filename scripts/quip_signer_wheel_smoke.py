#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Import + sign/verify smoke test for a built `quip-signer` wheel (QUI-793).

Run by `scripts/python-dists.sh smoke` against the wheel installed into a
throwaway venv (repo source NOT on the path), so it exercises the artifact a
`pip install quip-signer` user actually gets -- unlike the pytest suite, which
runs against `maturin develop` (the editable source tree). It proves the wheel
imports and runs from a clean install, catching what the build-side content
guard can't: a broken extension, an ABI/symbol mismatch, a bad re-export.

Asserts the exact public surface downstream consumers pin (the six free
functions, `HybridSigner`, and the documented byte lengths). Exits non-zero on
any failure so the CI job fails.
"""

from quip_signer import (
    HybridSigner,
    account_id_from_public,
    public_from_seed,
    seed_from_mnemonic,
    sign_payload_from_seed,
    verify_envelope,
)

payload = b"quip-signer wheel smoke test"

# Object surface: mnemonic -> sign -> verify, and the documented byte lengths.
signer = HybridSigner.from_mnemonic(
    "bottom drive obey lake curtain smoke basket hold race lonely fit walk"
)
assert len(signer.public_key) == 929, "unexpected public key length"
assert len(signer.account_id) == 32, "unexpected account id length"
assert verify_envelope(payload, signer.sign(payload), signer.account_id), \
    "object-path sign/verify failed"

# Free-function surface: seed -> public -> account, and a seed-based round-trip.
seed = seed_from_mnemonic("0x" + "07" * 32)
account = account_id_from_public(public_from_seed(seed))
assert account == HybridSigner.from_seed(seed).account_id, "account derivation mismatch"
assert verify_envelope(payload, sign_payload_from_seed(seed, payload), account), \
    "free-function sign/verify failed"

print("quip-signer wheel smoke test: import + sign/verify OK")
