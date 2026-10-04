# goway design

"go away": run a command on another machine, natively, from the current git
work tree. This document is the problem tree; each leaf names the module or
ticket that owns it. Read docs/prior-art.md for why this is a new tool.

## 1. Facts measured on the target machines (2026-10-03)

- Hosts are Windows laptops with WSL2 (Ubuntu, x86_64, systemd on). sshd in
  WSL listens on 2222; the Windows OpenSSH server on 22 lands in cmd.exe.
- WSL uses `networkingMode = mirrored` (Windows 11 22H2+), so the Windows
  Wi-Fi address reaches the WSL sshd. On Windows 10, or WSL in NAT mode, a
  Windows port relay (`netsh interface portproxy`) forwards the port to WSL
  and a scheduled task re-points it when WSL's address changes. A scheduled task keeps the distro alive; a Windows firewall
  rule and the Hyper-V firewall allow inbound 2222.
- Addresses are DHCP leases in a /13 (192.0.2.0/13). They change, and the
  subnet is far too large to scan.
- `<COMPUTERNAME>.local` resolves over mDNS from Windows. A WSL client in NAT
  mode cannot resolve `.local` itself, but can ask Windows through interop
  (`powershell.exe Resolve-DnsName`, about 4 s, so results are cached).
  LLMNR also answers with unrelated adapters (VirtualBox 192.168.56.1), so a
  name lookup alone is not an identity.
- Non-interactive ssh does not read ~/.cargo/env; cargo is only on PATH in a
  login shell.
- Cargo stays Fresh when a workspace is copied to a new directory with mtimes
  kept and the same CARGO_TARGET_DIR (tested with path and registry deps).
  This makes per-run work directories plus a shared per-repo target cheap.

## 2. Problem tree

1. Identity and reachability (`hosts`, `resolve`)
   1. A host is a name plus a pinned SSH host key, never an IP. ssh always
      runs with `HostKeyAlias=goway-<name>`, `StrictHostKeyChecking=yes`
      and goway's own known_hosts file, so a wrong address is rejected by
      ssh itself.
   2. Address resolution chain, first that answers wins: cached last good
      address, configured `address` (name or IP), system resolver for the
      name, `<name>.local` via the system resolver, `<name>.local` via
      Windows interop when running under WSL.
   3. `goway host add NAME` discovers port (2222 then 22, requires `uname`
      = Linux), user, and pins the key on first contact (trust on first
      use, fingerprint shown).
   4. SSH auto-detection reads `ssh -G` (effective config, no secrets) and
      warns about weak or odd settings: password auth allowed, no agent
      keys and no identity file, identity file with loose permissions
      (stat only, never read), the Windows sshd on 22 instead of WSL.
2a. The Windows remote side (`remote.ps1`, PowerShell 5.1 and 7)
   1. Same verbs, protocol, labelled directories and output formats as
      `remote.sh` (manifest, hashes, deletions, changes, receive,
      verify-wait, verify-verdict, run, envfile, probe, gc, auto-gc,
      doctor, purge, ping), including persistent slot trees, keep sets,
      copy integrity, GPU slots (`gpu-slots:N`, locks under `gpu\`),
      shard detection (`shard-detect:I:N:NONCE`, PE and ELF magic plus
      every marker of one framework) and the detached automatic gc.
      Differences forced by Windows: a seed's executable bits live in a
      `modes` sidecar (NTFS has none); symlinks are not created (skipped
      with a note); "load" is the CPU load scaled to the core count; a
      lock is an open handle (`FileShare.None`), so gc takes the same
      locks and a locked entry is simply busy.
   2. The script is about 2000 lines, far over the few thousand
      characters a Windows OpenSSH command line (often cmd.exe) allows,
      so it is installed once per version at
      `%LOCALAPPDATA%\goway\remote-<sha256 prefix>.ps1` and every call is
      a short encoded loader (`remote::ps_invocation`): it exits 126 when
      the file is missing, goway then sends it on stdin to
      `remote::ps_install` and retries. Every argument is single-quoted
      with embedded quotes doubled, so nothing is re-split; a long
      command goes through the `argsfile` verb and `run ... -- @args`.
   3. A job runs with the standard handles inherited (live, byte-exact
      output) inside a job object that kills its whole process tree when
      the script ends however it ends; a polling check of the session's
      parent process also stops the job and frees its slot when the
      connection drops. Exit codes outside 0-255 map to 128+N.
   4. Tests: `crates/goway/tests/remote_ps1.rs` drives the script under
      pwsh on Linux and under Windows PowerShell on Windows CI.
   5. Windows hosts through the pipeline (`transport::Kind`): a host with
      `os = "windows"` is `WindowsSsh` (OpenSSH, PowerShell) or
      `WindowsInterop` (the Windows side of this machine from WSL through
      `powershell.exe`, no ssh, no address; found as `Source::Interop`).
      Callers never build a command line: they make a `remote::Call`
      (verb and arguments) and `sync::SshTransport` (`kind`, target,
      settings) renders it as bash, as an encoded PowerShell line over
      ssh, or as a `powershell.exe` process. `resolve::resolve_call`
      probes in the host's language and, when the probe answers
      `goway-needs-install`, installs the script and probes again; the
      transport does the same on exit 126 for every other call. So run,
      sync, `--env`, copy verification, shards, status, gc and uninstall
      reach Windows hosts through the same code as Linux ones, and the
      `--report` file records `os` and the architecture the host reports
      (`aarch64` on an ARM Windows machine, never assumed).
   6. The NTFS copy. Sync is the same protocol as on Linux: the Windows
      script lists its seed manifest natively (never a stat scan across
      drvfs), goway decides on its own side what differs and sends a tar of
      only that plus a deletion list; deletions and writes happen only
      inside the seed of that repository and worktree under goway's
      directory, and a run updates its persistent slot tree in place.
      Where goway on the WSL side must name something on the NTFS side (a
      `--keep` work dir), `interop::wsl_path_under_root` converts it with
      `wslpath` and `interop::within_root` refuses any path outside
      goway's directory.
2. Transport (`ssh`)
   1. Spawn the system `ssh` binary (agent, config, and Windows OpenSSH all
      work). ControlMaster multiplexing on Unix clients only.
   2. A single embedded remote script (`remote.sh`) implements every remote
      verb (probe, receive, run, gc, status, doctor); it is sent inline
      with each call so the remote needs no install beyond bash, coreutils,
      tar, findutils and flock.
3. Sync (`sync`)
   1. File set: `git ls-files -co --exclude-standard -z`, minus deleted
      files, minus `.env` and `.env.*` (unless configured), never `.git`.
   2. Remote seed mirror per (repo, worktree); the remote reports its
      manifest (path, size, mtime); goway sends a tar of changed files and
      a list of deletions. No rsync, so Windows clients work.
   3. The run's work directory holds a hard-link snapshot of the seed
      (`cp -al`: no data copied), made under the seed lock in the same
      step as the upload. Only `receive` changes a seed, and it replaces
      files by unlink and recreate, so a later sync never changes what a
      snapshot holds; seeds of sibling worktrees may share inodes the
      same way. Nothing runs in or writes through the snapshot: the job
      runs in a slot tree (4.1), whose files are separate copies, so a
      job's writes never reach the seed or other runs.
4. Run (`run`)
   1. Remote layout under `~/.cache/goway/`:
      `work/<run-id>/{tree,meta.json,lock}`,
      `seed/<repo-id>/<worktree-id>/{tree,meta.json,lock}`,
      `cache/<repo-id>/{target-<k>,tree-<k>,tree-<k>.stats,affinity-<worktree>,sccache,meta.json}`.
      A run holding slot k runs in `tree-<k>`: binaries reused from that
      slot bake that path, so it must hold the current tree.
      `tree-<k>` persists between runs (persistent slot trees). Under the
      slot lock, `sync_slot` updates it in place from the run's snapshot:
      only files that may differ are looked at: new or changed in the
      snapshot since the slot's last reconcile (`tree-<k>.farm` keeps the
      snapshot records then), changed or removed in the slot by a job
      (`tree-<k>.slot` keeps the slot's own records then), missing, or
      named by the seed's change log. A candidate whose content (SHA-256)
      already equals the snapshot's is left alone, so files that did not
      change keep their mtime and warm builds stay warm. A file actually
      written gets the current time as mtime, as git checkout does:
      make, ninja and cargo rebuild only when a source is newer than its
      output, and a changed file from an older branch (older laptop
      mtime) must never look older than another branch's build. The
      laptop's mtimes only decide what is sent, never what the slot
      shows. If anything in the slot or its target dir is dated in the
      future (a clock step), written files are stamped one second after
      the newest such file instead. Files in neither the snapshot nor the keep
      set are removed, so no run sees another's leftovers; emptied
      directories go. It uses only find, sort, comm, xargs and cp (git
      when present, for the ignore rules). The keep set is detected
      dependency and build dirs (package.json: node_modules, .next,
      .nuxt; pyproject.toml and requirements*.txt: .venv, venv, .tox,
      .nox, __pycache__, caches; CMakeLists.txt: build, cmake-build-*;
      pom.xml: target; build.gradle(.kts): .gradle, build), plus the
      `keep` config list, plus every path the snapshot's .gitignore rules
      ignore (`git check-ignore --no-index`, switch `keep_ignored`).
      Markers anchor their dirs next to themselves; kept dirs are not
      scanned. A tracked file under a kept dir is still rewritten when it
      differs; one deleted from the work tree stays in a kept dir.
      Same-size edits: mtimes are whole seconds, so metadata alone cannot
      tell two same-size edits made within one second apart. Two rules
      close that. (1) The client stores a file whose mtime is within the
      last second of the sync one second older than it is (git's racy
      rule), so the next sync sees a different mtime and compares content.
      (2) The client sends the paths it writes (`changes` verb) and
      `receive` appends them to the seed's numbered change log (last 64
      kept); the slot records `seed key, generation, change number` in
      `tree-<k>.state`, and entries the log names since then are compared
      by SHA-256 even when size, mtime and mode match. A slot with no
      matching state compares every equal-metadata file by content. An
      edit that also restores the old size and mtime by hand
      (`touch -r`) is not detectable without hashing the whole tree and
      is not covered.
      Why not hard-link slot files to the seed: a job writing in place
      would change the seed and sibling worktrees. `tree-<k>.stats`
      records the last update's written and removed counts.
      Slot affinity: `affinity-<worktree>` names the slot that worktree
      used last; it is tried first, then the others in order.
      `--keep` copies the finished tree to `work/<run-id>/tree` (the slot
      stays in use by later runs). Slot trees live in the cache entry, so
      they expire with it (6.2) and purge removes them.
      Locks: gc may remove an expired entry at any time, so lock
      acquisition (`lock_dir`, the slot locks) creates the directory,
      locks, and retries unless the held lock file is the one the
      directory currently has.
   2. Every directory has `meta.json` (kind, repo, worktree, client,
      created, last_used) and is held by `flock` while in use; a free lock
      means nobody uses it.
   3. Cargo: `CARGO_TARGET_DIR` is the first free target slot of the repo
      (new slot when all are busy, up to `target_slots`); sccache is used
      when installed, with a per-repo `SCCACHE_DIR`.
   4a. Copy integrity (`Gate` in `run.rs`, `claims`/`verify_gate` in
      `remote.sh`). The run verb publishes claims about its slot tree in
      `work/<run>/verify.<phase>` (phase 1: every path the sync wrote, or
      all synced files for a distrusted repository; phase 2: after a
      non-zero exit, every synced file whose ctime did not change during
      the command) and blocks on `verdict.<phase>`. goway reads them with
      separate `verify-wait` calls (so stdio of the job is untouched),
      compares with the local files and answers with `verify-verdict`.
      Claims are `f SOH sha256 SOH path` and `l SOH target SOH path`
      records, parsed strictly (64 lowercase hex digits, plain relative
      paths, bounded count and length). A bad verdict makes the helper
      wipe the slot tree, its target dir and the seed (never refilled from
      a sibling seed) and exit 125; goway then reruns as attempt 2 (the
      attempt number is an explicit `verify:N:level[:fresh]` word of the
      run verb), and attempt 2 never reruns. A proven mismatch is stored
      per host and repository in local state (`distrust`, 7 days).
   4. stdout and stderr stream through (byte for byte unless a stream is a
      terminal, where control sequences other than colors are stripped; see
      docs/usage.md); goway's own lines go to
      stderr. Exit code is the command's; 128+N on signal N; 125 when goway
      itself fails (the docker convention).
   5. Ctrl-C: goway sends a remote kill to the job's process group, and a
      watchdog in the remote script kills the group if the ssh session dies.
   6. Provenance for frob: a header line on stderr naming host, arch and
      address, and `--report FILE` writes the same as JSON.
5. Pool (`pool`)
   1. Probe all hosts in parallel (one ssh each): nproc, loadavg, arch, goway
      jobs running, disk used.
   2. Score = (load1 + goway jobs) / cores, skipping unreachable hosts and
      hosts at `max_jobs`; `--host` pins.
6. Cleanup (`gc`)
   1. Work directories are removed when the run ends unless `--keep`.
   2. Expiry: caches and seeds 7 days idle, orphaned work dirs (lock free,
      no `--keep`) 1 day, kept work dirs 3 days. Configurable. A
      cache entry's slot trees go with it.
   3. Every run triggers a cheap gc of expired entries on the host it used.
   4. `goway gc [--older-than D] [--repo R] [--host H] [--all] [--dry-run]`.
7. Status and doctor (`status`, `doctor`)
   1. `goway status`: hosts, address, load, cores, running jobs, disk used.
   2. `goway doctor [HOST] [--fix]`: ssh reachability and auth, remote
      tools (rustup, cargo, cargo-nextest, sccache, tar, flock), disk. Fix
      commands run as the ordinary user; anything that needs root is
      printed with the reason and the user is asked to run it with sudo.
8. Output (`render`)
   1. One module owns all printing; clippy denies print macros elsewhere;
      colors honour `NO_COLOR` and `--color`. Diagnostics via tracing,
      `-v` raises the level.
9. Installation (`goway-setup`)
   1. Linux: `scripts/install.sh` and `scripts/uninstall.sh` (user-local).
   2. Windows: `goway-setup.exe`, components `client` (goway.exe, user
      PATH, Add/Remove Programs entry) and `host` (firewall rule, Hyper-V
      firewall, keepalive task, .wslconfig mirrored or, in NAT mode, the port relay and its refresh task, WSL sshd on 2222).
   3. Every change is a journal entry recording the prior state; the
      uninstaller replays the journal backwards. Proven by a property test
      over a model system (uninstall after install restores the state) and
      by a snapshot test on the real Windows hosts in an isolated profile.
      Implemented by the `goway-journal` crate: a serde `Change` vocabulary
      (whole files, tagged lines, directories, PATH-like list entries,
      registry values, ini keys, unix modes, SDDL ACLs, named resources),
      a `System` trait with a `LocalSystem` (files, dirs, lines, ini, unix
      modes) and an in-memory `ModelSystem`, `apply` (write-ahead: capture
      prior, record, then mutate; already-satisfied changes record a no-op)
      and `revert` (reverse replay, per-entry `reverted` flag so it is
      idempotent and resumable). Guarantee, property-tested:
      `revert(apply(plan)) == initial`, a second apply reverted leaves the
      first install intact, and targets edited since install are left
      alone and reported rather than clobbered.

10. Guided SSH setup (`goway ssh setup HOST`, journaled like 9.3)
   1. Client key: reuse an existing agent or identity key, or create a
      dedicated `~/.ssh/goway_ed25519` with `ssh-keygen` (never reads
      private keys; only checks presence and permissions).
   2. Authorize it on the host: append the public key to the WSL user's
      `~/.ssh/authorized_keys` with a `goway:<journal-id>` comment, fix
      modes (700 dir, 600 file); on a Windows sshd target, the
      `administrators_authorized_keys` path and its `icacls` ACL
      (Administrators and SYSTEM only) as Microsoft documents. Uses
      `ssh-copy-id` semantics; Windows has no ssh-copy-id, so goway does it.
   3. Client side on Windows: `icacls` on `%USERPROFILE%\.ssh` and the key
      so OpenSSH accepts them; start the `ssh-agent` service on startup.
   4. Host activation on startup is the `host` component of 9.2 (sshd
      enabled under systemd, keepalive task).
   5. Every step records its prior state; `goway ssh setup --undo HOST`
      removes exactly the lines and settings it added.

### Windows run overhead (one PowerShell per host, not per call)

Starting `powershell.exe` costs 0.3 s or more, so a warm run that made one
start per helper call (probe, manifest, receive, env, the copy verification
gate, the run, a detached gc) spent about 8 s on overhead. Measured with
`goway run --host winlocal -- cmd /c exit 0` on an unchanged one-file
repository, on the owner's ARM laptop through WSL interop while the machine
was busy (load average 20-30 on 12 cores), all runs sorted:

| build | runs (s) |
| --- | --- |
| before | 8.4 8.5 8.5 8.7 8.9 9.6 |
| after | 2.8 2.9 3.0 3.2 3.3 3.9 4.0 4.2 |

What changed, in order of weight:

1. `remote.ps1 session` serves manifest, hashes, deletions, changes,
   receive, envfile, argsfile, verify-wait and verify-verdict from one
   process (crates/goway/src/session.rs: length-prefixed frames, the same
   verbs and exit codes). A warm run is now three starts: probe, session,
   run. Anything that goes wrong with a session (it does not start, breaks,
   an input over 32 MiB) falls back to one process per call;
   `GOWAY_NO_SESSION=1` turns it off.
2. The detached gc is only started when an entry is old enough for the
   shortest TTL (it used to start a PowerShell on every run).
3. No WMI on the hot path: memory, CPU load and the parent process come
   from kernel32/ntdll calls (`Get-CimInstance` costs a second or so per
   query under Windows PowerShell).
4. The native helper is compiled once per script version into a DLL beside
   the script (Windows PowerShell), then loaded in about a tenth of the
   time.
5. The copy-verification handshake polls every 25 ms instead of 100 ms.

### `--with-git` (crates/goway/src/gitmeta.rs)

The `.git` of a helper copy is an overlay: built on the laptop under the
state dir (init with an empty template, a pack of HEAD's one commit made
with `rev-list --objects -1` + `pack-objects` + `index-pack`, `shallow`,
HEAD, one ref file, a whitelisted config, the real index) and added to the
sync's file list as ordinary `.git/...` entries whose bytes are read from
the overlay (`write_tar_with`, `compare_claims_with`). Because they are
ordinary seed files, `remote.sh` and `remote.ps1` need no change and the
copy-integrity check covers them. Blobs of paths the secret rules keep local
are filtered out of the pack list.

## 3. Where each part is documented

User-facing behaviour is described in docs/usage.md (run, status, gc,
doctor), docs/hosts.md (identity and resolution), docs/config.md (files,
keys, environment), docs/install-windows.md (installer) and
docs/positioning.md (niche and coexistence rules). This page keeps the
problem tree and the measured facts behind the design.

### Output integrity

`render.rs` owns one static output lock. Every writer takes it: the
`Renderer` messages (built whole, then written once), `write_locked` for
pass-through bytes and `LineFramer` batches for sharded runs. The framer
holds at most one partial line (64 KiB) per stream, splits longer lines
with a `[host]+ ` continuation prefix, and terminates a partial last line
at EOF. `shard::pump` reads with a 64 KiB `BufReader`, frames everything
buffered and writes it as one batch; a blocked write stops further reads,
which is the back-pressure. `tracing` diagnostics still go to stderr
unlocked; they are developer output and off by default.

## 4. Configuration

`~/.config/goway/config.toml` (Windows: `%LOCALAPPDATA%\goway\config.toml`):

```toml
[defaults]
remote_root = ".cache/goway"  # relative to the remote home
cache_ttl = "7d"
orphan_ttl = "1d"
kept_ttl = "3d"
target_slots = 4
send_env_files = false
keep = []                # extra paths kept in slot trees
keep_ignored = true      # also keep .gitignore'd paths

[[host]]
name = "helios"          # identity; ssh HostKeyAlias goway-helios
address = "Helios"       # optional; name or IP; resolved each time
port = 2222
user = "user"
max_jobs = 2
```

State (cached addresses) lives in `~/.local/state/goway/` and goway's
known_hosts in `~/.config/goway/known_hosts`.
