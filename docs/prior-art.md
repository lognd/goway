# goway prior-art check (2026-10-03)

Method: web search plus `gh api` repo metadata (stars, pushed_at, latest release)
fetched 2026-10-03. Items tagged [unverified] come from background knowledge or
a single search snippet and were not confirmed against primary docs.

## Short answer

Not reinventing Docker: Docker-over-SSH solves a different problem (image-based
isolation) and is a poor fit for "native build, warm per-repo cargo target on a
bare WSL2 host". The closest real prior art is the cargo-remote family
and rsync_cmd, which are single-host, single-command, and have no scheduling,
no parallel-safe workdirs, no cache GC. Nothing found does the full set:
git-aware sync + load-based host pick + per-repo caches + automatic cleanup +
many concurrent worktrees.

## Candidates

### 1. Docker contexts over SSH (DOCKER_HOST=ssh://, docker context create --docker host=ssh://)
- What: local docker CLI drives a remote daemon over SSH; `docker build` uploads
  the build context (honours .dockerignore); `docker run` runs remotely;
  BuildKit cache and named volumes persist on the remote.
  https://ruan.dev/blog/2022/07/14/remote-builds-with-docker-contexts
  https://blog.crafteo.io/2023/02/20/efficient-docker-build-and-cache-re-use-with-ssh-docker-daemon/
- Closeness: medium for the "sync + run + stream + exit code" part; the remote
  needs a docker daemon (more setup on each WSL2 box; user must be in docker group).
- Maintenance: Docker itself is very active (not re-verified here).
- Lacks:
  - Bind mounts of the local tree do NOT work remotely; relative volume mounts
    do not sync (https://github.com/docker/compose/issues/6697). So you must
    bake the source into an image via build context each run, or rsync yourself.
  - Source upload is a full context tar each build unless layered carefully;
    no delta sync of the working tree. Cargo target would have to live in a
    named volume or BuildKit cache mount (`RUN --mount=type=cache`), which works
    for `docker build` but is awkward for `cargo nextest run` as an arbitrary command.
  - No host selection or load awareness (one context = one host).
  - Cleanup is `docker system prune` / `builder prune`: global, not per-repo-aged.
  - Containers add overhead and WSL2-nested-docker friction; the remote is a
    different arch than the laptop so images must be x86_64 only (fine, but
    means a dedicated image and toolchain management).
- Verdict: good plumbing idea (context upload, named cache volumes), wrong
  default for this workload. Could be an optional goway backend, not the core.

### 2. cargo-remote (sgeisler) and forks
- sgeisler/cargo-remote: 212 stars, last push 2024-06-12; crates.io 0.2.0
  published 2021-04-24, ~7k downloads. rsync project to a remote temp dir, run
  one cargo command via ssh, optionally copy target back (-c).
  https://github.com/sgeisler/cargo-remote
- Apothic-AI/cargo-remote-3000: claims to be the maintained fork (all upstream
  PRs and issues addressed, workspace support via cargo metadata, --watch,
  fixes macOS rsync flag). 0 stars, pushed 2026-10-02, crates.io 0.4.0, ~10
  downloads (very new, little adoption). https://github.com/Apothic-AI/cargo-remote-3000
  https://github.com/sgeisler/cargo-remote/issues/27
- portlandhodl/cargo-remote: pure-Rust ssh (russh) plus built-in delta sync, no
  external ssh/rsync; 0 stars, pushed 2026-09-17. https://github.com/portlandhodl/cargo-remote
- joshvoigts/rcargo: rsync, streams output, respects .gitignore, optional
  Landlock sandbox via nono; 0 stars, pushed 2026-09-18. https://github.com/joshvoigts/rcargo
- Others seen: blob42/remote-cargo, rmcgibbo/cargo-remote, weikengchen/cargo-remote (forks/variants).
- Closeness: high for single-host cargo. Cargo-only (not "any command").
- Lacks: multi-host pool and load-based pick; git-aware file set (uses rsync
  filters, not `git ls-files`); safe concurrent runs of the same repo from
  several worktrees (upstream used a per-project remote dir [unverified:
  hash-of-path naming]); automatic stale-dir and cache cleanup; sccache
  integration; WSL2/aarch64-specific concerns. README says "hacky".
- Verdict: closest to adopt-able, but fork landscape is young/unproven.
  Read its source for ideas rather than depend on it.

### 3. rsync_cmd (ltratt)
- 11 stars, pushed 2026-05-15. `rsync_cmd HOST cmd...`: mirrors CWD with
  `rsync -az --delete-during`, adds `dir-merge,-n /.gitignore` plus global
  gitignore, then `ssh -t host "cd dir && cmd"`. Requires a `.rsync_cmd` marker
  file per synced dir (safety). https://github.com/ltratt/rsync_cmd
- Closeness: this IS the "sync and run any command" core, in about one file.
- Lacks: one host, remote dir is `~/<dirname>` (collides across worktrees with
  same basename), --delete-during deletes remote target/ unless you add
  excludes (gitignored target is excluded from sync but --delete may still
  remove it unless protected; [unverified, check rsync filter semantics]),
  no load pick, no cleanup, no structured exit/stream handling beyond ssh.
- Verdict: proof that the core is ~50 lines of shell; the gaps are exactly goway's value.

### 4. cargo-nextest archive / build reuse
- nextest is very active: 3314 stars, release cargo-nextest-0.9.146 on
  2026-09-21, pushed 2026-10-03. Docs: https://nexte.st/docs/ci-features/archiving/
- `cargo nextest archive --archive-file X.tar.zst` bundles test binaries; on
  another machine `cargo nextest run --archive-file X --extract-to D
  --workspace-remap W` runs them. `--partition count:i/N` shards tests.
  Source tree must be at same revision on target (fixtures). Archive is
  built on a machine of the SAME OS/arch as the runner.
- Remote runner: none found. Searches found only CI-matrix partition usage
  (https://nexte.st/docs/running/, https://docs.rs/nextest-runner/latest/nextest_runner/reuse_build/index.html).
- Closeness: complementary. Because goway builds natively on the remote,
  archive/reuse is not needed for the main flow; useful later for
  "build on beefy host, shard run across several hosts" (--partition).
- Lacks: any transport, scheduling, or sync. It is a building block.

### 5. Remote execution APIs (Bazel REAPI, BuildBuddy, NativeLink, Buildbarn)
- Active: NativeLink v1.7.3 (2026-10-01, 1607 stars), BuildBuddy v2.312.0
  (2026-10-01), Buildbarn bb-remote-execution (release 2026-09-30).
- These execute individual hermetic actions (cmd + declared input digests)
  on workers. Cargo cannot speak REAPI. Paths to use them: Bazel with
  rules_rust (reimplements the build in Bazel; huge migration, build.rs and
  proc-macros need care) [unverified detail]. sccache supports remote
  *storage* backends and its own sccache-dist, not REAPI as an executor
  [unverified whether newer sccache adds REAPI; I saw no evidence].
- Closeness: low. Wrong granularity (per-action, hermetic) vs whole commands
  with a mutable work tree; requires adopting Bazel.
- Verdict: reject for this scope.

### 6. sccache and sccache-dist, distcc, icecream
- sccache v0.18.0 (2026-09-14), 7748 stars, active. sccache-dist: scheduler plus
  build servers; build servers Linux-only, run as root with bubblewrap;
  clients can ship toolchains. https://github.com/mozilla/sccache/blob/main/docs/DistributedQuickstart.md
  https://android.googlesource.com/toolchain/sccache/+/HEAD/docs/DistributedQuickstart.md
  Distributed Rust compilation has caveats (linking and proc-macros/build
  scripts run locally; only rustc invocations are shipped) [partly
  unverified, see mozilla.dev.platform thread
  https://groups.google.com/g/mozilla.dev.platform/c/dbvbxe9Iy88/m/br44JFrKDwAJ].
- Hard blocker: sccache-dist ships compile jobs, but the client (aarch64) and
  workers (x86_64) differ in arch, so host-side build scripts/proc-macros
  and cross toolchains come in; this is exactly the cross-compile you want to avoid.
- icecream v1.4 (last release 2022-03-04, pushed 2026-03-04) and distcc v3.4
  (2021-05-11, pushed 2026-07-08): C/C++ compiler distribution; no rustc story
  to speak of. Neither runs "whole commands".
- Verdict: reject as the mechanism; plain sccache (local or shared-storage)
  stays useful as the remote per-repo cache goway should configure.

### 7. Mutagen plus ssh
- mutagen v0.18.1 (2025-02-24), pushed 2026-04-22, 4474 stars; owned by Docker
  since 2023-06-27. Continuous bidirectional or one-way sync, forwarding.
  https://mutagen.io/blog/mutagen-is-joining-docker/
- Closeness: sync engine only. Needs agent on remote; continuous sync with
  ignore rules (can use .gitignore VCS ignore). Ideal for edit-loop; heavier
  than needed for one-shot runs; no `git ls-files` semantics; no run/schedule.
- Verdict: possible sync backend; slows down many parallel worktrees
  (a daemon session per worktree) [judgement].

### 8. Devcontainers and DevPod
- DevPod (loft-sh) v0.6.15 last release 2025-03-10, last default-branch push
  2025-11-14; issue "Still Maintained?" #1915 open; community fork exists.
  SSH provider (Linux remotes only). https://github.com/loft-sh/devpod
  https://github.com/loft-sh/devpod/issues/1915 https://github.com/loft-sh/devpod-provider-ssh
- Closeness: low. Designed for long-lived dev environments with an IDE, not
  fire-and-forget command runs; heavy; docker-based; upstream stalled.
- Verdict: reject.

### 9. GNU parallel --sshlogin / --transferfile / --return / --cleanup
- Mature [unverified in this session; from documentation knowledge]. Can
  distribute jobs over hosts (`-S host1,host2`), transfer files, return
  results, clean up, and has `--load` / slot limits.
- Closeness: surprisingly good for scheduling and cleanup flags, but it
  transfers file lists you supply (not a git-aware tree), assumes parallel
  installed on remotes, and has no persistent per-repo caches (it is made
  for stateless jobs). Load check is per-host threshold, not "pick least loaded".
- Verdict: nice for batch fan-out of independent jobs; not a workspace runner.

### 10. Earthly and Dagger
- Earthly: company ended active maintenance and shut Earthly Cloud/Satellites
  2025-07-16; repo frozen to critical fixes (last release v0.8.16 2025-07-16);
  community fork "EarthBuild". https://earthly.dev/blog/shutting-down-earthfiles-cloud/
  https://earthly.dev/blog/shutting-down-earthly-ci/
- Dagger: very active (v0.21.10, 2026-09-30, 16k stars). Engine runs in a
  container; can point client at a remote engine (`_EXPERIMENTAL_DAGGER_RUNNER_HOST`
  style ssh/tcp) [unverified exact current flag name]. Pipelines are
  containerised, cache volumes are first-class.
- Closeness: low-medium. Both require pipelines as code in containers, the
  remote engine needs docker, no least-loaded pool.
- Verdict: reject Earthly (unmaintained); Dagger is heavy for "run cargo test".

### 11. GitHub Actions self-hosted runners and act
- act v0.2.89 (2026-06-01), 72k stars: runs workflow YAML locally in docker.
  Self-hosted runner: register each WSL2 box; a workflow_dispatch triggers
  the job. Both need workflow files and (for GH runners) a push to GitHub
  and round trip; no local-dirty-tree sync; runner queueing is GH's label-based
  matching, not load-aware [unverified]. Reject for a tight local loop.

### 12. Small "rsync and run over ssh" tools (search results)
- rsync_cmd, rcargo, cargo-remote family (above), de9uch1/git-rsync (sync only).
- cross-rs/cross has a "remote" mode (docker on remote host, copies via volume):
  https://github.com/cross-rs/cross/wiki/Remote (v0.2.5 last release 2023-02-04,
  still pushed 2026-09-24). It targets cross-compilation, not native builds.
- PyPI/crates.io searches for names like remote-run, rrun, offload, ssh-run
  were not systematically completed [unverified: only web search used; no
  direct PyPI/crates.io index queries beyond cargo-remote*, rcargo].
  Also not checked: Nix remote builders / `nix build --builders` (relevant
  prior art for host pool + load: builders have `speedFactor`, `maxJobs`,
  `supportedFeatures`; documentation knowledge only, [unverified]), Bazel
  `--remote_executor`, Distcc-pump, Rust-specific `cargo-remote-*` on PyPI.

## Verdict

Build a thin tool, not a Docker clone. Do NOT adopt one off-the-shelf tool;
do borrow from several. Concretely: "build goway as a thin wrapper over
plain rsync/git plus ssh (the rsync_cmd/cargo-remote pattern), generalised to
any command and a host pool." Keep Docker as an optional later backend only.

### Gaps that justify building
1. Pool scheduling: no tool picks least-loaded host from several (cargo-remote,
   rsync_cmd, rcargo: single host; Docker: one context = one host; parallel:
   threshold only).
2. Any command, not only cargo, with exit code and stream fidelity.
3. Git-aware file set (tracked + modified + untracked-not-ignored via
   `git ls-files -co --exclude-standard`) instead of rsync filters; safe
   deletions that never touch remote caches.
4. Concurrency-safe per-repo and per-worktree remote workdirs (many parallel
   runs from several worktrees) with shared per-repo cargo target/sccache
   policy, plus locking where cargo's own target lock would serialise.
5. Automatic and on-demand GC of stale workdirs/caches (nobody found does
   this; Docker prune is global, cargo-remote leaves temp dirs).
6. Arch honesty: aarch64 laptop to x86_64 remote means NO copy-back of
   artifacts (cargo-remote's `-c` is useless here); goway is run-only.

### Design lessons to borrow
- cargo-remote: per-project stable remote dir so the remote target dir stays
  warm across runs; read target dir via `cargo metadata` (cargo-remote-3000
  workspace fix) rather than assuming `./target`; propagate the remote exit
  code unchanged; opt-in copy-back only.
- rsync_cmd: refuse to sync directories without an opt-in marker (avoid
  accidentally rsyncing $HOME); honour global gitignore; `-v/--dry-run`
  printing exact commands; use `ssh -t` only when stdin is a tty.
- rcargo: layered global+project TOML config; optional sandbox (Landlock);
  resolve $HOME on the remote.
- Docker: content-addressed, ignore-file-driven build context; named cache
  volumes separate from source; explicit `prune` verbs with filters
  (`--filter until=24h`) -> implement `goway gc --older-than`; BuildKit-style
  cache mounts keyed by repo id.
- nextest: `--partition count:i/N` plus archive/extract for later multi-host
  sharding of one test run; always pass `--workspace-remap` equivalents
  rather than hard-coding paths.
- Nix remote builders: per-host `maxJobs` and `speedFactor` for scheduling
  [unverified]; use as a model for the host config schema.
- sccache: set RUSTC_WRAPPER and SCCACHE_DIR per repo on the remote; do not use
  sccache-dist across arches.
- Transport: ControlMaster/ControlPersist ssh multiplexing for fast repeat
  invocations and cheap load probes (`cat /proc/loadavg`, `nproc`).

## Final report (five-line verdict)
1. Verdict: build a thin wrapper (git ls-files + rsync/ssh), not a Docker clone.
2. Closest prior art: cargo-remote family and rsync_cmd; single host, no pool, no GC.
3. Docker over SSH: no remote bind mounts, one host per context, global prune; optional backend only.
4. Rejected: REAPI/Bazel, sccache-dist (cross-arch), icecream/distcc, DevPod, Earthly, act.
5. Gaps: least-loaded pool, per-worktree safe dirs, warm caches, auto GC; borrow nextest --partition.
