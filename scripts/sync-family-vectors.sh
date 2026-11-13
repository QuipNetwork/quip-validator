#!/usr/bin/env bash
# Copy the canonical family vectors from quip-solver-core into the
# quantum-validation test fixtures, byte for byte.
#
# Usage: scripts/sync-family-vectors.sh <quip-solver-core checkout> <rev>
#
# <rev> must be the `tag` of `ising-generators` in
# crates/quantum-validation/Cargo.toml. The script records the commit that
# tag points to in SOURCE_REV, and the family_vectors test fails when that
# commit differs from the one Cargo.lock resolved the tag to.
set -euo pipefail

if [[ $# -ne 2 ]]; then
	echo "usage: $0 <quip-solver-core checkout> <rev>" >&2
	exit 2
fi
source_repo=$1
rev=$2
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/quantum-validation/tests/fixtures/solver-core"
vectors=crates/quip-solver-conformance/vectors

families=(registry ice_structure twisted_dimer ice_charge ice_fields two_commodity polariton defects flow)

full_rev="$(git -C "$source_repo" rev-parse --verify "$rev^{commit}")"
rm -rf "$dest"
mkdir -p "$dest/families/codecs" "$dest/topologies"

git -C "$source_repo" show "$full_rev:$vectors/golden_vectors.json" >"$dest/golden_vectors.json"
git -C "$source_repo" show "$full_rev:$vectors/golden_lease.json" >"$dest/golden_lease.json"
for name in "${families[@]}"; do
	git -C "$source_repo" show "$full_rev:$vectors/families/$name.json" >"$dest/families/$name.json"
done

for name in polariton defects flow twisted charge glass fields routing; do
	git -C "$source_repo" show "$full_rev:$vectors/families/codecs/$name.json" >"$dest/families/codecs/$name.json"
done

# Copy only the topology files that the family cases and admission vectors
# reference.
topologies="$(
	python3 -I - "$dest/families" "${families[@]:1}" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
names = set()
for family in sys.argv[2:]:
    doc = json.loads((root / f"{family}.json").read_text())
    for entry in doc["cases"] + doc.get("admission", []):
        names.add(Path(entry["topology"]["topology"]).name)
print("\n".join(sorted(names)))
PY
)"
while IFS= read -r topology; do
	git -C "$source_repo" show "$full_rev:$vectors/topologies/$topology" >"$dest/topologies/$topology"
done <<<"$topologies"

echo "$full_rev" >"$dest/SOURCE_REV"
echo "synced $(wc -l <<<"$topologies") topologies and ${#families[@]} family files from $full_rev"
