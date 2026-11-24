#!/usr/bin/env bash
# Regenerates every checked-in value that pins a phrase- or `//Alice`-derived
# hybrid key (QUIP HD v1, WP2 batch 1 V4), from the real implementations:
#
# - crates/transaction-crypto-core/tests/golden_vectors.txt, from the core's
#   `generate_golden_vectors` example;
# - the dev Alice/Bob seeds in the four js/quip-signer/test scripts, and the
#   Alice account id, revive address and spec version in
#   revive-fund-account.mjs, from the transaction-crypto crate's
#   `print_dev_accounts` example (the SDK fork's `Pair::from_string_with_seed`);
# - ALICE_PINNED_ACCOUNT_HEX in both runtimes' genesis_config_presets.rs;
# - both runtimes' genesis_legacy_hashes.json, from each runtime's own
#   `get_preset` in both feature profiles (production and runtime-benchmarks).
#
# Prerequisite: `npm ci --prefix js/quip-signer`, for @polkadot/util-crypto's
# keccak (the revive address). Re-running on an up-to-date tree changes
# nothing.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ ! -d js/quip-signer/node_modules/@polkadot/util-crypto ]]; then
  echo "run 'npm ci --prefix js/quip-signer' first" >&2
  exit 1
fi

echo "golden vectors" >&2
golden="$(cargo run -q --locked -p quip-transaction-crypto-core --example generate_golden_vectors)"
printf '%s\n' "$golden" >crates/transaction-crypto-core/tests/golden_vectors.txt

echo "dev accounts" >&2
accounts="$(cargo run -q --locked -p quip-transaction-crypto --example print_dev_accounts)"

hashes=""
for crate in quip-protocol-runtime quip-testnet-runtime; do
  for features in "" "runtime-benchmarks"; do
    echo "preset hashes: $crate ${features:-production}" >&2
    # The benchmark build compiles pallet-revive's contract fixtures, which
    # need solc and resolc. No preset embeds a fixture, so skipping them
    # cannot change a preset hash.
    out="$(QUIP_PRINT_LEGACY_HASHES=1 SKIP_PALLET_REVIVE_FIXTURES=1 \
      cargo test -q --release --locked -p "$crate" \
      ${features:+--features "$features"} --lib \
      genesis_config_presets::tests::legacy_preset_bytes_match_pre_launch_base \
      -- --exact --nocapture)"
    hashes+="$(grep '^legacy_hash ' <<<"$out" | sed "s/^/$crate /")"$'\n'
  done
done

ACCOUNTS="$accounts" HASHES="$hashes" python3 - <<'PY'
import json
import os
import re
import subprocess
from pathlib import Path

values = dict(line.split("=", 1) for line in os.environ["ACCOUNTS"].split())
alice_seed, bob_seed = values["alice_seed"], values["bob_seed"]
alice_account = values["alice_account"]


def rewrite(path, transform):
    path = Path(path)
    old = path.read_text()
    new = transform(old)
    if new != old:
        path.write_text(new)
        print(f"updated {path}")


def sub_once(pattern, replacement, text, label):
    text, count = re.subn(pattern, replacement, text, flags=re.MULTILINE)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text


# The JS seeds: read the current pins from local-node.mjs, replace them
# everywhere they appear.
local_node = Path("js/quip-signer/test/local-node.mjs").read_text()
old = {
    name: re.search(rf"name: 'Quip {name}',\s*seedHex: '(0x[0-9a-f]{{64}})'", local_node).group(1)
    for name in ("Alice", "Bob")
}
seed_files = [
    "js/quip-signer/test/local-node.mjs",
    "js/quip-signer/test/xqvm-onchain-smoke.mjs",
    "js/quip-signer/test/validator-rotation.mjs",
    "js/quip-signer/test/revive-fund-account.mjs",
]
for path in seed_files:
    rewrite(path, lambda t: t.replace(old["Alice"], alice_seed).replace(old["Bob"], bob_seed))

# revive-fund-account.mjs: Alice's account, its revive address, spec version.
revive_address = subprocess.run(
    [
        "node",
        "--input-type=module",
        "-e",
        "import { keccakAsU8a } from '@polkadot/util-crypto';"
        "import { hexToU8a, u8aToHex } from '@polkadot/util';"
        f"console.log(u8aToHex(keccakAsU8a(hexToU8a('{alice_account}'), 256).subarray(12)).toLowerCase());",
    ],
    cwd="js/quip-signer",
    check=True,
    capture_output=True,
    text=True,
).stdout.strip()
spec_version = re.search(r"^\s*spec_version: (\d+),", Path("runtime/src/lib.rs").read_text(), re.M).group(1)


def revive(text):
    text = sub_once(r"(export const ALICE_ACCOUNT_ID =\n  ')0x[0-9a-f]{64}(')", rf"\g<1>{alice_account}\g<2>", text, "ALICE_ACCOUNT_ID")
    text = sub_once(r"(export const ALICE_REVIVE_ADDRESS = ')0x[0-9a-f]{40}(')", rf"\g<1>{revive_address}\g<2>", text, "ALICE_REVIVE_ADDRESS")
    return sub_once(r"(const EXPECTED_SPEC_VERSION = )\d+", rf"\g<1>{spec_version}", text, "EXPECTED_SPEC_VERSION")


rewrite("js/quip-signer/test/revive-fund-account.mjs", revive)

# The runtimes: Alice's pinned account and the preset hashes.
hashes = {}
for line in os.environ["HASHES"].strip().splitlines():
    crate, _, profile, name, digest = line.split()
    hashes.setdefault(crate, {}).setdefault(profile, {})[name] = digest
for crate, tree in (("quip-protocol-runtime", "runtime"), ("quip-testnet-runtime", "runtime-testnet")):
    rewrite(
        f"{tree}/src/genesis_config_presets.rs",
        lambda t: sub_once(
            r'(const ALICE_PINNED_ACCOUNT_HEX: &str =\n\s*")[0-9a-f]{64}(";)',
            rf"\g<1>{alice_account[2:]}\g<2>",
            t,
            "ALICE_PINNED_ACCOUNT_HEX",
        ),
    )
    profiles = hashes[crate]
    if sorted(profiles) != ["benchmark", "production"] or any(len(p) != 5 for p in profiles.values()):
        raise SystemExit(f"{crate}: expected 5 hashes in each of two profiles, got {profiles}")
    body = {profile: dict(sorted(profiles[profile].items())) for profile in ("production", "benchmark")}
    rewrite(f"{tree}/src/genesis_legacy_hashes.json", lambda _t: json.dumps(body, indent=2) + "\n")
PY
