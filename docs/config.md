# goway configuration

goway keeps three local files:

| File | Linux | Windows | Contents |
|---|---|---|---|
| config | `~/.config/goway/config.toml` | `%APPDATA%\goway\config.toml` | defaults and the host pool (you edit this) |
| pinned keys | `~/.config/goway/known_hosts` | `%APPDATA%\goway\known_hosts` | one ssh host key per host, stored under `goway-<name>` |
| state | `~/.local/state/goway/hosts.json` | `%LOCALAPPDATA%\goway\hosts.json` | last working address per host (a cache that is safe to delete) |

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
remote_root = ".cache/goway"   # goway's remote state; relative to the remote home
cache_ttl = "7d"               # seeds and per-repository caches expire after this idle time
orphan_ttl = "1d"              # unlocked work dirs left by crashed runs
kept_ttl = "3d"                # work dirs kept with `goway run --keep`
target_slots = 4               # most cargo target dirs per repository per host
send_env_files = false         # .env and .env.* are never sent unless true
port = 2222                    # ssh port when a host does not set one (WSL sshd)

[[host]]
name = "helios"                # identity; also tried as helios.local
# address = "Helios"           # optional: a DNS name or IP to try first
# port = 22
# user = "user"               # default: whatever ssh config says
# max_jobs = 2                 # skip this host while it runs this many goway jobs
```

Durations use humantime syntax: `90s`, `30m`, `12h`, `7d`.

## Environment variables

| Variable | Effect |
|---|---|
| `GOWAY_CONFIG_DIR`, `GOWAY_STATE_DIR` | override the local directories |
| `GOWAY_REPORT` | same as `goway run --report FILE` |
| `GOWAY_LOG` | tracing filter, such as `goway=debug` (`-v`, `-vv` and `-vvv` raise the level too) |
| `NO_COLOR` | no color in goway's own output (`--color` overrides) |

On the remote, a job sees these variables:

| Variable | Value |
|---|---|
| `GOWAY` | `1` |
| `GOWAY_HOST` | the host's hostname |
| `GOWAY_RUN_ID` | the run's id |
| `CARGO_TARGET_DIR` | a free per-repository target slot, unless already set |
| `RUSTC_WRAPPER`, `SCCACHE_DIR`, `SCCACHE_SERVER_PORT`, `SCCACHE_IDLE_TIMEOUT` | sccache with a per-repository cache, port and a 300 s idle timeout. These are set only when sccache is installed and `RUSTC_WRAPPER` is unset, and each one only if it is still unset. |

The remote environment, `~/.cargo/env` and `--env KEY=VALUE` values are
applied first. goway only fills in what is still unset, so your settings
always win (see docs/positioning.md).
