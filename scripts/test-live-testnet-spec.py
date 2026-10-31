#!/usr/bin/env python3
"""Exercise byte-integrity gate failures in a disposable copy, never the checkout."""

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def main():
    source = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="live-testnet-spec-") as directory:
        root = Path(directory)
        (root / "scripts").mkdir()
        (root / "chain-specs").mkdir()
        checker = root / "scripts/check-live-testnet-spec.py"
        shutil.copyfile(source / "scripts/check-live-testnet-spec.py", checker)
        spec = root / "chain-specs/aglais-network.json"
        original = (source / spec.relative_to(root)).read_bytes()

        def check(label, expected_error=None):
            result = subprocess.run([sys.executable, str(checker)], capture_output=True, text=True)
            output = result.stdout + result.stderr
            if expected_error is None:
                assert result.returncode == 0, output
            else:
                assert result.returncode != 0 and expected_error in output, output
            print(f"{label}: exit {result.returncode}\n{output}", end="")

        spec.write_bytes(original)
        check("original")
        spec.write_bytes(original + b" ")
        check("one byte appended", "SHA-256 mismatch")
        equivalent = json.dumps(json.loads(original), separators=(",", ":")).encode()
        assert equivalent != original and json.loads(equivalent) == json.loads(original)
        spec.write_bytes(equivalent)
        check("semantic no-op reserialization", "SHA-256 mismatch")
        spec.unlink()
        check("missing file", "cannot read artifact")
        spec.write_bytes(original)
        check("restored original")


if __name__ == "__main__":
    main()
