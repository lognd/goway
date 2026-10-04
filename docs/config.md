# goway configuration

goway keeps three local files:

| File | Linux | Windows | Contents |
|---|---|---|---|
| config | `~/.config/goway/config.toml` | `%LOCALAPPDATA%\goway\config.toml` | defaults and the host pool (you edit this) |
| pinned keys | `~/.config/goway/known_hosts` | `%LOCALAPPDATA%\goway\known_hosts` | one ssh host key per host, stored under `goway-<name>` |
| state | `~/.local/state/goway/hosts.json` | `%LOCALAPPDATA%\goway\hosts.json` | last working address per host (a cache that is safe to delete) |

On Windows everything lives in the local profile (`%LOCALAPPDATA%`), never the roaming one
(`%APPDATA%`), because goway's unencrypted private key is kept there and the roaming profile is
copied to domain servers. An older `%APPDATA%\goway` directory is moved over on first run.

`GOWAY_CONFIG_DIR` and `GOWAY_STATE_DIR` override these directories.
`goway config path` prints the paths in effect. On Unix, ssh connection
sharing (ControlMaster) sockets live in `$XDG_RUNTIME_DIR/goway`. If the
socket path would exceed the 108-byte Unix limit, multiplexing is turned
off instead of failing.

## config.toml

Unknown keys are rejected, so a typo never silently changes behaviour.
`goway host add` and `goway host remove` edit the file in place and keep
your comments and layout.

```toml
[defaults]
remote_root = ".cache/goway"   # goway's remote state, relative to the remote home; must be a
                               # dedicated directory: not ~, /, or containing ..; its last
                               # component must contain "goway" (so .ssh or Documents are refused).
                               # goway only adopts a missing or empty directory, and purge removes
                               # only work/, seed/, cache/ and its marker, never foreign files
cache_ttl = "7d"               # seeds and per-repository caches expire after this idle time
orphan_ttl = "1d"              # unlocked work dirs left by crashed runs
kept_ttl = "3d"                # work dirs kept with `goway run --keep`
target_slots = 4               # most cargo target dirs per repository per host
# max_disk = "20G"             # most disk goway's remote root may use; default: the smaller of 20% of the host's disk and 50G
min_free = "10G"               # free space kept on the host's disk; over budget or below this, least recently used entries are evicted
cache_size = "2G"              # size cap of each repository's sccache and ccache (unless you set SCCACHE_CACHE_SIZE / CCACHE_MAXSIZE)
send_secret_files = false      # secret-looking files are never sent unless true
secret_allow = []              # ...or send these anyway, e.g. ["tests/fixtures/*.pem"]
keep = []                      # extra paths that stay in a build slot's tree between runs (below)
keep_ignored = true            # also keep every path the tree's .gitignore rules ignore (needs git on the host)
port = 2222                    # ssh port when a host does not set one (WSL sshd)
priority = "low"               # "low": jobs run under nice 10 with idle-class I/O; "normal"
# max_load = 0.8               # skip hosts whose 1-minute load per core is above this
# gpu_jobs = 1                 # GPU runs that may share each GPU (override per host)
# mem_per_core = 0.5           # GiB of free RAM per core below which a host scores worse (0 = off)
# owner_idle = "5m"            # a helper used or on battery within this counts as in use ("0s" = off)

[local]                        # this machine as a place to run; `--host local` works without it
# pool = false                 # true: compete with the helpers when goway picks hosts and shards
# fallback = false             # true: run here (with a note) when no helper is reachable
# max_jobs = 1                 # most goway jobs here at once when pooled
# margin = 0.5                 # added to this machine's score so helpers win unless it is clearly freer
# priority = "low"             # default: defaults.priority

[[host]]
name = "my-helper"             # the helper's computer name; also tried as my-helper.local
# address = "my-helper"        # optional: a DNS name or IP to try first
# port = 22
# user = "user"               # default: whatever ssh config says
# max_jobs = 2                 # skip this host while it runs this many goway jobs (default: every core, at least 1)
# priority = "normal"          # override defaults.priority for this host
# max_load = 0.5               # override defaults.max_load for this host
# gpu_jobs = 2                 # override defaults.gpu_jobs for this host
# labels = ["gpu-box"]         # names `--needs label=gpu-box` can ask for
# identity = "/home/me/.config/goway/id_ed25519"   # private key to offer (see below)
# os = "windows"               # "linux" (default; WSL helpers) or "windows" (PowerShell side)
# transport = "interop"        # "ssh" (default) or "interop": the Windows side of this machine
                               # from WSL through powershell.exe, no ssh (needs os = "windows";
                               # address, port, user and identity must be unset)
```

`os = "windows"` over ssh talks to a Windows machine's OpenSSH server (default
port 22, not 2222) with PowerShell as its shell, under the same pinned-key
rules as every other host. See docs/hosts.md.

`labels` name hosts for `goway run --needs label=NAME` and for rules in a
project's `goway.toml` (see [usage.md](usage.md)); they are 1-63 letters,
digits, `-` or `_`. Labels live only in this file, never in a project.

A project's own `goway.toml` `[toolchain]` section (not this file) can also
list `rust_targets` and `packages = { apt, dnf, pacman }` that `goway doctor`
checks and installs on the helpers; packages are root installs that need
`--rsudo` and a confirmation naming the repository as their source. The
name syntax, the `rust-toolchain.toml` targets and the fixes are in
[usage.md](usage.md).

Without `identity`, ssh offers every key in your agent and default key
files to the helper, one after another, so the helper learns their
fingerprints and comments and a full agent can exhaust the helper's
`MaxAuthTries`. With `identity` set, goway offers only that key
(`IdentitiesOnly=yes`). `goway ssh setup` sets it when it creates goway's
own key; for a host you set up yourself, set it to the key you chose.

### Slot trees and `keep`

A build slot's source tree persists between runs and is updated in place,
so dependency and build directories stay warm. Before each run, files that
are in neither your work tree nor the keep set are removed. The keep set is:

- detected directories: `node_modules`, `.next`, `.nuxt` (package.json);
  `.venv`, `venv`, `.tox`, `.nox`, `__pycache__`, `.pytest_cache`,
  `.mypy_cache`, `.ruff_cache` (pyproject.toml, requirements*.txt);
  `build`, `cmake-build-*` (CMakeLists.txt); `target` (pom.xml);
  `.gradle`, `build` (build.gradle, build.gradle.kts). Cargo adds nothing,
  because the cargo target dir is separate.
- your `keep` list. An entry without `/` is a name or glob matched at any
  depth (`node_modules`); one with `/` is a path glob relative to the tree
  root (`out/cache.bin`; write `./build` to anchor a single name).
  Absolute entries and `..` are rejected.
- unless `keep_ignored = false`: every path ignored by the tree's
  `.gitignore` files (checked with `git check-ignore --no-index`, using
  only the ignore files in your work tree). This matches what you see
  locally.

### Disk and memory on helpers

Each repository costs a helper one seed (a copy of the tree, hard-linked
into runs), one source tree per build slot, and one cargo target dir per
slot (`target_slots`, default 2). A large Rust workspace can need several
GiB per slot, so a repository on a helper costs roughly
`target_slots` x (tree + target). Memory is whatever the commands use: a
parallel build uses a few GiB per core.

Caps, all on the laptop side:

- `max_jobs` (per host, default every core, at least 1) bounds the goway
  jobs at once, which bounds concurrent memory use. Jobs run at nice 10 with
  idle I/O, so a full helper still yields to whoever sits at it (and extra
  nicely while its owner is using it, see below). Set `max_jobs` lower on a
  small helper or one whose RAM per core is thin.
- `max_load` skips a host whose load per core is above the limit, and
  `mem_per_core` scores hosts with little free RAM worse.
- `target_slots` bounds the target dirs per repository; `cache_ttl` and
  `goway gc` (see usage) remove idle caches, `goway status` shows the disk
  goway uses on each host.

Durations use humantime syntax: `90s`, `30m`, `12h`, `7d`.

## Environment variables

| Variable | Effect |
|---|---|
| `GOWAY_CONFIG_DIR`, `GOWAY_STATE_DIR` | override the local directories |
| `GOWAY_REPORT` | same as `goway run --report FILE` |
| `GOWAY_LOG` | tracing filter, such as `goway=debug` (`-v`, `-vv` and `-vvv` raise the level too) |
| `NO_COLOR` | no color in goway's own output (`--color` overrides) |
| `GOWAY_WINDOWS_LOOKUP=0` | under WSL, never ask Windows (`powershell.exe`) to resolve names |

On the remote, a job sees these variables:

| Variable | Value |
|---|---|
| `GOWAY` | `1` |
| `GOWAY_HOST` | the host's hostname |
| `GOWAY_RUN_ID` | the run's id |
| `CARGO_TARGET_DIR` | a free per-repository target slot, unless already set |
| `RUSTC_WRAPPER`, `SCCACHE_DIR`, `SCCACHE_SERVER_UDS`, `SCCACHE_IDLE_TIMEOUT` | sccache with a per-repository cache, a server socket in goway's owner-only cache directory (no TCP port), and a 300 s idle timeout. These are set only when sccache is installed and `RUSTC_WRAPPER` is unset, and each one only if it is still unset. |

| `CMAKE_C_COMPILER_LAUNCHER`, `CMAKE_CXX_COMPILER_LAUNCHER` | `sccache`, or else `ccache`, when installed on the host (CMake 3.17+ reads these from the environment), so C and C++ builds compile from the per-repository cache. Set only if unset: `CMAKE_CXX_COMPILER_LAUNCHER=` (empty) switches it off. A `-DCMAKE_..._LAUNCHER` on the command line or in the project's CMakeLists always wins over the environment. |
| `CCACHE_DIR` | a per-repository ccache directory, when ccache (and no sccache) is the launcher and the variable is unset |
| `CPM_SOURCE_CACHE` | `cache/<repo-id>/cpm` in goway's remote root: one CPM.cmake download directory shared by all slots and worktrees of the repository, unless already set |

The remote environment, `~/.cargo/env` and `--env KEY=VALUE` values are
applied first. goway only fills in what is still unset, so your settings
always win (see docs/positioning.md).

## A helper whose owner is using it

Helpers are often somebody's laptop. goway never skips a helper because its
owner is using it, but it is considerate:

- a helper counts as **in use** when it runs on battery or its user touched
  the keyboard or mouse within `defaults.owner_idle` (default 5 minutes;
  `"0s"` turns all of this off);
- when the choice is otherwise close, an idle helper on mains power wins (an
  in-use helper scores a quarter of a load per core worse; it is never
  excluded, and `--host` still pins);
- a job on a helper in use runs at nice 19 with idle I/O instead of nice 10,
  and with at most half the helper's cores for builds: `CARGO_BUILD_JOBS`,
  `MAKEFLAGS=-jN`, `CMAKE_BUILD_PARALLEL_LEVEL` and `NEXTEST_TEST_THREADS`
  are set to that unless you set them (`--env`). goway says so in one line;
- `goway status` shows the state in the `owner` column: `idle`, `in use`,
  `on battery`, or `-` when it cannot be told.

How the state is read: power from the kernel's power supplies (WSL2 shows the
Windows laptop's battery) or `pmset` on a Mac; the idle time from Windows
through WSL interop (`powershell.exe`) or the HID idle time on a Mac. Where
interop is switched off (on purpose, on some helpers), on a plain Linux
helper, on a Windows helper or when the probe times out, the state is
**unknown**: it neither blocks nor penalises, and status shows `-`.
