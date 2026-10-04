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

### Windows-side administrator steps

Some steps need Windows administrator rights, not Linux root: on a native
Windows helper, authorizing goway's key; on a Windows main laptop, adding
the OpenSSH client. They go through the same two switches, and goway never
stores, asks for or types a password.

- `--rsudo` on a native Windows helper (`goway add NAME --rsudo`, also
  `goway ssh setup NAME --rsudo`): goway runs `goway-setup install --host
  --native --authorized-key ...` there, after one question (`--yes` skips
  it). It elevates through the helper's own Windows OpenSSH server as the
  administrator account you name with `--windows-admin USER`, whose key is in
  the helper's `administrators_authorized_keys` (unattended: nobody has to be
  at the helper). With no such account, goway says so and prints the exact
  command to run in an administrator PowerShell on the helper. The login is
  tried once, so a wrong key never counts toward a fail2ban limit.
- The elevated session never starts WSL: a step that touches a distro
  first checks `wsl --list --running` and stops when the distro is not
  running (start WSL from a normal terminal; see SECURITY.md). A step on a
  helper that runs WSL can also go through `goway-setup` started from that
  WSL, which shows Windows' own UAC prompt on the helper's desktop when
  someone is logged in there; no command uses that route yet.
- `--lsudo` on a Windows main laptop (`goway add NAME --lsudo`): the missing
  OpenSSH client is added through this laptop's own UAC prompt. (git needs
  no administrator: `winget install --id Git.Git -e`.)


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
   a slot's binaries always find the current tree where they expect it. The usual per-user tool directories (`~/.local/bin`, `~/.cargo/bin`, ...) are put on PATH; no startup file is sourced. `goway doctor` names any proxy variables (`HTTPS_PROXY`, ...) set on the helper, never their values. sccache is
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

### Rust on a Windows host

A host with `os = "windows"` (OpenSSH, or `transport = "interop"` for the
Windows side of this machine) runs commands natively on Windows with the
same `goway run -- cargo nextest run --workspace`. The toolchain is the one
rustup has as the host's default (`x86_64-pc-windows-msvc` on an Intel
machine, `aarch64-pc-windows-msvc` on an ARM one); goway never picks or
installs a target, and the `--report` file records what ran: `host`, `os`
(`windows`) and `arch` as the machine reports it. Every repository gets its
own cargo target directory under goway's directory on the Windows side
(`cache\<repository id>\target-<slot>`, `CARGO_TARGET_DIR` unless you set
it), kept between runs, so the second run of a build or test is warm and
only changed crates recompile. Idle caches expire like on any other host
(`cache_ttl`), and `goway gc` removes them on demand. The command's exit
code is goway's exit code.

#### Checking a Windows host

`goway doctor NAME` works on a Windows host the same way it does on any
helper, over the same transport `goway run` uses (ssh to the Windows OpenSSH
server, or `powershell.exe` through WSL interop for the Windows side of this
machine). It checks how goway reaches the host, the Visual C++ Build Tools
(msvc linker), rustup, rustup's `windows-msvc` target, cargo-nextest (also
when it is only a bare file in the cargo `bin` directory, copied there by
hand) and the free space of the system drive. Each problem names the exact
PowerShell command that fixes it (`goway doctor NAME --explain CHECK`). Only
problems are listed unless you pass `--all`.

#### Fixing a Windows host

`goway doctor NAME --fix` installs what a Windows host lacks, after one
confirmation that lists every step with its exact PowerShell (`-y` skips the
question):

- the Visual C++ Build Tools with the C++ workload, through winget
  (`Microsoft.VisualStudio.2022.BuildTools`); this is the one step that needs
  Windows administrator rights;
- rustup, from a pinned `rustup-init.exe` whose sha256 is checked before it
  runs, with the msvc host toolchain (or, when rustup is there without an msvc
  toolchain, `rustup toolchain install`);
- cargo-nextest, from a pinned zip whose sha256 is checked, copied into the
  user's cargo `bin` directory. A cargo-nextest that is already there as a
  bare file counts as present and is left alone.

The steps that need no administrator rights always run as the host's user.
The administrator step runs only with `--rsudo`, through the best route
there is, each tried once: `--windows-admin USER` (a Windows administrator
account whose key is in the host's `administrators_authorized_keys`, so
nobody has to be at the host), else the UAC prompt (for the Windows side of
this machine, the prompt on this desktop), else goway stops and prints the
exact command to paste into an administrator PowerShell. goway never asks
for, stores or types a password.

Each install that a re-check confirms is recorded for `goway uninstall`:
rustup is removed with `rustup self uninstall` (kept when `~/.cargo` existed
before), the msvc toolchain with `rustup toolchain uninstall`, and
cargo-nextest by deleting the file (never `cargo uninstall`). The Build Tools
are a system package other software may use: they are listed, not removed.

#### Running a Windows test suite from WSL

With an interop host in your config (`transport = "interop"`, `os = "windows"`),
`goway run --host win -- cargo nextest run --workspace` and
`goway run --host win -- cargo clippy --workspace --all-targets -- -D warnings`
run natively on the Windows side of this machine, from any worktree, and
exit with the command's exit code. This replaces the old `winsync`, `winrun`
and `winbuild` scripts: there is no mirror to keep in step, and the copy is
updated incrementally by goway itself. Measured on an ARM Windows laptop
for a 4000-file, 1300-test workspace: clippy 281 s cold, nextest 213 s warm.

Two differences from a CI checkout are worth knowing. The Windows copy has
no `.git` unless the run asks for one (`--with-git`, see below), so tests
that open the repository fail there without it; and tests that run `sh` need
Git for Windows' `usr\bin` on `PATH`. goway handles the second itself: when
`sh` is not on the host's `PATH`, the run appends Git for Windows' `usr\bin`
(found from `git.exe`'s location, then the standard install paths, and only
if the directory exists) to `PATH`, never prepends it, and says so in the
run's notes on stderr.

### A `.git` on the helper (`--with-git`)

`goway run --with-git` (or `with_git = true` at the top of `goway.toml`)
gives the helper's copy a `.git` so tests that ask git about the repository
(`git status`, tracked paths, HEAD) behave as locally. goway builds it on
this machine, in its state directory (`git-meta/<worktree id>`), as a
one-commit shallow repository: HEAD's commit, tree and blobs (no history),
the real branch name, and a copy of the real index, so `git status` there
lists the same changes as here. It is rebuilt only when HEAD moves; its
files then travel like any other file of the tree, so they are synced
incrementally, verified like the rest and removed by gc with the copy. The
same on Linux and Windows hosts (on Windows `core.filemode` is off).

Never in it: remotes (and any URL with a token), credentials, hooks, user
config, reflogs, stashes, other branches, history, and the committed
content of secret-looking files (their blobs are left out of the pack). Of
your git config only `core.autocrlf`, `core.eol` and `core.safecrlf` are
copied. Staged content that is not in HEAD is not included, so
`git diff --cached` there cannot show it (`git status` can). A repository
with no commit yet is refused.

### Copy integrity

The copy of your tree on a helper is checked end to end, against your own
files, because a stale or damaged copy would make tests run the wrong code
without any sign of it. These checks guard against goway's own bugs; they
do not defend against a compromised helper (see SECURITY.md).

- **Every run.** After the helper has updated its slot tree and before
  your command starts, it reports the SHA-256 of every file this sync
  wrote there; goway compares them with your files. A mismatch stops the
  command before it starts.
- **A command that fails.** The helper then reports the hash of every
  synced file the command did not itself change, and goway compares them.
  If all match, the failure is genuine: it is reported with the command's
  exit code and never rerun. If any differ, goway names the files, rebuilds
  the slot, its cargo target dir and the helper's seed copy from scratch,
  and reruns the command exactly once.
- **Reruns terminate.** The attempt number is goway's own count, passed to
  the helper explicitly. The rerun is attempt 2, and attempt 2 can never
  rerun: if the rebuilt copy fails verification too, goway exits 125 with
  the evidence. Each shard of a sharded run reruns at most once.
- **After a proven mismatch,** that repository on that host gets full
  verification of every file and a fresh slot copy on every run, until 7
  days pass without a mismatch. The mark lives only in goway's local state
  (never in the repository or goway.toml). `goway gc --repo NAME` clears
  it; `goway run --trust-copy` skips it for one run (changed files are
  still checked).
- **Reporting.** A mismatch is a goway bug: goway says so and prints what
  to include in a report (host, repository id, paths and sizes, never file
  contents). With `--report FILE`, the JSON has an `attempts` list with
  both attempts, the first marked `"valid": false`.

A file you edit on this machine after the sync started is skipped by the
check, as is a file the command changed itself.

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
  `--list-tests`, `catch2-version` (measured in a real stripped v3.7.1
  binary; CI builds real GoogleTest and Catch2 binaries with CMake
  FetchContent and checks detection and sharding against them). A file with only some markers is not
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

Minimums (`cores`, `mem`, `gpu-mem`, `disk`, `cuda`) can be written
`cores:8`, `mem:2G` or `cuda:12.1`, which mean the same as `cores>=8` and
need no quoting. An unquoted `cores>=8` is read by the shell as the word
`cores` plus a redirect into a file named `=8`; goway then sees a bare
`cores`, says so, and names that file so you can delete it.

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

### Which OS runs it: the same one, unless you say so

A plain `goway run` (and every shard of `--shard N`) only uses hosts of the
laptop's own OS family (`linux`, `darwin` or `windows`; WSL counts as
Linux). `goway status` marks the hosts the default pool uses. Ask for more:

- `--any-os`, or `cross_os = true` at the top of `goway.toml`: hosts of
  every OS are candidates; each OS keeps its own caches, and reports and
  shard lines name the OS.
- `--needs os=windows`, or `--host NAME`: that OS or host, as before.
- `--each-os`: the whole command runs once on the best host of each OS in
  the pool, in parallel. Lines are prefixed `[host os]`, a summary lists
  each OS's exit code, and goway exits with the first failing OS's code
  (125 if goway itself failed there). It cannot be combined with `--host`
  or `--shard`.

For a recognized portable runner (`cargo build/test/nextest/clippy`,
`pytest`, `go test`, `npm/pnpm/yarn test`, `vitest`/`jest`, `mvn`,
`gradle`, `dotnet test`, `ctest`) goway prints a prominent warning when
hosts of another OS are configured: the run stays on the laptop's OS, and
the warning names those hosts and the two ways to allow it. `cross_os =
false` in `goway.toml` silences it. Other commands stay on the same OS
without a word. The warning reads the config's `os`, not live reachability.

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

### Nested goway is bounded

Every `goway run` sets `GOWAY_DEPTH` (its own depth plus one) and
`GOWAY_CHAIN` (the machines the chain of runs passed through) for the
command, on the helper as well as locally. A goway that finds
`GOWAY_DEPTH=4` refuses to run anything: it exits 125 and names the chain,
so a command that calls goway recursively always terminates. A non-numeric
`GOWAY_DEPTH` counts as the limit. Note that each level of nesting holds
its own build slot and job slot while it waits for the next, so deeply
nested runs of one repository need `target_slots` above the depth.

goway's own background work never starts runs: the automatic gc after a run
only deletes expired entries, and `gc.lock` in the remote root keeps it to
one automatic gc per host root at a time.

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

### A helper stays awake while a job runs

A laptop helper that suspends on idle would drop the job mid-run. So the
job runs under a sleep inhibitor held for exactly its lifetime:
`systemd-inhibit --what=sleep:idle` on Linux (when logind accepts it; a
WSL without systemd just goes without), `caffeinate -i -m -s` on macOS.
The inhibitor is the job's outermost wrapper, so it is released however
the job ends: exit, signal, or the watchdog's kill after a lost
connection. It does not hold off a closed lid or an empty battery.

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
- disk used by goway against its budget (`max_disk`, marked `(over)` past it) and disk free

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

### C and C++ builds

On a host with sccache (preferred) or ccache, goway sets `CMAKE_C_COMPILER_LAUNCHER`
and `CMAKE_CXX_COMPILER_LAUNCHER` in the job's environment, so a CMake build in a
fresh slot or worktree compiles from the repository's compiler cache. goway
installs sccache for Rust (`goway doctor --fix`) but never installs ccache; a
host without either simply has no launcher. `CPM_SOURCE_CACHE` points at one
directory per repository, shared by every slot, so CPM.cmake downloads once.

goway only fills in what is unset. A launcher or `CPM_SOURCE_CACHE` you set
(in `--env`, the host's environment, or on the cmake command line) is left
alone, and `CMAKE_CXX_COMPILER_LAUNCHER=` (empty) turns the launcher off.

Plain `FetchContent` downloads into the build directory (`build/_deps`), which
stays in the slot's tree between runs, so each slot downloads once and then
stays warm. To share downloads across slots and worktrees, set
`FETCHCONTENT_BASE_DIR` yourself, for example in your CMakeLists or with
`-DFETCHCONTENT_BASE_DIR=$HOME/.cache/deps`. goway never adds it to your
command line, because it is a cmake variable and changes where your project
looks for its sources.

### The disk budget

A helper's disk is not goway's to fill. Each host has a budget: goway's root
may use `max_disk` (default: the smaller of 20% of the host's disk and
50 GiB) and the disk keeps `min_free` free (default 10 GiB). After a run, and
on every `goway gc`, a host over either limit evicts unlocked entries, least
recently used first, until both hold: build slots (a slot's tree with its
target and build directories, aged by when it was last used), work dirs,
seeds of idle worktrees, then whole per-repository caches. An entry in use is
never touched: the same locks as the TTL gc are taken first, a locked slot is
reported `busy` and skipped, and ages are read after the lock is held.

`goway gc --dry-run` lists what eviction would remove (`would evict`, kind
`slot`, `work`, `seed` or `cache`). The eviction that follows a run is
detached, so its summary ("evicted 2 entries, freed 3.1 GiB") is printed by the
next run on that host. Note that `min_free` counts the whole disk: on a host
whose disk is nearly full for other reasons, goway frees everything it is
allowed to. On a small disk `min_free` is capped at a quarter of it, so a
4 GiB tmpfs is not emptied after every run.

sccache and ccache get a size cap per repository (`cache_size`, default 2 GiB)
through `SCCACHE_CACHE_SIZE` and `CCACHE_MAXSIZE`, unless you set them
(`--env` or the host's environment).

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
goway doctor                 # every host: problems only
goway doctor --all           # also the passing checks
goway doctor --explain mold  # the long text for one check
goway doctor --configure     # CMake project: also one traced configure on each helper
goway doctor <YOUR-COMPUTER-NAME-HERE> --fix          # run the fixes that need no root
goway doctor <YOUR-COMPUTER-NAME-HERE> --fix --rsudo  # also the administrator fixes
goway doctor --fix --rsudo --harden                   # and the optional hardening
```

### What doctor prints

Doctor is quiet by default. It prints one summary line per host (how
many problems and how many checks passed), then one table with each
problem once. A problem several hosts share is one row that names those
hosts (`all 3 hosts` when every host has it), failures before warnings.
Passing checks appear only with `--all`. Long explanations, such as the
one-run workaround for a linker the project's cargo config asks for, appear
only for one named check, with `--explain CHECK`, together with the exact
fix command.

With `--fix`, the fixes are listed once in plain words with the hosts
that need them. System changes need `--rsudo`: for each host goway
says in plain words what will change there and asks `[s]how exact
commands / [y]es / [N]o`. The exact root script is always shown before it
runs (with `--yes` it is shown and runs without asking). Everything runs
in one sudo session on that host, as separate steps: a step that fails
does not stop the others, each step reports its own result, and goway
checks again afterwards and records only what it verified as fixed.
`apt-get update` runs once per session. sudo asks for the password
itself; goway never sees it.

#### Fleet drift

When the project's tools are probed, doctor also keeps what it saw. It
prints a versions table, a row per tool and a column for this laptop and
each host, with `--all` and whenever the hosts disagree. Hosts disagree
(marked `DRIFT`) when a tool has a different major version on two hosts, or
a different minor version for a compiler (gcc, g++, clang, rustc, go, java,
dotnet, ...) or a tool pinned in `goway.toml` `[toolchain]`. A tool that is
missing or below the project's minimum is an error in the problem table,
with its fix; a missing tool is not drift. The laptop column is for
comparison only and never counts as drift. `goway doctor --fix
--all-hosts` says explicitly that every configured host is fixed (the
default when no HOST is named) and refuses a HOST. The versions are cached
in goway's state (with the time they were captured) so later runs can use
them.

A run uses that cache without probing. When the host it picked differs
from the others for a tool the project uses, goway prints one note
(`tool versions differ across your hosts: gcc 13.2.0 here (major: 13 vs
12) ...`, with how long ago doctor saw them). `--report` records those
versions under `tool_versions` (with `source` and the time they were
captured), so frob evidence says what built and tested the run. The cache is
per repository: run `goway doctor` in the project to refresh it, and a run in
another project does not use it.

Hardening, such as turning off ssh password login, is not needed to run
anything. It is listed separately as optional and is applied only with
`--harden` (which needs `--rsudo`), with its own question, never in the
same confirmation as tool installs.

Checks:
- goway's remote prerequisites: bash, tar, flock, setsid
- curl, which the fixes use for downloads
- the C linker cargo needs
- cargo/rustup, cargo-nextest and sccache
- disk space
- whether sshd still allows password logins
- the local ssh setup

### What your project needs

Run inside a project, doctor reads the project's own files and checks
each host for what they require, and only that (a C++ project is never
asked for cargo; outside a project, or in one goway does not recognise,
the Rust-first checks above apply):

| Files | Checked |
| --- | --- |
| `Cargo.toml` | cargo, nextest, sccache |
| `pyproject.toml`, `requirements*.txt`, `uv.lock` | python3 (and `requires-python`), uv, pytest when declared |
| `package.json`, `.nvmrc` | node (and `engines.node`), npm, pnpm or yarn from `packageManager` or the lockfile |
| `pom.xml`, `build.gradle(.kts)` | java, mvn or gradle (not when `mvnw` or `gradlew` exists) |
| `CMakeLists.txt`, `Makefile` | cmake against `cmake_minimum_required`, cc and c++ per `project(... LANGUAGES ...)`, make (ninja when `CMakePresets.json` asks for it), git when FetchContent or CPM fetch from git, ccache (optional) |
| `go.mod` | go (and its `go` line) |
| `Gemfile` | ruby (and `.ruby-version`), bundle |
| `*.csproj`, `*.sln`, `global.json` | dotnet |

Cargo, pyproject, package.json, global.json and CMakePresets.json go
through real TOML and JSON parsers. `CMakeLists.txt` has no declarative
form, so it is read as text (bounded) and its findings are labelled
approximate.

#### Asking each ecosystem's own tool

Where an ecosystem has a tool that answers, doctor asks it instead of
reading text:

| Ecosystem | Asked | Falls back to |
| --- | --- | --- |
| Rust | `cargo metadata --no-deps --offline` for the greatest `rust-version` (checked as `rustc`) | the root `Cargo.toml`, parsed as TOML, labelled approximate |
| Go | `go list -m -json` (offline, no toolchain download) for the `go` version | the `go` line of `go.mod`, labelled approximate |
| .NET | `global.json` parsed as JSON; the SDK itself is checked on each host | |
| Java | the `pom.xml` or Gradle files, read as text | always labelled approximate |

When a tool is missing on this laptop, the result is labelled
`approximate` and says to install that tool first for an exact answer.
doctor never runs `mvn` or `gradle` for this: both execute the project's own
plugins and build scripts, which a diagnostic must not do on your laptop. Every
tool doctor does run is started without a shell, with a time limit and a cap on
its output, and `cargo` is told not to install a toolchain.

#### CMake projects: what CMake itself says

For a project with a `CMakeLists.txt`, doctor asks CMake rather than reading the
file as text, and only on helpers (never on this laptop, never in your work
tree): see [cmake.md](cmake.md). `goway doctor` reads the File API replies the
helpers' slot trees already hold; `goway doctor --configure` also runs one
traced configure of a snapshot on each helper, in goway's own labelled scratch
directory, and names every package a `find_package` could not find with the
exact install command.

#### The linker cargo will use

A Rust project may name a linker or a linker backend in cargo's own
configuration, for example `linker = "clang"` with
`rustflags = ["-C", "link-arg=-fuse-ld=mold"]` under
`[target.x86_64-unknown-linux-gnu]`. doctor reads every `.cargo/config.toml`
cargo would read (the project directory and each parent, then
`$CARGO_HOME`), with cargo's precedence, plus `CARGO_TARGET_<TRIPLE>_LINKER`,
`CARGO_TARGET_<TRIPLE>_RUSTFLAGS` and `RUSTFLAGS` from your environment. For
each host's own target triple it then checks the linker program and the
backend (`mold`, `lld` as `ld.lld`) on that host, and a missing one is an
error whose fix is the system package (`goway doctor --fix --rsudo`, shown
in full and confirmed, installed by apt, dnf or pacman and recorded by check
name only; packages are listed by `goway uninstall`, never removed).

`goway doctor --explain mold` (or `clang`) also names a one-run
override, which goway never applies by itself:

```sh
goway run --env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc \
          --env "RUSTFLAGS=-C link-arg=-fuse-ld=lld" -- cargo nextest run
```

A non-empty `RUSTFLAGS` replaces the configured rustflags, and Rust's
bundled lld needs nothing installed. Two things that do not work:
cargo-nextest's inner `cargo test` ignores `--config`, and an empty
`CARGO_TARGET_*_RUSTFLAGS` does not override config rustflags.

`goway.toml` can pin or add tools; these are checked like detected ones
and win over a detected version:

```toml
[toolchain]
cmake = ">=3.24"      # at least
gcc = "14"            # starts with 14
tools = ["protoc"]    # must exist, any version
```

Tool names must be plain (letters, digits, `._+-`): they go into a
command on the host.

A repository can also ask for Rust targets and distro packages, such as the
cross-compiler a CI step needs on the helpers:

```toml
[toolchain]
rust_targets = ["x86_64-pc-windows-gnu"]
tools = ["x86_64-w64-mingw32-gcc"]

[toolchain.packages]
apt = ["gcc-mingw-w64-x86-64"]
dnf = ["mingw64-gcc"]
pacman = ["mingw-w64-gcc"]
```

`rust-toolchain.toml` `targets = [...]` count as `rust_targets` too, and its
`channel` names the toolchain the targets are checked for (the default
toolchain when it names none). `goway doctor` checks each target and package
on every Unix helper, as rows `target:TRIPLE` and `pkg:NAME` (a package
manager other than apt, dnf or pacman gets one warning row). `--fix` adds a
missing target for your user only (`rustup target add --toolchain CHANNEL
TRIPLE`). A missing package is a root fix: it needs `--fix --rsudo`, and the
confirmation lists every package and names the repository and `goway.toml`
as the source before asking, then shows the exact commands. A package list
is validated against the package manager's name syntax (apt: lower-case
letters, digits and `+-.`; dnf: letters, digits and `+._-`; pacman: letters,
digits and `@._+-`; never starting with `-`), so a repository can ask for
package names and nothing else: an option, a path, a space or a shell
character is a config error and nothing runs. Any other key under
`[toolchain.packages]` is an error too.

Fixes for these tools prefer a user-level install: pinned releases of
uv, go, cmake, node, a Temurin JDK and Maven (and mold where the
distribution has no package for it, Ubuntu before 22.04 and Debian
before 12), each verified by checksum
before it is unpacked into `~/.local/opt/goway-TOOL` and linked into
`~/.local/bin` (put that on `PATH`); corepack for pnpm and yarn; a user
gem for bundler. Everything else (compilers, make, ninja, git, ccache,
python3, ruby, dotnet, ...) is a system package for apt, dnf or pacman,
run through `--rsudo`. Every install is recorded for `goway uninstall`
by check name only; the undo command is derived from the name, never
read from the record.

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

- The git-visible work tree (see Sync above), without `.git` unless you
  ask for `--with-git` (one-commit shallow history and the index, never
  remotes, credentials or hooks; see above).
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
