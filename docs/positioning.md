# goway: niche and coexistence

This page states what goway is for and how it differs from other
distributed build and execution systems. It also lists the rules goway
follows so it never fights them, and names the mechanism that enforces
each rule. The evidence behind the comparisons is in docs/prior-art.md.

## The niche, in one sentence

goway places **whole commands** from a **dirty git work tree** onto
**personal machines you already own**, Windows laptops with WSL that move
between DHCP addresses and that people also use. It needs no daemon, no
root, no containers and nothing installed on the remote beyond ssh and
coreutils, and it keeps each repository's build caches warm.

Five properties together define the niche. Each comparison below fails
at least one of them.

1. **Unit of work = a whole command** such as `cargo nextest run`,
   `make test` or `pytest`. It is not a single compiler invocation or a
   hermetic action.
2. **Input = the work tree as the user sees it**: uncommitted edits and
   untracked files included, ignored files excluded. No commit, push,
   image or archive step comes first.
3. **Hosts = personal machines** with no static IP, no admin team, other
   users and other workloads. A host's identity is its name plus a
   pinned ssh key, never its address.
4. **Zero footprint**: no agent, service, coordinator or root on the
   remote. The remote side is one bash script sent with each call.
5. **Many concurrent worktrees** (for example, agent swarms) share warm
   per-repository caches, and goway cleans up after itself without being
   asked.

## How it differs

| System | What it distributes | Why it is not goway's niche |
|---|---|---|
| sccache-dist, distcc, icecream | single compile steps | Linking, build scripts, proc-macros and tests stay local. Client and server need the same architecture or shipped toolchains, which here means aarch64 to x86_64 cross-compilation. Needs a scheduler daemon and root build servers. |
| Bazel/REAPI (BuildBuddy, NativeLink, Buildbarn) | hermetic actions | You must adopt Bazel and declare every input. Cargo cannot speak REAPI. Workers are dedicated. |
| Docker contexts over ssh | image builds and containers | Needs a daemon on every host. Bind mounts of the local tree do not work remotely. One context is one host, with no placement. Prune is global. |
| CI runners (GitHub Actions, act) | workflow jobs | Needs a commit and push, or workflow YAML. Placement is by labels, not load. Runners are registered services. |
| GNU parallel `--sshlogin` | independent jobs over file lists | No git-aware tree, no persistent per-repository caches, and only a load threshold (no least-loaded choice). |
| cargo-remote, rsync_cmd | one command on one host | One fixed host and no pool. Workdirs collide across worktrees. No automatic cleanup. Cargo or rsync only. |
| Nix remote builders | derivations | Nix-only and hermetic, with the Nix daemon on every builder. |

So goway is **placement plus transport for unmodified commands**. It
builds nothing itself and caches nothing it does not own. Above all, it
runs other build systems rather than replacing them.

## Coexistence rules and how each is enforced

| Rule | Mechanism |
|---|---|
| goway never owns a host: no daemons, services or root on the remote. | The remote side is `remote.sh`, sent inline per call. Host setup (firewall, keepalive, sshd) is a separate, opt-in, journaled installer that `uninstall` reverses exactly (docs/install-windows.md). |
| All goway state lives in one labelled directory and expires on its own. | `remote_root` (default `~/.cache/goway`). Every entry carries a `meta.json` label and a `flock` lock. TTLs come from the config. `goway gc` runs automatically after runs. |
| No process outlives its run (except sccache's server, which keeps the cache warm and exits after `SCCACHE_IDLE_TIMEOUT`, 300 s by default). | Each job runs in its own session. A watchdog kills the job's process group when the ssh session dies, and the work directory is removed on exit. Tested by interrupting a live `sleep 300` on a real host. |
| Scheduling respects everyone's load, not only goway's. | The score uses the OS load average, which includes CI runners, sccache-dist servers and the human at the keyboard, plus goway's own running jobs: (load1 + goway jobs) / cores. Hosts at `max_jobs` are skipped. |
| goway is a polite guest on personal machines. | `priority = "low"` (the default) runs jobs under `nice -n 10` with idle-class I/O, so the person using the laptop comes first. `max_load` skips hosts whose load per core is already above a ceiling unless `--host` pins them. Tests: `low_priority_jobs_run_niced_with_idle_io` and `hosts_above_max_load_are_skipped_unless_pinned`. Not done yet: noticing that the owner is active or that the laptop runs on battery. |
| goway never hijacks another tool's configuration. | goway only fills in what is unset. The remote environment, `~/.cargo/env` and `--env` values come first. If `CARGO_TARGET_DIR` is already set, goway uses it and skips its target slots. sccache is enabled only when `RUSTC_WRAPPER` is unset (an empty `--env RUSTC_WRAPPER=` turns it off). `SCCACHE_DIR` and `SCCACHE_SERVER_PORT` are kept when present. goway's sccache uses a per-repository port in 4300-5299, so it never takes over a user's server on 4226. Tests: `user_settings_win_over_goway_defaults` and `a_run_leaves_no_files_outside_its_root_and_no_processes`. |
| goway composes rather than replaces. | goway is a command prefix with a faithful exit code: the command's own code, 128+N on signals, 125 for goway failures. It runs unchanged inside make, CI, GNU parallel or frob. `--report` writes host, arch and address as JSON for evidence records. Other systems run through it unchanged: `goway run -- bazel test //...` keeps Bazel's own remote cache, and `goway run -- docker build .` uses the remote's daemon. |
| goway does not send secret-looking files by default. | Only the git-visible tree is sent. Env files, credential files, private keys and key stores are matched case-insensitively and kept back unless allowed in `secret_allow`. Files under symlinked directories are never read, and keys stay with ssh and its agent. This is a denylist, so a secret with an unusual name that you track in git is still sent. |

## Where goway deliberately stops

- **No cross-compilation and no artifact copy-back.** The remote builds
  for its own architecture, and the results are the exit code and the
  streamed output.
- **No action-level caching** beyond what the command's own tools do
  (cargo's target dir, sccache). Teams that need hermetic remote
  execution should use Bazel/REAPI, and they can still use goway to
  place the `bazel` invocation itself.
- **No cluster management.** The pool is a list in a config file.
  Machines join with `goway host add` and leave with `goway host remove`.

## Planned work that widens the niche without leaving it

- Test sharding across hosts through nextest's own `--partition`
  (the multi-host sharding story). goway uses nextest's partitioning
  instead of inventing its own.
