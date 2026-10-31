#!/usr/bin/env python3
"""Check the exact vesting benchmark inventory and generated weight signatures."""
import csv
import io
import re
import sys
from pathlib import Path

NAMES = {
    "vest_locked", "vest_unlocked", "vest_other_locked", "vest_other_unlocked",
    "vested_transfer", "force_vested_transfer", "not_unlocking_merge_schedules",
    "unlocking_merge_schedules", "force_remove_vesting_schedule",
}


def validate_inventory(text):
    names = []
    lines = text.splitlines()
    reader = csv.reader(io.StringIO(text), skipinitialspace=True)
    for row in reader:
        if row and row[0].strip() == "pallet_vesting":
            # Match the drivers' CSV format; otherwise their awk could skip a
            # row that a more permissive CSV parser accepted here.
            if len(row) != 2 or lines[reader.line_num - 1] != f"pallet_vesting, {row[1]}":
                raise ValueError("malformed pallet_vesting benchmark row")
            names.append(row[1].strip())
    if len(names) != len(NAMES) or set(names) != NAMES:
        raise ValueError(
            "expected exactly nine pallet_vesting benchmarks; "
            f"missing={sorted(NAMES - set(names))}, "
            f"unexpected={sorted(set(names) - NAMES)}, rows={len(names)}"
        )


def validate(text):
    if not re.search(r"impl\s*<[^>]+>\s+pallet_vesting::WeightInfo\s+for\s+WeightInfo<T>", text):
        raise ValueError("expected runtime-style pallet_vesting::WeightInfo implementation")
    methods = re.findall(r"\bfn\s+(\w+)\s*\(([^)]*)\)\s*->\s*Weight\s*\{", text)
    if len(methods) != len(NAMES) or {name for name, _ in methods} != NAMES:
        raise ValueError("expected exactly all nine vesting weight functions")
    for name, parameters in methods:
        if not re.fullmatch(r"\s*_?l\s*:\s*u32\s*,\s*_?s\s*:\s*u32\s*,?\s*", parameters):
            raise ValueError(f"{name}: expected (l: u32, s: u32)")


def self_test():
    inventory = "pallet, benchmark\nframe_system, remark\n" + "".join(
        f"pallet_vesting, {name}\n" for name in sorted(NAMES)
    )
    validate_inventory(inventory)
    invalid_inventories = ["frame_system, remark\n"]
    invalid_inventories += [inventory.replace(f"pallet_vesting, {name}\n", "") for name in sorted(NAMES)]
    invalid_inventories += [inventory + "pallet_vesting, unexpected\n", inventory + "pallet_vesting, vest_locked\n"]
    invalid_inventories += [inventory.replace("pallet_vesting, ", " pallet_vesting, ", 1)]
    for invalid in invalid_inventories:
        try:
            validate_inventory(invalid)
        except ValueError:
            continue
        raise AssertionError("invalid vesting benchmark inventory was accepted")
    print("Vesting inventory gate: exact inventory passes; absent/missing/extra/duplicate paths fail")
    valid = "impl<T: frame_system::Config> pallet_vesting::WeightInfo for WeightInfo<T> {\n"
    valid += "\n".join(f"fn {name}(l: u32, s: u32) -> Weight {{ Weight::zero() }}" for name in sorted(NAMES)) + "\n}"
    validate(valid)
    mutations = [valid.replace(name, "missing_" + name) for name in sorted(NAMES)]
    mutations += [valid.replace("s: u32", "s: u64", 1), valid.replace("pallet_vesting::WeightInfo", "WeightInfo")]
    for invalid in mutations:
        try:
            validate(invalid)
        except ValueError:
            continue
        raise AssertionError("invalid generated weight fixture was accepted")
    print("Vesting generated-weight gate: valid output passes; missing paths, wrong types/template fail")


if __name__ == "__main__":
    try:
        if len(sys.argv) == 3 and sys.argv[1] == "--inventory":
            validate_inventory(Path(sys.argv[2]).read_text())
            print("Vesting inventory gate: PASS (exactly nine benchmarks)")
        elif sys.argv[1:] == ["--self-test"]:
            self_test()
        elif len(sys.argv) == 2:
            validate(Path(sys.argv[1]).read_text())
            print("Vesting generated-weight gate: PASS (nine runtime-style functions)")
        else:
            sys.exit("Usage: check-vesting-weights.py <generated-file>|--self-test|--inventory <csv-file>")
    except (OSError, ValueError) as error:
        sys.exit(f"Vesting generated-weight gate: {error}")
