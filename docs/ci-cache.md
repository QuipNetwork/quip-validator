# CI cache on the runner hosts

The heavy Rust jobs keep Cargo state on a host volume, not in the GitLab
tar cache. Each amd64 runner host must provide that volume. Without it the
jobs still pass, but every build starts cold.

Affected jobs: `cargo-test`, `browser-signer-test`, `benchmark-preflight`.
All three extend the `.cargo-host-cache` template in `.gitlab-ci.yml`.
The bench-stage jobs on the reference machine already used this arrangement.

## Why the tar cache failed

The GitLab cache archives a directory into a zip file at the end of a job.
A Rust `target/` for this workspace holds 43,430 files. Measured times to
archive it in `cargo-test`:

| date | runner | archive time | result |
|---|---|---|---|
| 2026-09-05 | watch | 7 min 56 sec | cache written |
| 2026-09-08 17:40 | watch | 13 min 18 sec | cache written |
| 2026-09-08 18:46 | finney | over 30 min | killed by the 90 min job timeout |

A killed archiver writes nothing. The next run started cold, took longer to
compile, and left even less time to archive. Two pipelines failed that way
with every test already green.

Two more faults kept the cache cold:

- The archive goes to disk on the runner that ran the job. The amd64 pool has
  more than one host. A job that ran on a different host always missed.
- `py-signer-test` computed the same cache key as `cargo-test`, because both
  keyed on `files: [Cargo.lock]`. The 357-file registry archive from
  `py-signer-test` overwrote the 43,430-file archive from `cargo-test` on
  every pipeline.

## Host setup

Do these steps on each runner host in the amd64 pool.

The path inside the container is always `/ci-cache`. The path on the host is a
per-host choice. Put it on a disk with room. `finney` uses
`/zfspool/srv/ci-cache`.

1. Create the directory:

   ```sh
   mkdir -p /zfspool/srv/ci-cache
   ```

2. Mount it into the job containers. Edit `/etc/gitlab-runner/config.toml`:

   ```toml
   [[runners]]
     name = "finney"
     [runners.docker]
       volumes = ["/cache", "/zfspool/srv/ci-cache:/ci-cache"]
   ```

   Keep the `/cache` entry. The `volumes` key replaces the whole list, and the
   runner needs `/cache` for the jobs that still use the GitLab cache.

`gitlab-runner` watches `config.toml` and reloads it. Do not restart the
service. A restart cancels the jobs that are running.

The toolchain image sets no `USER`, so jobs run as root. Root writes to the
directory whatever its ownership, so no ownership change is needed.

### On ZFS

Make the cache its own dataset. A plain directory inherits the parent dataset
properties, and the next two settings then apply to the whole parent:

```sh
zfs create zfspool/srv/ci-cache
```

Turn off automatic snapshots for that dataset. Build artifacts churn heavily.
A snapshot keeps every deleted artifact on disk, so an hourly snapshot policy
fills the pool:

```sh
zfs set com.sun:auto-snapshot=false zfspool/srv/ci-cache
```

Check `atime` before you choose a pruning command:

```sh
zfs get atime,relatime zfspool/srv/ci-cache
```

`atime=off` breaks any prune that reads access times. See Pruning below.

## Directory layout

The template sets these variables:

```yaml
CARGO_HOME: /ci-cache/cargo-home
CARGO_TARGET_DIR: /ci-cache/$CI_CONCURRENT_ID/$CI_JOB_NAME/target
WASM_BUILD_WORKSPACE_HINT: $CI_PROJECT_DIR
```

`substrate-wasm-builder` finds the workspace by a search upward from
`OUT_DIR` for `Cargo.lock`. `OUT_DIR` is now outside the checkout, so that
search never reaches it. `WASM_BUILD_WORKSPACE_HINT` names the checkout
instead. Without it the runtime build prints a warning on every run.

All jobs share `CARGO_HOME`. Jobs rarely write to the registry, and Cargo
locks it correctly.

`CI_CONCURRENT_ID` is the runner execution slot. Two pipelines on one host get
separate target directories, so neither blocks on the Cargo target lock. The
runner reuses slot numbers across pipelines, which is what keeps a directory
warm.

Plan for one target directory per slot per job. With a runner concurrency of
2 and three jobs, that is 6 directories.

## Pruning

CI prunes the volume. No host cron is needed.

`scripts/prune-ci-cache.sh` runs in an `after_script` on the three jobs that
extend `.cargo-host-cache`. Those jobs reach every host in the pool on their
own, so the prune reaches every host that holds anything worth pruning. A
scheduled job would reach one runner only, so the prune rides along with the
jobs instead.

The script holds a per-host lock and prunes at most once a day. It exits 0 on
every path. A prune must never turn a green build red.

The toolchain image carries `cargo-sweep` at a pinned version. Do not install
it in a job.

### What the script removes

Two kinds of garbage need two different rules.

The first rule covers a slot that is still in use. `cargo sweep --maxsize`
evicts the oldest artifacts there, and only while that slot sits over its
ceiling. An age rule fails on this case, because cargo never updates an
artifact's modification time when it reuses one. The oldest files in a warm
slot are the stable dependencies worth keeping. A 14-day age rule
would delete most of the dependency graph on a slot that runs every day.

The second rule covers a slot that nothing writes to any more. `--maxsize`
never reclaims one, because an abandoned slot stays under the ceiling forever.
Each run stamps the slot it used in `target/.last-used`. The script then
removes any slot whose stamp has gone stale. A slot with no stamp gets a stamp
rather than a deletion, so the first run after this change keeps every warm
cache.

A `find -atime` prune is the obvious alternative to both rules. It reads access
times, and ZFS datasets with `atime=off` never update them. The command then
deletes everything or nothing.

### Tuning

The defaults are a starting point, not a measurement. Set the ceiling against
what the volume actually holds:

```sh
du -sh /zfspool/srv/ci-cache/*
```

| variable | default | meaning |
|---|---|---|
| `CI_CACHE_ROOT` | `/ci-cache` | volume root |
| `CI_CACHE_MAX_SIZE` | `50GiB` | per-slot ceiling |
| `CI_CACHE_ORPHAN_DAYS` | `14` | age at which an unused slot is removed |
| `CI_CACHE_INTERVAL` | `86400` | seconds between prunes on one host |
| `CI_CACHE_PROJECT_DIR` | the script's own checkout | manifest `cargo-sweep` reads |

Set any of these as a CI/CD variable to change the policy for every host.

`cargo-sweep` runs `cargo metadata`, so it needs a `Cargo.toml`. A slot holds a
bare `target/` with no manifest beside it, so the script points `cargo-sweep`
at the checkout and names the slot through `CARGO_TARGET_DIR`. Run the script
from a checkout for this reason. Without one it removes unused slots and skips
the sweep.

Run the script by hand on a host to see what it would remove:

```sh
cd /path/to/quip-validator
CI_CACHE_ROOT=/zfspool/srv/ci-cache scripts/prune-ci-cache.sh --dry-run
```

`--dry-run` reports the slots it would remove and passes through to
`cargo sweep`. Extra arguments reach `cargo sweep` unchanged.

## If the volume is missing

Cargo creates `/ci-cache` inside the container and the job passes. The
directory disappears with the container, so the build is cold every time.
The symptom is a `cargo-test` run near 60 minutes that shows no
`Compiling` skips.

To confirm the mount from a job log, look for the directory surviving between
runs. A warm run compiles far fewer crates than a cold one.
