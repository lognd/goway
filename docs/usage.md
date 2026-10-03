# Using goway

## Add a helper laptop

```
goway add <YOUR-COMPUTER-NAME-HERE> --fingerprint <FINGERPRINT-FROM-THE-INSTALLER> --user <YOUR-LINUX-USER-NAME-HERE>
goway add <YOUR-COMPUTER-NAME-HERE> --rsudo   # also make the administrator changes there
```

The helper's installer prints this exact command with its values filled
in. The README says how to find each value by hand
("What to put in place of each <...>").
`goway add` does the following:
1. Checks that this laptop has ssh and git. With `--lsudo` it installs
   what is missing here.
2. Sets up key login with one password prompt (docs/ssh-setup.md).
3. Pins the helper's identity, and adds it to the pool.
4. Installs its toolchain (cargo-nextest, sccache).
5. With `--rsudo`, it lists the changes that need administrator rights
   on the helper, with the reason for each, asks once, and runs them
   all in one sudo session. You type the helper's password into its own
   sudo; goway never sees it.

Running `goway add` again changes nothing that is already in place.


## Run a command

```
goway run -- cargo nextest run --workspace
goway run --host <YOUR-COMPUTER-NAME-HERE> -- cargo build --release
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
   and stderr passed through (see "Terminal output" below). `CARGO_TARGET_DIR` points at
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
`goway: info: running on my-helper (x86_64, MY-HELPER) at 192.0.2.10: cargo test`.
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

### Terminal output

The command runs somewhere else and may be untrusted, so what it prints
must not be able to drive your terminal. When goway's stdout or stderr is
a terminal, the output of that stream keeps text, newline, carriage
return, tab, backspace and color (SGR) sequences, and drops everything
else: OSC strings (clipboard writes, window titles), DCS, APC, PM and SOS
strings, every other CSI sequence (cursor movement, terminal queries and
reports) and all other control characters. Sequences split between two
reads are handled. With `--output=raw` (or `GOWAY_OUTPUT=raw`) every byte
passes through. When a stream is a pipe or a file (`goway run ... > log`,
frob evidence), the output is never touched, byte for byte. Sharded runs
(`--shard`) follow the same rules per `[host]`-prefixed stream.

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
goway gc --host <YOUR-COMPUTER-NAME-HERE> --all   # everything not in use
```

Locked entries (a run in progress) are reported `busy` and never
touched. gc removes only entries goway labelled itself, and only under a
root that carries goway's `.goway-root` marker. Anything else you put
there is left alone and reported as `unlabelled`.

## Uninstall

```
goway uninstall                 # lists what it would remove everywhere, then asks
goway uninstall --everywhere    # does it without asking
goway uninstall --dry-run       # only lists
```

On every helper this removes:
- goway's state directory (refused while a run is in progress)
- the tools goway installed there (cargo-nextest, sccache, and rustup if
  goway installed it)
- goway's key line in `~/.ssh/authorized_keys`, with the previous modes
  restored

Administrator-level changes (the "no password login" setting) are
undone with `--rsudo`, after one question. System packages goway
installed, such as the C compiler, stay, because other software may
use them; goway lists them with their removal command. If a helper is
off, nothing on this laptop is removed, so you can run it again later.

On this laptop it then removes goway's config, keys and state, and
finally the goway program and its PATH line, using the install record.
On Windows, remove the program in Settings > Apps. On each helper
laptop, its own uninstall entry in Settings > Apps removes the helper
setup.

## Doctor

```
goway doctor                 # every host
goway doctor <YOUR-COMPUTER-NAME-HERE> --fix          # run the fixes that need no root
goway doctor <YOUR-COMPUTER-NAME-HERE> --fix --rsudo  # also the administrator fixes
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
- rustup, from its official installer
- cargo-nextest, a pinned release verified by sha256 before it is
  unpacked
- sccache, a pinned release verified the same way

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
  - environment files (dot-env files, their `.env.*` variants, `*.env`
    such as `prod.env`, `.envrc`)
  - credential files (`.npmrc`, `.yarnrc.yml`, `.netrc`, `.pypirc`,
    `.git-credentials`, `.pgpass`, `.htpasswd`, `.dockercfg`,
    `.vault-token`, `.my.cnf`, `.boto`, `.s3cfg`, `rclone.conf`,
    `auth.json`, `master.key`, `kubeconfig*`, `service-account*.json`,
    `secrets.yaml`) and any data file (json, yaml, toml, ini, xml, txt,
    no extension) whose name contains `secret` or `credential`, such as
    `.cargo/credentials.toml`; source files like `secret_store.rs` are
    sent
  - Terraform state and variables (`*.tfstate`, `*.tfstate.backup`,
    `*.tfvars`)
  - private keys (`id_*` except `.pub`)
  - key, certificate and password stores (`*.pem`, `*.key`, `*.p12`,
    `*.pfx`, `*.p8`, `*.jks`, `*.jceks`, `*.keystore`, `*.ppk`, `*.kdbx`,
    `*.gpg`, `*.pgp`)
  - anything under `.ssh`, `.aws`, `.gnupg`, `.azure`, `.docker`,
    `.kube`, `.terraform`, `.gcloud`, `.config/gh` or `.config/gcloud`

  This is a denylist: a secret with an unusual name that you track in git
  is still sent.

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
