#!/usr/bin/env python3
"""Reject incomplete or in-pallet-template vesting weight output before mirroring."""
import re
import sys
from pathlib import Path

NAMES = {
    "vest_locked", "vest_unlocked", "vest_other_locked", "vest_other_unlocked",
    "vested_transfer", "force_vested_transfer", "not_unlocking_merge_schedules",
    "unlocking_merge_schedules", "force_remove_vesting_schedule",
}


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
    if len(sys.argv) != 2:
        sys.exit("Usage: check-vesting-weights.py <generated-file>|--self-test")
    try:
        if sys.argv[1] == "--self-test":
            self_test()
        else:
            validate(Path(sys.argv[1]).read_text())
            print("Vesting generated-weight gate: PASS (nine runtime-style functions)")
    except (OSError, ValueError) as error:
        sys.exit(f"Vesting generated-weight gate: {error}")
