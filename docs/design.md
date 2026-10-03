# goway design

"go away": run a command on another machine, natively, from the current git
work tree. This document is the problem tree; each leaf names the module or
ticket that owns it. Read docs/prior-art.md for why this is a new tool.

## 1. Facts measured on the target machines (2026-10-03)

- Hosts are Windows laptops with WSL2 (Ubuntu, x86_64, systemd on). sshd in
  WSL listens on 2222; the Windows OpenSSH server on 22 lands in cmd.exe.
- WSL uses `networkingMode = mirrored`, so the Windows Wi-Fi address reaches
  the WSL sshd. A scheduled task keeps the distro alive; a Windows firewall
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
      files missing or different (type, size, mtime, mode, link target)
      are copied with the seed's mtime, so cargo, make and ninja
      fingerprints stay valid; files in neither the snapshot nor the keep
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
      firewall, keepalive task, .wslconfig mirrored, WSL sshd on 2222).
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

## 3. Where each part is documented

User-facing behaviour is described in docs/usage.md (run, status, gc,
doctor), docs/hosts.md (identity and resolution), docs/config.md (files,
keys, environment), docs/install-windows.md (installer) and
docs/positioning.md (niche and coexistence rules). This page keeps the
problem tree and the measured facts behind the design.

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
