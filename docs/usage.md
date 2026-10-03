# Using goway

## Run a command

```
goway run -- cargo nextest run --workspace
goway run --host helios -- cargo build --release
goway run --keep -e RUST_LOG=debug -- ./scripts/integration.sh
goway run --report run.json -- make test
```

What happens:

1. **Pick a host.** Without `--host`, every host is probed in parallel
   (one ssh call each) and the lowest `(load1 + goway jobs) / cores`
   wins. Unreachable hosts, hosts at `max_jobs` and hosts above
   `max_load` (load per core) are skipped.
2. **Sync.** The file set is exactly what git shows:
   `git ls-files -co --exclude-standard`, minus files deleted from the
   work tree, minus secret-looking files (see below) and anything inside
   a symlinked directory. It goes to a per-worktree seed on
   the host. Only files whose size, exec bit or link target changed are
   sent (as a tar stream), plus files whose mtime changed and whose
   content (sha256) differs. Files gone locally are deleted. mtimes are
   kept, so cargo stays warm. A worktree's first sync starts from a
   hard-link copy of the most recently used seed of the same
   repository, so a new worktree only sends what differs.
   Each seed has a generation token. If gc replaced the seed between
   reading its manifest and uploading, the upload is refused and the
   sync starts over, so a delta never lands on the wrong base.
3. **Snapshot.** In the same locked step as the upload, the run gets a
   fresh work dir that is a copy of the seed (reflinked where the
   filesystem supports it), so nothing the job writes reaches the seed
   or another run.
   Concurrent runs from the same or other worktrees never
   see each other's files, and a later sync never changes a running
   snapshot.
4. **Run.** The command starts in its own session, with stdin, stdout
   and stderr passed through untouched. `CARGO_TARGET_DIR` points at
   the first free per-repository target slot (a new one when all are
   busy, up to `target_slots`). While it holds slot k, the job runs in
   the tree at `cache/<repo>/tree-k` (its snapshot, moved there). Builds
   bake absolute source paths into binaries (`CARGO_MANIFEST_DIR`,
   `file!()`), and cargo reuses binaries when only the path changed, so
   a slot's binaries always find the current tree where they expect it. `~/.cargo/env` is sourced. sccache is
   used if installed. With `priority = "low"` (the default) the job
   runs under `nice -n 10` with idle-class I/O.
5. **Finish.** The work dir is removed unless `--keep` is given, a
   cheap gc of expired entries runs in the background, and goway exits
   with the command's exit code.

A line on stderr says where the job ran:
`goway: running on helios (x86_64, Helios) at 192.0.2.10: cargo test`.
`--report FILE` writes the same as JSON (host, address, arch, hostname,
command, exit code, duration, run id, repository) for evidence records.

### Sharding one run across hosts

```
goway run --shard 2 -- cargo nextest run --workspace
```

`--shard N` picks the N least-loaded usable hosts, syncs to all of them
in parallel and runs part i of N on each. goway does not invent its own
partitioning: for `cargo nextest run` it adds nextest's own
`--partition count:i/N` (before any `--`). Every command sees
`GOWAY_SHARD=i` and `GOWAY_SHARD_COUNT=N`, so other test runners can
split their work the same way. Output lines are prefixed with
`[host] `. goway exits with the first failing shard's code, or 0 when
every shard passed. `--report` lists every shard's host, arch, address,
command and exit code. Each host builds for itself, so give every host
the toolchain (`goway doctor --fix`).

### Exit codes

| Code | Meaning |
|---|---|
| the command's own | normal completion |
| 128+N | the command died of signal N |
| 130 | you pressed Ctrl-C (the remote job is stopped by its watchdog) |
| 125 | goway itself failed before or around the command (config, ssh, sync) |

### Ctrl-C and lost connections

sshd does not signal commands that run without a terminal; it orphans
them. goway's remote side therefore runs a watchdog per job. When the
ssh session goes away, whether from Ctrl-C, a closed laptop lid or Wi-Fi
loss, the watchdog sends the job's process group TERM, then KILL after 5
seconds, and the work dir is cleaned up.

## Status

`goway status` prints for every host:
- the current address and how it was found
- arch and cores
- load averages
- running goway jobs (against `max_jobs` if set)
- disk used by goway and disk free

Unreachable hosts are marked.

## Clean up

Remote state is labelled (`meta.json`: kind, repository, worktree,
client, time) and locked with `flock` while in use:

| Entry | Expires after |
|---|---|
| work dir of a crashed run | `orphan_ttl` (1 day) |
| work dir kept with `--keep` | `kept_ttl` (3 days) |
| seed (synced mirror per worktree) | `cache_ttl` (7 days idle) |
| per-repository cache (target slots, sccache) | `cache_ttl` (7 days idle) |

Every run collects expired entries on its host. To clean up on demand:

```
goway gc --dry-run                    # what would go
goway gc --repo goway --older-than 2d
goway gc --host helios --all          # everything not in use
```

Locked entries (a run in progress) are reported `busy` and never
touched. gc removes only entries goway labelled itself, and only under a
root that carries goway's `.goway-root` marker. Anything else you put
there is left alone and reported as `unlabelled`.

## Doctor

```
goway doctor                 # every host
goway doctor helios --fix    # run the fixes that need no root
goway doctor helios --fix --sudo
```

Checks:
- goway's remote prerequisites: bash, tar, flock, setsid
- curl, which the fixes use for downloads
- the C linker cargo needs
- cargo/rustup, cargo-nextest and sccache
- disk space
- whether sshd still allows password logins
- the local ssh setup

Each problem comes with the exact command that fixes it. `--fix` runs
the user-level fixes as your ordinary user:
- rustup
- the prebuilt nextest
- the prebuilt sccache release

Fixes that need root (system packages, sshd config) are never run
silently. goway lists each one with the reason and asks you to rerun
with `--sudo`, which runs them over an interactive ssh session so sudo
can ask for your password.

## First-time ssh setup

`goway ssh setup HOST` makes key login work with one password login and
is undone by `goway ssh setup HOST --undo`; see docs/ssh-setup.md.

## What is sent to the remote

- The git-visible work tree (see Sync above), without `.git`.
- Secret-looking files stay on your machine unless you allow them. The
  match ignores case and covers:
  - env files (`.env`, `.env.*`, `.envrc`)
  - credential files (`.npmrc`, `.netrc`, `.pypirc`, `.git-credentials`,
    `credentials*`)
  - private keys (`id_*` except `.pub`)
  - key and certificate stores (`*.pem`, `*.key`, `*.p12`, `*.pfx`,
    `*.jks`)
  - anything under `.ssh`, `.aws`, `.gnupg`, `.docker` or `.kube`

  `goway run` lists the files it kept back. To send some anyway, add
  them to `secret_allow` in the config. Files inside a directory that is
  a symlink are never read.
- The command line, and any `--env` values. The values travel over the
  encrypted ssh connection's input into a file only you can read, which
  is deleted when the job starts. They never appear in the host's process
  list or in goway's logs.
- Labels: repository name, a repository id (a hash of the root commit),
  the local worktree path, and this machine's hostname.

Nothing else is sent. Keys stay with your ssh client and agent.
