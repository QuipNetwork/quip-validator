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
        cases.update({f"missing-{name}": valid.replace(f"pallet_vesting, {name}\n", "") for name in sorted(NAMES)})
        cases["unexpected-name"] = valid + "pallet_vesting, new_upstream_path\n"
        cases["duplicate-name"] = valid + "pallet_vesting, vest_locked\n"
        cases["format-indented-pallet"] = valid.replace("pallet_vesting, ", " pallet_vesting, ", 1)
        cases["format-quoted-pallet"] = valid.replace("pallet_vesting, ", '"pallet_vesting", ', 1)
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
    print(Path(os.environ["INVENTORY_FIXTURE"]).read_text(), end="")
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
            for driver in DRIVERS:
                for case, inventory in cases.items():
                    with self.subTest(driver=driver, case=case):
                        fixture.write_text(inventory)
                        marker.unlink(missing_ok=True)
                        env["FAIL_LIST"] = "1" if case == "failed-list-command" else "0"
                        result = subprocess.run(
                            ["bash", str(root / "scripts" / driver)], cwd=root, env=env,
                            capture_output=True, text=True, check=False,
                        )
                        output = result.stdout + result.stderr
                        if case == "exact-inventory":
                            self.assertEqual(result.returncode, 73, output)
                            self.assertTrue(marker.exists(), output)
                            self.assertIn("Vesting inventory gate: PASS", output)
                        else:
                            self.assertNotEqual(result.returncode, 0, output)
                            self.assertFalse(marker.exists(), output)
                            if case.startswith("format-"):
                                self.assertIn("malformed pallet_vesting benchmark row", output)
                            elif case != "failed-list-command":
                                self.assertIn("expected exactly nine pallet_vesting benchmarks", output)
                        print(f"{driver}: {case}: PASS")


if __name__ == "__main__":
    unittest.main()
