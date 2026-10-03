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
   fresh work dir holding a hard-link snapshot of the seed (no data is
   copied), so a later sync never changes what the run sees.
   Concurrent runs from the same or other worktrees never
   see each other's files, and a later sync never changes a running
   snapshot.
4. **Run.** The command starts in its own session, with stdin, stdout
   and stderr passed through (see "Terminal output" below). `CARGO_TARGET_DIR` points at
   the first free per-repository target slot (a new one when all are
   busy, up to `target_slots`). While it holds slot k, the job runs in
   the tree at `cache/<repo>/tree-k`. That tree persists: each run updates
   it in place from its snapshot (only changed files are written, deleted
   files are removed, leftovers from earlier runs are removed) and keeps
   detected dependency and build directories such as `node_modules`,
   `.venv` and `build/`, your `keep` list and `.gitignore`d paths (see
   docs/config.md). A worktree prefers the slot it used last. Builds
   bake absolute source paths into binaries (`CARGO_MANIFEST_DIR`,
   `file!()`), and cargo reuses binaries when only the path changed, so
   a slot's binaries always find the current tree where they expect it. `~/.cargo/env` is sourced. sccache is
   used if installed. With `priority = "low"` (the default) the job
   runs under `nice -n 10` with idle-class I/O.
5. **Finish.** The work dir is removed unless `--keep` is given (then
   the finished tree is copied into it for inspection), a
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
in parallel and runs part i of N on each. goway recognises the test
framework from the command line (and, for `npm test` style commands, from
`scripts.test` in `package.json`) and splits the tests the way that
framework supports. Every command also sees `GOWAY_SHARD=i` and
`GOWAY_SHARD_COUNT=N`. A command goway does not recognise runs unchanged
on every host with just those two variables, so your own runner can split
by them. Output lines are prefixed with `[host] `. goway exits with the
first failing shard's code, or 0 when every shard passed. `--report`
lists every shard's host, arch, address, command and exit code. Each host
builds for itself, so give every host the toolchain (`goway doctor --fix`).

goway refuses (usage error, before any host is used) a command that
already splits itself: `--partition`, `--shard`, `--shard-count`,
`-I` for ctest, `-Dtest=` for Maven, `--tests` for Gradle, or the
`GTEST_*` shard variables.

#### Capacity-weighted shares

Hosts differ, so equal shares leave the big ones idle while the small ones
finish last. When the framework can split by weight, goway sizes each host's
share by its **free capacity**, measured when the hosts are chosen: cores
minus the 1-minute load minus goway jobs already running (never below half a
core). The freest host gets weight 4, the others a proportional whole weight
from 1 to 4 (so hosts within about 12% of each other split equally), reduced by
their common divisor; the shares are those weights out of their sum. goway
prints `shares by free cores: big 4/5, small 1/5` and each finished shard shows
its percentage.

- **cargo nextest**: the tests are split into as many `--partition count:p/M`
  partitions as the weights sum to (M), and a host runs the partitions it owns,
  one nextest run after the other in one command (every partition runs even if
  an earlier one fails; the first failure's exit code is the shard's). Equal
  weights are the plain single `count:i/N` run.
- **pytest, RSpec, go test, Maven, Gradle**: goway's own file, package or class
  split becomes a weighted round-robin over the sorted list, so a host with twice
  the weight gets twice the units, interleaved evenly. With equal weights it is the
  plain round-robin of before.
- **vitest, jest, Playwright (`--shard=i/N`), CTest (`-I`), GoogleTest, Catch2
  and unknown commands** split natively and equally; goway says so when the hosts
  differ.

Either way each test runs on exactly one shard. The `--report` file lists, per
shard, `host`, `arch` and `share` (`capacity`, `weight`, `total_weight`,
`fraction`, and `weighted`: false when the split was equal).

#### Framework adapters

| Framework | How it is sharded | Prerequisites | Status |
|---|---|---|---|
| cargo nextest | `--partition count:i/N` | none | tested (unit, and in CI) |
| vitest, jest, Playwright | `--shard=i/N`; also via `npm/pnpm/yarn test` when `scripts.test` names the tool (`npm test -- --shard=i/N`) | the framework 3.x/29+/1.x | argument rewriting unit-tested; not run against the real tools |
| Catch2 v3 | `--shard-count N --shard-index i-1` | detected from the binary on the helper (below), or `--env GOWAY_RUNNER=catch2` | detection tested end to end with faithful fixture binaries; not run against the real library |
| GoogleTest | env `GTEST_TOTAL_SHARDS=N`, `GTEST_SHARD_INDEX=i-1` | detected from the binary on the helper (below), a `--gtest_*` argument, or `--env GOWAY_RUNNER=gtest` | detection tested end to end with faithful fixture binaries; not run against the real library |
| CTest | `-I i,,N` (every Nth test starting at the i-th) | `ctest` on the host | tested with real ctest through the fake-ssh harness |
| pytest | goway splits the `test_*.py` / `*_test.py` files of the synced project (sorted, round-robin) and passes them as arguments; explicit path arguments narrow the set and `--ignore` is honoured; composes with pytest-xdist (`-n auto` runs inside each shard) | none | tested with real pytest through the fake-ssh harness; xdist not run |
| go test | goway splits the packages that contain `*_test.go` (skipping `testdata`, `vendor` and nested modules); needs a pattern such as `./...` or `./x/...` | none | tested with real `go test` through the fake-ssh harness |
| Maven | `-Dtest=<fully qualified classes>` from `src/test/**` (surefire's `Test*`, `*Test`, `*Tests`, `*TestCase`), plus `-Dsurefire.failIfNoSpecifiedTests=false` | surefire 2.19 or newer | class split unit-tested; Maven not run |
| Gradle | repeated `--tests <fully qualified class>` | a project where every module with tests has matching classes (Gradle fails a project whose filter matches nothing) | class split unit-tested; Gradle not run |
| RSpec | goway splits the `*_spec.rb` files under `spec/` and passes them as arguments | none | file split unit-tested; RSpec not run |

#### Test binaries: GoogleTest and Catch2 are detected

`goway run --shard N -- ./build/tests` needs no `GOWAY_RUNNER`. A command
whose program is not a known tool is checked on each helper, right before it
runs (the binary is usually built there during the run), by
`shard_run` in `remote.sh`:

- The program is resolved the way the shell will (a path containing `/`
  relative to the work tree, otherwise a `PATH` search) and only that file is
  inspected, never its arguments.
- The file is **read, never executed**. It must start with ELF or PE magic
  bytes (scripts never count) and contain every marker of one framework,
  found with a fixed-string search over at most its first 256 MiB. The
  markers are flag and variable names a framework needs to parse its own
  command line, so they survive stripping:
  GoogleTest `GTEST_SHARD_INDEX`, `GTEST_TOTAL_SHARDS`, `--gtest_list_tests`,
  `--gtest_filter`; Catch2 v3 `--shard-count`, `--shard-index`,
  `--list-tests`, `Catch2TestRun`. A file with only some markers is not
  detected, Catch2 v2 (no shard flags) is not detected, and neither is a
  file with both sets (goway's own binary carries both, because it embeds
  this script). An undetected program runs unchanged and sees `GOWAY_SHARD`
  and `GOWAY_SHARD_COUNT`.
- `GOWAY_RUNNER=gtest|catch2` overrides detection (no check is made). A
  program that already has `GTEST_TOTAL_SHARDS`/`GTEST_SHARD_INDEX` in its
  environment or `--shard-count`/`--shard-index` among its arguments is left
  alone.

Checks afterwards, because a marker proves a binary can parse the options,
not that sharding took effect:

- **GoogleTest** shards get `GTEST_SHARD_STATUS_FILE`. GoogleTest creates it
  when it applies sharding. If it is missing after the run (`--gtest_list_tests`
  and help runs are not judged), goway warns and does **not** rerun: every
  shard ran the whole suite, so the results stand, but the work was
  duplicated. `--report` records `"duplicated": true`.
- **Catch2** shards are rerun when the binary rejected the shard flags: the
  attempt exited non-zero and its stderr *begins* with Catch2's
  `Error(s) in input:` and names `--shard-count` or `--shard-index`, so no test
  ran. The shard then runs **once** more without the flags (it runs the whole
  suite) and `--report` records both attempts. The rerun is the second of two
  explicit calls in the script (attempt 1 has the flags, attempt 2 has
  none and nothing follows it); it is never driven by the environment or by
  output. If the rerun is rejected as well, goway stops and reports the
  failure (`"rejected_again": true`). Any less certain command-line error is
  only flagged (`"flagged": true`) and never rerun.

After a failed detection (the missing status file, or a Catch2 rerun) goway
remembers the program path for that repository in its **local state file
only** and skips detection on later runs, with a note, so shards use the
`GOWAY_SHARD` fallback. `GOWAY_RUNNER` overrides the memory, and
`goway gc --repo NAME` (or `--all`) clears it.

The helper reports what it did in one result line at the end of the shard's
stderr; goway strips that line (it carries a per-shard nonce) and records it
in `--report` under each shard's `detection`. The line only feeds the report,
the notes and the local memory.

Fixtures: the real frameworks are not installed on the test machines, so
`tests/shard_detect.rs` builds tiny stripped C programs (real ELF files) that
embed exactly the frameworks' marker strings and behave like them (read the
shard variables or flags, touch the status file, or reject the flags with
Catch2's error text). Windows `.exe` helpers need the same checks in
`remote.ps1` (PE `MZ` magic, same markers).

Splitting by file works from the synced file list, so it needs no round
trip to the host. A shard that receives no tests runs `true` instead of
running everything. The split is deterministic (stable sort, then
round-robin by position), so reruns and `--keep` debugging reproduce a
shard. Unit tests of every splitting adapter prove the shards together
cover each file, package or class exactly once.

### Choosing hardware: `--needs` and `--prefers`

```
goway run --needs gpu-mem>=8G,cuda>=12.1 -- cargo test --features cuda
goway run --needs mem>=16G --prefers cpu=avx512f -- cargo nextest run
goway run --shard 3 --needs kvm -- ./vm-tests
```

`--needs` terms are hard: a host that fails any is never used (not even with
`--host`). `--prefers` terms are soft: each one a host meets lowers its score
by 0.5 (about half a core of load per core), so a preferred host wins unless
it is much busier; a host that lacks a preference is never excluded. Both
flags repeat or take comma-separated terms.

| Term | Meaning |
|---|---|
| `gpu`, `gpu=cuda`, `gpu=rocm` | a GPU (any, NVIDIA, AMD) visible on the host |
| `gpu-mem>=8G` | some single GPU with at least that much memory |
| `cuda>=12.1` | a GPU whose CUDA version (from `nvidia-smi`) is at least this |
| `mem>=16G` | total RAM |
| `cores>=8` | logical CPUs |
| `arch=x86_64` | `uname -m` (`amd64` and `arm64` are accepted) |
| `os=linux` | `uname -s`, lower case |
| `cpu=avx512f` | a CPU feature: `avx2`, `avx512f`, `neon` |
| `kvm`, `docker` | usable `/dev/kvm`; a working `docker info` |
| `disk>=50G` | free disk where goway keeps its state |
| `label=NAME` | the host's `labels = ["NAME"]` entry in the config |

Sizes are binary (`16G` is 16 GiB) with `K`, `M`, `G` or `T`. A term that is
not in the table is an error that lists the valid ones. goway never guesses
needs from a project's dependencies; `goway doctor` may suggest a rule.

When no host qualifies, goway exits 125 and lists every host with what it
lacks (`helios: lacks gpu-mem>=48G: largest GPU has 24.0 GiB`). With
`--shard N`, every shard's host must meet the needs; preferences only order
the choice. Facts come from the host's own probe: RAM and disk live, the
rest cached daily (see `goway status`). A fact that was never probed counts
as not met.

The `--report` file (each shard's entry when sharded) lists `matched`: every
need and preference the chosen host met and the fact that met it (GPU model
and memory, RAM, cores), so a result says what it was measured on. The
`running on` line shows them too.

#### Project rules: `goway.toml`

A `goway.toml` at the repository root (a normal tracked file; it is synced
like any other) can say what a command needs, so nobody has to remember the
flags:

```toml
[[rule]]
command = "cargo nextest*"      # glob over the whole command line
needs = ["mem>=8G"]
prefers = ["cpu=avx2"]

[[rule]]
command = "pytest*"
needs = ["label=gpu-box"]
```

The command is a glob (`*` any run of characters, `?` one character) matched
against the command line with its words joined by single spaces, so end it
with `*` to allow arguments. The **first** rule that matches applies, and goway
says so (`goway.toml rule 1 (command = "cargo nextest*") applies: needs
mem>=8G ...`; the `--report` file records it as `rule`). The terms are merged
with `--needs` and `--prefers`, and the command line wins: a command-line term
replaces the rule's term of the same key in the same list (`--needs mem>=16G`
replaces the rule's `mem>=8G`; `cpu=` and `label=` terms with different values
add up). The file has no keys for host names or secrets; an unknown key or a
bad term is an error naming the file and line, and nothing runs. goway never
infers a rule from a project's dependencies.

`label=NAME` matches the `labels = [...]` of a `[[host]]` in your own config
(see [config.md](config.md)); only hosts with the label qualify.

#### GPU slots

A run whose `--needs` (or `goway.toml` rule) asks for a GPU (`gpu`, `gpu=cuda`,
`gpu=rocm`, `gpu-mem>=`, `cuda>=`) holds a lock on one GPU of the host while it
runs, so concurrent GPU runs get different GPUs instead of fighting over one:

- the host lists its GPUs live (`nvidia-smi`, `rocm-smi`); the run takes the
  first free slot, spreading over the GPUs before sharing any, and says
  `using GPU 1 (nvidia), slot 0`;
- `CUDA_VISIBLE_DEVICES` and `ROCR_VISIBLE_DEVICES` name that GPU for the
  command (on a host with one GPU vendor both are set; with two, each only for
  its own vendor). A variable you set with `--env` or on the host stays yours;
  the run still takes a slot, so it is counted against the GPU's capacity;
- when every GPU is busy the run prints `all 2 GPU(s) are busy; waiting for
  one` and waits, **before** it takes a build slot, so a queue for GPUs does
  not pin the build slots CPU-only runs need;
- `gpu_jobs = N` (in `[defaults]` or a `[[host]]`; default 1) lets up to N GPU
  runs share each GPU;
- the lock is a `flock` held by the run's shell, so it is released however the
  run ends (exit, signal, a dropped connection). A host with no GPU tool gets a
  warning and the run proceeds without a slot.

Runs that do not ask for a GPU never take a slot. GPU locks live under the
remote root's `gpu/` directory, which `goway uninstall` removes.

### Running on this machine: `--host local`

```
goway run --host local -- cargo test      # here, in place, no sync
```

`--host local` runs the command on this machine, in the current directory
(the work tree as it is: nothing is synced, nothing is copied), with live
output and the command's own exit code. A command that cannot be started
exits 127 (not found) or 126, as a shell would. Priority follows
`[local] priority` (default: `defaults.priority`, so `low` runs under `nice`
and `ionice`). `--needs` still applies, and `--report` records host `local`
and this machine's architecture. A `[[host]]` called `local` in your config
wins over this meaning of the name (and keeps this machine out of the pool).

The rest is opt-in, in the config (see [config.md](config.md)):

```toml
[local]
pool = true        # compete with the helpers (default false)
max_jobs = 1       # most goway jobs here at once when pooled (default 1)
margin = 0.5       # added to this machine's score; helpers win unless it is clearly less loaded
fallback = true    # run here, with a note, when no helper is reachable (default false)
```

- Without `[local]`, or with `pool = false`, goway never picks this machine for
  a normal run or a shard.
- With `pool = true` this machine is probed like a helper (RAM, GPUs and
  load, so `--needs` works) and competes on load with its `margin`; it can take
  shards (its shard runs in place, and the GoogleTest/Catch2 detection above is
  only done on helpers). Running local jobs are counted in files under goway's
  state directory, so `max_jobs` holds across processes and a crashed run frees
  its slot.
- Without `fallback`, when no helper is reachable `goway run` exits 125 with the
  reason for each helper and names both ways out: `--host local` and
  `[local] fallback = true`. With `fallback = true` it runs here and says so
  (`no helper is reachable; running on this machine because ...`); the report
  records host `local`. Only a run where *every* helper failed to answer falls
  back; busy or overloaded helpers do not, and a run with `--needs` this
  machine cannot meet fails instead.
- `goway status` shows a `local` row (load, RAM, jobs against `max_jobs`) when
  `[local]` exists, saying whether it is in the pool.

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

#### Output integrity

Everything goway writes to your terminal or pipe goes through one global
output lock: goway's own messages (info, note, warning, error), the
`[host]`-prefixed lines of sharded runs, and the pass-through of a
single-host run. Each logical line, prefix included, is assembled in one
buffer and written with one write call, so a line from the other stream
or another host can never land inside it. In a sharded run:

- all complete lines already read are written as one batch per lock;
- a line longer than 64 KiB is split, each continuation starting with
  `[host]+ ` instead of `[host] `, and no memory beyond one such line is
  held per stream;
- a last line with no newline is terminated, so the next host's line
  never joins it;
- a slow terminal blocks the writer while it holds the lock, which
  stops reading from the remote command (ssh back-pressure); there is no
  unbounded queue anywhere.

A single-host run to a pipe stays byte-identical to the command's output.

## Status

`goway status` prints for every host:
- the current address and how it was found
- arch and cores
- RAM (available/total), GPUs (model, memory, driver, CUDA), and notable
  features (`avx2`, `avx512f`, `neon`, `kvm`, `docker`), with the age of
  those facts
- load averages
- running goway jobs (against `max_jobs` if set)
- disk used by goway and disk free

Unreachable hosts are marked.

RAM and disk are probed live on every run. The rest (GPUs through
`nvidia-smi` or `rocm-smi`, CPU features, `/dev/kvm` readable and writable,
a working `docker info`) costs more on the host, so goway caches it per host
in its state file and probes it again daily or on `goway status --refresh`.
Values a host reports are bounds-checked before they are used.

Scheduling takes RAM into account: a host with less than
`defaults.mem_per_core` GiB (default 0.5) of available RAM per core scores
worse, by up to 1.0 in proportion to the shortfall, so a 16-core host with
3 GiB loses to a 12-core host with 7 GiB but is still used when it is the
only one. Set `mem_per_core = 0` to turn it off. `goway doctor` says when
Windows has an NVIDIA or AMD GPU that WSL cannot see (the fix is the
Windows driver with WSL support, never a Linux driver inside WSL).

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

A work directory a run has only just created (it has no lock file yet and is
under two minutes old) is never removed, so `gc` cannot race a run that is
starting.

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
  goway installed it and `~/.cargo` did not exist before). The record of
  what was installed holds only check names; the undo commands are goway's
  own, so an edited record cannot make uninstall run anything else
- goway's key line in `~/.ssh/authorized_keys`, with the previous modes
  restored

Administrator-level changes (the "no password login" setting) are
undone with `--rsudo`, after one question. System packages goway
installed, such as the C compiler, stay, because other software may
use them; goway lists them with their removal command. If a helper is
off, nothing on this laptop is removed, so you can run it again later.

On this laptop it then removes goway's config, keys and state, and
finally the goway program and its PATH line, using the install record
(which is only trusted for the goway binary, `~/.profile` and
directories under your home).
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
