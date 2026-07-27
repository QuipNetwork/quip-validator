<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Releasing `quip-signer` to PyPI

The `quip-signer` PyO3 binding ships as abi3 wheels (+ sdist) on PyPI via the
release jobs in [`.gitlab-ci.yml`](../.gitlab-ci.yml) (`release:build-pypi`,
`release:smoke-pypi-amd64`, `release:smoke-pypi-arm64`,
`release:publish-testpypi`, `release:publish-pypi`) backed by
[`scripts/python-dists.sh`](../scripts/python-dists.sh). The pipeline is
build → smoke (both arches) → publish; publishing uses PyPI **Trusted Publishing
(OIDC)** — there is no long-lived API token in CI.

Tracking issue: QUI-776.

## How a release flows

```
tag v0.X.Y  ──▶  release:publish-testpypi   (auto)   → TestPyPI
                 release:publish-pypi        (manual) → PyPI  (click to promote)
```

- The build + smoke jobs (`release:build-pypi`, `release:smoke-pypi-amd64`,
  `release:smoke-pypi-arm64`) run on **every MR/push**: they build all three
  artefacts and import-test both wheels, so a packaging regression fails an MR
  rather than a tag pipeline.
- `release:build-pypi` guards the wheels beyond `twine check`, which only reads
  metadata: it asserts each wheel carries a manylinux/musllinux platform tag,
  asserts the `quip_signer` Python surface is packed (QUI-792), and runs `twine
  check`, then publishes `dist/` as an artefact the smoke and publish jobs
  consume (so the uploaded bytes are the ones that were smoke-tested).
- The two smoke jobs import-test the built wheels **on their native
  architectures** (QUI-793): each installs the one wheel matching its runner
  arch into a throwaway venv and runs `import quip_signer` + a sign/verify
  round-trip. The amd64 job covers the x86_64 wheel; the arm64 job covers the
  cross-built aarch64 wheel — the artefact Linux/aarch64 consumers pull, now
  import-tested on real arm64 before publish rather than shipped unexercised. A
  wheel that builds but can't import fails the build before any upload, and the
  publish jobs `needs` both smokes.
- On a release tag, TestPyPI publishes automatically. PyPI is a **manual
  "promote" button** in the same pipeline — click it once the TestPyPI wheel is
  verified.

### Artefact matrix

| Artefact | Platform | How |
|---|---|---|
| abi3 wheel | linux x86_64 | native `maturin build` |
| abi3 wheel | linux aarch64 | `maturin build --target aarch64-unknown-linux-gnu --zig` |
| sdist | any | `maturin sdist` (macOS/Windows build from this) |

One wheel per arch serves CPython ≥ 3.9 (the crate's `abi3-py39` floor).

## Versioning

The wheel version is **single-sourced from the crate**, not the git tag:
`crates/transaction-crypto-py/Cargo.toml`'s `[package] version`. The pyproject
declares `dynamic = ["version"]`, so maturin reads that one value (same pattern
as xquad's `xqffi`). The `quip-signer` version line is **independent** of the
node's `v0.2.x` line — the tag triggers a publish, it does not name the wheel
version.

To ship a new `quip-signer` version:

1. Bump `[package] version` in `crates/transaction-crypto-py/Cargo.toml` in an
   MR; merge it. (pyproject picks it up automatically — nothing else to edit.)
2. The next `v*` tag publishes that version. Re-tagging at an unchanged version
   is a no-op (`twine upload --skip-existing`), so existing node tags that
   re-fire the pipeline never error.

## One-time setup (cannot be done in CI)

Do these once per registry. Phase 1 is TestPyPI; Phase 2 adds production PyPI.

### 1. GitLab side (both phases)

- **Protected tag pattern** — Settings → Repository → Protected tags: add `v*`.
- **`release` environment** — Settings → CI/CD → Environments: create an
  environment named exactly `release`. Restrict its deployments to protected
  tags so only `v*` tag pipelines can mint OIDC tokens against it.
- **Enable publishing** — Settings → CI/CD → Variables: set
  `QUIP_SIGNER_PUBLISH` to `true`. Until this is set, the publish jobs stay
  dormant on `v*` tags, so a node release tag never reds on an unconfigured
  wheel publish. Set it **only after** the Trusted Publisher below is configured
  (do it per registry: enable after TestPyPI for Phase 1, keep it on for Phase 2).

The CI project coordinates the Trusted Publisher needs below:

| Field | Value |
|---|---|
| Namespace | `quip.network` |
| Project | `quip-validator` |
| Top-level pipeline file path | `.gitlab-ci.yml` |
| Environment | `release` |

(Confirm the namespace against the CI project URL —
`gitlab.com/quip.network/quip-validator`.)

> **If the GitLab project is ever renamed or moved, publishing breaks.** The
> OIDC token GitLab mints carries the project path as a claim, so a Trusted
> Publisher configured against the old path stops matching and `twine` fails
> with `publisher config mismatch`. Nothing in CI detects this — the first
> symptom is a red `v*` tag pipeline. Update the **Project** field above on
> **both** test.pypi.org and pypi.org (`/manage/account/publishing/`) as part
> of the rename, before the next release tag.
>
> This repository was renamed `quip-protocol-rs` → `quip-validator` on
> 2026-07-27; the table above reflects the new name.

### 2. TestPyPI (Phase 1)

1. Create the project owner account / org access on https://test.pypi.org.
2. Add a **GitLab Trusted Publisher** at
   `https://test.pypi.org/manage/account/publishing/` for project name
   `quip-signer`, using the four fields in the table above. (For a brand-new
   project name, add it as a *pending* publisher — it activates on first
   upload.)
3. Set `QUIP_SIGNER_PUBLISH=true` (GitLab step above), then cut a release tag
   and let `release:publish-testpypi` run.
4. Verify the install resolves and imports on both arches:
   ```bash
   pip install -i https://test.pypi.org/simple/ quip-signer
   python -c "import quip_signer; print(quip_signer.__file__)"
   ```

### 3. Production PyPI (Phase 2)

1. Create the `quip-signer` project on https://pypi.org (the name is currently
   available — claim it).
2. Add the GitLab Trusted Publisher at
   `https://pypi.org/manage/account/publishing/` with the same four fields.
3. On the next release tag, click the manual **`release:publish-pypi`** job to
   promote.
4. Verify:
   ```bash
   pip install quip-signer            # linux x86_64 + aarch64 get wheels
   python -c "import quip_signer"
   ```
   macOS/Windows fall back to building the sdist with a local Rust toolchain.

## OIDC audiences (reference)

The two registries are separate OIDC realms; the job's `id_tokens` `aud` and the
script's mint/upload endpoints differ accordingly:

| Registry | `aud` | mint endpoint | upload URL |
|---|---|---|---|
| TestPyPI | `testpypi` | `https://test.pypi.org/_/oidc/mint-token` | `https://test.pypi.org/legacy/` |
| PyPI | `pypi` | `https://pypi.org/_/oidc/mint-token` | `https://upload.pypi.org/legacy/` |

## Troubleshooting

- **`invalid-publisher` / publisher-config mismatch** on mint: a Trusted
  Publisher field (namespace / project / pipeline path / environment) does not
  match what the job presents. The script prints the registry's response with
  the token redacted — compare against the table above.
- **Wheel rejected for a `linux_x86_64` platform tag**: PyPI only accepts
  `manylinux*` / `musllinux*` wheels. The native x86_64 build must stay
  manylinux-compatible (it depends on the build image's glibc being old enough
  for a manylinux profile). `twine check` does **not** inspect platform tags, so
  `scripts/python-dists.sh` asserts every wheel carries a manylinux/musllinux tag
  after the build — that assertion (not `twine check`) is what fails
  `release:build-pypi` on a non-portable wheel. The aarch64 build targets an old glibc via the zig
  linker, so it stays manylinux by construction; if the native x86_64 wheel ever
  trips the guard, build it via cargo-zigbuild too (pin an older glibc).
