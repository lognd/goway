# goway

"go away": run a command on another machine, natively, from the current
git work tree.

```
goway host add helios                      # find it by name, pin its ssh key
goway run -- cargo nextest run --workspace # least-loaded host, warm caches
goway status                               # hosts, load, jobs, disk
goway gc --dry-run                         # stale remote state
goway doctor --fix                         # toolchain and ssh checks
```

goway syncs exactly what git shows (tracked, modified and untracked but
not ignored files, never `.env` files) to the host and runs the command
in a fresh work directory there. Output streams back untouched, and
goway exits with the command's exit code. Builds stay warm through a
per-repository cargo target slot and sccache. Remote state labels
itself and expires on its own.

Hosts are identified by name and pinned ssh key, never by IP, so
machines on DHCP Wi-Fi keep working. The remote needs only sshd, bash
and coreutils. goway installs nothing on it and runs no daemon.

## Install

- Linux / WSL: `cargo install --locked --path crates/goway`
- Windows: `goway-setup.exe install` (per user; `uninstall` reverses
  every change; see docs/install-windows.md). Build it with
  `scripts/windows/build.sh`.

## Documentation

- docs/usage.md: run, status, gc and doctor; exit codes; what is sent
- docs/hosts.md: host identity, address resolution, `host add`
- docs/config.md: config file, paths, environment variables
- docs/install-windows.md: the Windows installer and uninstaller
- docs/positioning.md: goway's niche and how it coexists with other
  build systems
- docs/design.md: the problem tree; docs/prior-art.md: what exists
