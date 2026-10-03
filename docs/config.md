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
send_secret_files = false      # secret-looking files are never sent unless true
secret_allow = []              # ...or send these anyway, e.g. ["tests/fixtures/*.pem"]
keep = []                      # extra paths that stay in a build slot's tree between runs (below)
keep_ignored = true            # also keep every path the tree's .gitignore rules ignore (needs git on the host)
port = 2222                    # ssh port when a host does not set one (WSL sshd)
priority = "low"               # "low": jobs run under nice 10 with idle-class I/O; "normal"
# max_load = 0.8               # skip hosts whose 1-minute load per core is above this

[[host]]
name = "my-helper"             # the helper's computer name; also tried as my-helper.local
# address = "my-helper"        # optional: a DNS name or IP to try first
# port = 22
# user = "user"               # default: whatever ssh config says
# max_jobs = 2                 # skip this host while it runs this many goway jobs
# priority = "normal"          # override defaults.priority for this host
# max_load = 0.5               # override defaults.max_load for this host
# identity = "/home/me/.config/goway/id_ed25519"   # private key to offer (see below)
```

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

The remote environment, `~/.cargo/env` and `--env KEY=VALUE` values are
applied first. goway only fills in what is still unset, so your settings
always win (see docs/positioning.md).
