# Metadata surface review

Render one V16 SCALE metadata file or compare two:

```bash
cargo run -p metadata-surface -- FILE.scale
cargo run -p metadata-surface -- OLD.scale NEW.scale
cargo run -p metadata-surface -- --types OLD.scale NEW.scale
```

Exit status is 0 for a single dump or matching surfaces, 1 for differences,
and 2 for an input or usage error. `--types` includes named registry type
definitions so layout changes behind unchanged names are visible.

This is a review aid. It omits docs, does not compare every metadata field,
and resolves type IDs into readable names. The diff counts occurrences of
each line and reports every surplus, including duplicate named type definitions.
Changes only to line ordering do not appear. An empty diff does not prove
byte equality or compatibility; the strict runtime metadata gate remains
the acceptance check. Deeply nested type references fall back to numeric IDs
to bound recursion.

Unit tests embed both `tests/fixtures/native-2609/*.scale` anchors with
`include_bytes!`. Replacing or relocating those fixtures rebuilds the tests
and may require updating their paths or expected network differences.

The inputs are the checked-in `tests/fixtures/native-2609/*.scale` captures,
produced by each runtime's `capture_native_2609_anchor` test. To reproduce a
review, export the earlier fixture with `git show COMMIT:tests/fixtures/native-2609/quip.scale`
and compare it with the current fixture using the commands above. See the
repository README for the explicit capture command and required Wasm checks.
