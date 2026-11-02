#!/usr/bin/env bash
# Negative fixtures run in a disposable copy; the working trees are never mutated.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$repo_root" <<'PY'
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1])
with tempfile.TemporaryDirectory(prefix="runtime-parity-") as temporary:
    root = Path(temporary)
    for tree in ("runtime", "runtime-testnet"):
        shutil.copytree(source / tree, root / tree)
    (root / "scripts").mkdir()
    shutil.copy2(source / "scripts/check-runtime-parity.sh", root / "scripts")

    def check(label, expected_error=None):
        result = subprocess.run(
            ["bash", "scripts/check-runtime-parity.sh"], cwd=root,
            capture_output=True, text=True,
        )
        output = result.stdout + result.stderr
        if expected_error is None:
            assert result.returncode == 0, output
        else:
            assert result.returncode != 0 and expected_error in output, output
        print(f"{label}: exit {result.returncode}\n{output}", end="")

    def edit(label, relative, replacement, expected_error):
        path = root / relative
        original = path.read_bytes()
        try:
            if replacement is None:
                path.unlink()
            else:
                path.write_bytes(replacement(original))
            check(label, expected_error)
        finally:
            path.write_bytes(original)

    check("baseline")
    edit("spec_name mismatch", "runtime-testnet/src/lib.rs",
         lambda data: data.replace(b'Cow::Borrowed("quip-testnet")', b'Cow::Borrowed("quip")'),
         "src/lib.rs: expected exactly one approved value, found 0")
    for tree in ("runtime", "runtime-testnet"):
        path, actual = root / tree, root / f"{tree}-actual"
        path.rename(actual)
        try:
            path.symlink_to(actual, target_is_directory=True)
            check(f"{tree} root symlink", "unexpected tree-root symlink:")
        finally:
            path.unlink()
            actual.rename(path)
    edit("one-sided source edit", "runtime/src/lib.rs",
         lambda data: data + b"\n// parity negative fixture\n",
         "unexpected difference in src/lib.rs")
    edit("missing file", "runtime-testnet/src/weights/mod.rs", None,
         "src/weights/mod.rs exists only in runtime")
    check("restored baseline")
PY
