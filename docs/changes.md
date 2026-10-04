# What goway changes on a machine

Every change goway or goway-setup makes to a machine is written to a journal
first (the state it replaces, or the fact that it happened), so that you can
list it and undo it. The code enforces this: a test fails the build when a
command or file write that changes a machine appears outside the journal-backed
modules (see "How this is enforced" below).

## The journals and how to read them

| Journal | Where | List | Undo |
|---|---|---|---|
| This laptop: config and pinned keys, and actions (fixes run on helpers, tools installed here) | `changes.json` in goway's config directory (`goway config path`) | `goway changes` | `goway changes undo` (the newest change), `--last N`, `--all` |
| Key login set up on a helper (its `~/.ssh`, `authorized_keys` line, goway's own key pair) | `ssh-setup-HOST.json` in the config directory | `goway ssh setup HOST` says what is recorded | `goway ssh setup HOST --undo` |
| What `goway doctor --fix` installed on a helper | `installed-HOST.json` (what was installed) and `fixes-HOST.json` (the journal of pinned tools and rustup targets) in the config directory | `goway uninstall --dry-run` lists it | `goway uninstall` (derives each undo from the check name) |
| The Windows helper install (host and client components) | `host-journal.json` (administrator-only directory) and `install-journal.json` (per-user state), see `docs/install-windows.md` | `goway-setup status` | `goway-setup uninstall` |
| `.wslconfig` tuning | `tune-journal.json` in the per-user state directory | read the JSON file (no list command yet) | `goway-setup uninstall --host` (restores the previous values) |
| goway's own install on this laptop | the install journal written by `scripts/install.sh` | `goway uninstall --dry-run` | `goway uninstall` |

Undo runs newest first and keeps what you changed since: a file edited again
after goway wrote it is left alone and reported (`kept`), never overwritten.

## Every kind of change

Changes that undo reverses exactly (the journal holds the prior state):

| Change | Where it comes from |
|---|---|
| file written, line added to a file, directory created, file installed (a byte-exact copy) | `goway-setup install`, the keepalive/relay scripts, `.wslconfig` and `wsl.conf` keys, sshd drop-ins on the distro, `~/.ssh` and `authorized_keys` on a helper |
| `PATH` entry, registry key and value (the Add/Remove Programs entry) | `goway-setup install` |
| unix mode, Windows ACL | `~/.ssh` (700) and `authorized_keys` (600) on a helper, protected install files |
| Windows Defender and Hyper-V firewall rules and the scope of the built-in rule | `goway-setup install --host` |
| scheduled tasks (the WSL keepalive, the relay refresh) | `goway-setup install --host` |
| `netsh interface portproxy` relay | `goway-setup install --host` in NAT mode |
| Windows service (sshd) and capability (OpenSSH Server) | `goway-setup install --host --native` |
| distro package (`openssh-server`) and enabled systemd unit | `goway-setup install --host` |
| pinned tool: the tree under `~/.local/opt/goway-TOOL` and each link in `~/.local/bin`, and an outside link it replaced (restored; a regular file in the way is never replaced) | `goway doctor --fix` (`uv`, `go`, ...) |
| rustup target of a user's toolchain | `goway doctor --fix` for `rust_targets` |
| ssh key pair (goway's own, in its config directory) | `goway ssh setup`, `goway add` |
| `config.toml` and pinned `known_hosts` edits, as whole-file writes with the text they replaced | `goway add`, `goway host add`, `goway host remove`, `goway ssh setup` |

Changes that cannot be inverted are recorded as **actions** with the time, the
machine and the reason. Undo reports each one as `not undone` (and exits 1),
with how a person can take it back by hand when there is a way; it never skips
one silently.

| Action | When |
|---|---|
| start a scheduled task | `goway-setup install --host` (the keepalive), and after a WSL restart |
| shut down WSL, terminate a distro | `goway-setup tune`, only after an explicit yes (recorded in the tune journal) |
| reload or restart the distro's sshd | `goway-setup install --host` |
| run a fix command on a helper (a system package, the rustup toolchain, `loginctl enable-linger`, sshd hardening) | `goway doctor --fix`, `goway add`; the entry says to use `goway uninstall` |
| install the tools goway needs on this laptop | `goway add --lsudo` |
| authorize goway's key through a Windows administrator step | `goway ssh setup --rsudo` (the elevated `goway-setup` journals its own changes on the helper too) |

An action is written before it runs. When it cannot be written, it does not run.

Undo steps themselves (`goway uninstall`, `--undo`, the restart of sshd after
its drop-in is removed) are not new changes and are not recorded; `goway
uninstall` deletes the change log with the rest of goway's files.

## What is not recorded: goway's own run state

goway keeps state that exists only to run commands: the per-run work trees and
caches on helpers, run footprints and locks, the queue, `systemd` run scopes,
`TMPDIR`s, ssh control sockets, the scratch `known_hosts` of one probe, the
host address cache, and the staging, log and settings files of `goway-setup`.
These are labelled, expire on their own and are removed by `goway gc` (and by
uninstall for the `goway-setup` files). They are not machine changes in the
sense above, so they are outside the journal by design. `--report FILE` writes
only the file you name.

## How this is enforced

`crates/goway/tests/journal_invariant.rs` scans `crates/*/src` (Rust, shell and
PowerShell) for the verbs that change a machine (`Register-ScheduledTask`,
`schtasks`, `netsh`, firewall cmdlets, `Set-ItemProperty`, `reg add`, `apt`,
`dnf`, `systemctl enable`, `loginctl enable-linger`, `wsl --shutdown`,
`icacls`, `chmod`, ...) and for `std::fs` calls that write or remove. It fails,
naming the file, line and verb, when one appears outside a reviewed allowlist
in that file. Each allowlist entry has a one-line reason (the `System`
implementation, a command builder only the `System` implementation calls,
fix text that only runs through a recording runner, or run state) and is
checked for staleness. Test code is not scanned. The scanner is itself tested
with planted violations.

To add a change goway makes to a machine: express it as a `goway_journal::Change`
(or, for something that cannot be inverted, call `changelog::record_action`
before it runs), and only then add the module to the allowlist if it is the
journal-backed implementation.

A clippy `disallowed-methods` list was not added: `std::fs` calls are
legitimate and everywhere in run state, so a lint would need an allow
attribute on most files and would not tell a machine change from run state.
