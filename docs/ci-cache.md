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
```

All jobs share `CARGO_HOME`. Jobs rarely write to the registry, and Cargo
locks it correctly.

`CI_CONCURRENT_ID` is the runner execution slot. Two pipelines on one host get
separate target directories, so neither blocks on the Cargo target lock. The
runner reuses slot numbers across pipelines, which is what keeps a directory
warm.

Plan for one target directory per slot per job. With a runner concurrency of
2 and three jobs, that is 6 directories.

## Pruning

Each host must prune its own cache directory. A scheduled CI job cannot do
this, because a scheduled job runs on one runner only.

Add a cron entry or a systemd timer on each host. Use `cargo sweep`:

```sh
cargo sweep --time 14 --recursive /zfspool/srv/ci-cache
```

`cargo sweep` reads Cargo metadata, removes only build artifacts, and judges
age by modification time. Install it with `cargo install cargo-sweep`.

A `find -atime` prune is the obvious alternative, but it reads access times.
ZFS datasets with `atime=off` never update them, and the command then deletes
either everything or nothing. Use `cargo sweep` instead.

Check the size when a build slows down without an obvious cause:

```sh
du -sh /zfspool/srv/ci-cache/*
```

## If the volume is missing

Cargo creates `/ci-cache` inside the container and the job passes. The
directory disappears with the container, so the build is cold every time.
The symptom is a `cargo-test` run near 60 minutes that shows no
`Compiling` skips.

To confirm the mount from a job log, look for the directory surviving between
runs. A warm run compiles far fewer crates than a cold one.
