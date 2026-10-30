#!/usr/bin/env bash
# Compare complete runtime trees; only the approved network differences are allowed.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$repo_root" <<'PY'
import difflib
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
import_paths = {
    "tests/metadata_v16.rs",
    "tests/signing_fixture.rs",
    "tests/support/signing_fixture.rs",
}


def files(tree):
    if not tree.is_dir():
        raise ValueError(f"missing runtime tree: {tree}")
    result = {}
    for path in tree.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"unexpected symlink: {path}")
        if path.is_file():
            result[path.relative_to(tree).as_posix()] = path.read_bytes()
    return result


def replace_once(text, pattern, replacement, label):
    text, count = re.subn(pattern, replacement, text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"{label}: expected exactly one approved value, found {count}")
    return text


def normalize(path, data, testnet):
    if path not in import_paths | {"Cargo.toml", "src/network_params.rs"}:
        return data
    text = data.decode("utf-8")
    if path == "Cargo.toml":
        name = "quip-testnet-runtime" if testnet else "quip-protocol-runtime"
        text = replace_once(
            text, rf'\A\[package\]\nname = "{name}"$',
            '[package]\nname = "NETWORK_RUNTIME"', path,
        )
    elif path == "src/network_params.rs":
        for name, kind, value in [
            ("EPOCH_DURATION_SLOTS", "u64", "600" if testnet else "2_400"),
            ("MINIMUM_ELECTION_SLACK_BLOCKS", "u32", "300" if testnet else "1_200"),
        ]:
            text = replace_once(
                text, rf"^pub const {name}: {kind} = {value};$",
                f"pub const {name}: {kind} = NETWORK_VALUE;", f"{path}:{name}",
            )
    elif testnet:
        text, count = re.subn(r"\bquip_testnet_runtime\b", "quip_protocol_runtime", text)
        if count == 0:
            raise ValueError(f"{path}: missing testnet crate import")
    return text.encode("utf-8")


try:
    main, testnet = files(root / "runtime"), files(root / "runtime-testnet")
    failed = False
    for path in sorted(main.keys() ^ testnet.keys()):
        print(f"runtime parity: {path} exists only in {'runtime' if path in main else 'runtime-testnet'}")
        failed = True
    for path in sorted(main.keys() & testnet.keys()):
        left, right = normalize(path, main[path], False), normalize(path, testnet[path], True)
        if left != right:
            failed = True
            print(f"runtime parity: unexpected difference in {path}")
            print("".join(difflib.unified_diff(
                left.decode("utf-8", errors="replace").splitlines(keepends=True),
                right.decode("utf-8", errors="replace").splitlines(keepends=True),
                fromfile=f"runtime/{path}", tofile=f"runtime-testnet/{path}",
            )), end="")
    if failed:
        sys.exit(1)
except (OSError, UnicodeError, ValueError) as error:
    print(f"runtime parity: {error}", file=sys.stderr)
    sys.exit(1)
print(f"runtime parity: PASS ({len(main)} files; only approved network differences)")
PY
