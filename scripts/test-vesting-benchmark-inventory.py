#!/usr/bin/env python3
"""Exercise both real drivers with CLI fixtures; no build or measurement occurs."""
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
NAMES = runpy.run_path(str(ROOT / "scripts/check-vesting-weights.py"))["NAMES"]
DRIVERS = ("run-benchmarks.sh", "check-benchmark-preflight.sh")


class InventoryGate(unittest.TestCase):
    def test_drivers_fail_before_measurement_on_incomplete_inventory(self):
        valid = "pallet, benchmark\nframe_system, remark\n" + "".join(
            f"pallet_vesting, {name}\n" for name in sorted(NAMES)
        )
        # The CLI lists other pallets normally when Vesting registration is removed.
        cases = {"removed-registration": "pallet, benchmark\nframe_system, remark\n"}
        # The gate compares a set against NAMES, so every missing name takes the same
        # path; the first and last names cover the boundaries.
        ordered = sorted(NAMES)
        cases.update({f"missing-{name}": valid.replace(f"pallet_vesting, {name}\n", "") for name in (ordered[0], ordered[-1])})
        cases["unexpected-name"] = valid + "pallet_vesting, new_upstream_path\n"
        cases["duplicate-name"] = valid + "pallet_vesting, vest_locked\n"
        cases["format-indented-pallet"] = valid.replace("pallet_vesting, ", " pallet_vesting, ", 1)
        cases["format-quoted-pallet"] = valid.replace("pallet_vesting, ", '"pallet_vesting", ', 1)
        cases["format-multiline-unexpected"] = valid + 'frame_system, "remark\npallet_vesting, unexpected\n"\n'
        cases["format-multiline-duplicate"] = valid + 'frame_system, "remark\npallet_vesting, vest_locked\n"\n'
        cases["format-unterminated-pallet-quote"] = valid + '"pallet_vesting, unexpected\n'
        cases["format-unterminated-multiline-quote"] = valid + 'frame_system, "remark\npallet_vesting, unexpected\n'
        cases["format-unterminated-benchmark-quote"] = valid + 'pallet_vesting, "unexpected\n'
        cases["format-extra-fields"] = valid + "frame_system, remark, extra\n"
        cases["format-quoted-other-pallet"] = valid + '"frame_system", remark\n'
        cases["format-indented-other-pallet"] = valid + " frame_system, remark\n"
        cases["format-uppercase-pallet"] = valid + "Frame_system, remark\n"
        cases["format-missing-delimiter-space"] = valid + "frame_system,remark\n"
        cases["format-tab-delimiter"] = valid + "frame_system,\tremark\n"
        cases["format-blank-row"] = valid + "\n"
        cases["format-carriage-return"] = valid.replace("\n", "\r\n")
        cases["format-non-lf-separator"] = valid + "frame_system, remark\vpallet_vesting, unexpected\n"
        cases["exact-cli-header"] = valid.replace("pallet, benchmark", "pallet, extrinsic")
        cases["exact-qualified-pallet"] = valid + (
            "pallet_election_provider_multi_block::unsigned, submit_unsigned\n"
            "pallet_election_provider_multi_block::unsigned, validate_unsigned\n"
        )
        cases["exact-inventory"] = valid
        cases["failed-list-command"] = valid
        with tempfile.TemporaryDirectory(prefix="quip-vesting-inventory-") as directory:
            root = Path(directory)
            for relative in [
                *("scripts/" + name for name in DRIVERS),
                "scripts/check-vesting-weights.py",
                "pallets/quantum-pow/src/weights.rs",
                "pallets/quantum-compute-mempool/src/weights.rs",
            ]:
                dest = root / relative
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / relative, dest)
            stub = root / "stub-bin"
            stub.mkdir()
            cargo = stub / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            for profile in ("debug", "release"):
                node = root / "target" / profile / "quip-network-node"
                node.parent.mkdir(parents=True)
                node.write_text('''#!/usr/bin/env python3
import os, sys
from pathlib import Path
if "--list" in sys.argv:
    sys.stdout.buffer.write(Path(os.environ["INVENTORY_FIXTURE"]).read_bytes())
    sys.exit(17 if os.environ["FAIL_LIST"] == "1" else 0)
Path(os.environ["MEASUREMENT_MARKER"]).write_text("measurement reached")
sys.exit(73)
''')
                node.chmod(0o755)
            env = os.environ.copy()
            env.pop("CARGO_TARGET_DIR", None)
            env.pop("SKIP_PALLETS", None)
            env["PATH"] = str(stub) + os.pathsep + env["PATH"]
            fixture = root / "inventory.csv"
            marker = root / "measurement"
            env.update(INVENTORY_FIXTURE=str(fixture), MEASUREMENT_MARKER=str(marker))
            # SKIP_PALLETS acts after the inventory gate, so only the gate-bypass case
            # and the passing case can tell a skipped run from an unskipped one.
            skip_cases = {"removed-registration", "exact-inventory"}
            for driver in DRIVERS:
                for skip in ("", "pallet_vesting"):
                    env["SKIP_PALLETS"] = skip
                    for case, inventory in cases.items():
                        if skip and case not in skip_cases:
                            continue
                        with self.subTest(driver=driver, skip=skip, case=case):
                            fixture.write_text(inventory)
                            marker.unlink(missing_ok=True)
                            env["FAIL_LIST"] = "1" if case == "failed-list-command" else "0"
                            result = subprocess.run(
                                ["bash", str(root / "scripts" / driver)], cwd=root, env=env,
                                capture_output=True, text=True, check=False,
                            )
                            output = result.stdout + result.stderr
                            if case.startswith("exact-"):
                                self.assertEqual(result.returncode, 73, output)
                                self.assertTrue(marker.exists(), output)
                                self.assertIn("Vesting inventory gate: PASS", output)
                                if case == "exact-qualified-pallet":
                                    if driver == "run-benchmarks.sh":
                                        self.assertIn("frame_system pallet_election_provider_multi_block::unsigned pallet_vesting", output)
                                    else:
                                        self.assertIn("Discovered 12 benchmarks across 3 pallets", output)
                            else:
                                self.assertNotEqual(result.returncode, 0, output)
                                self.assertFalse(marker.exists(), output)
                                if case.startswith("format-"):
                                    self.assertIn("malformed", output)
                                elif case != "failed-list-command":
                                    self.assertIn("expected exactly nine pallet_vesting benchmarks", output)
                            print(f"{driver}: skip={skip or 'none'}: {case}: PASS")


if __name__ == "__main__":
    unittest.main()
