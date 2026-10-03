# Installing goway on Windows

`goway-setup.exe` is a single-file, per-user installer (no administrator rights) built around the
journal in `crates/goway-journal`. Every change it makes is recorded together with the state it
replaced, so `uninstall` replays the journal backwards and restores the machine.

## Commands

    goway-setup install [--client] [--host] [--profile NAME] [--dry-run]
                        [--port N] [--distro NAME] [--keepalive logon|boot] [--harden]
                        [--no-activate] [--no-elevate]
    goway-setup uninstall [--client] [--host] [--profile NAME] [--no-activate] [--no-elevate]
    goway-setup status [--profile NAME]

`--profile` (default `goway`) names the install directory, the journal and the Add/Remove Programs
key, so a test profile never touches a real install. `--dry-run` prints the plan and changes
nothing. `-v` raises diagnostics, `--color` controls color. Components are selectable: `--client`
(the default when neither is named) and `--host`. Each component has its own journal in the profile's
state directory (`install-journal.json` for the client, `host-journal.json` for the host), so either
can be installed and removed on its own; `uninstall` without flags removes every component that has a
journal (host first, then client) and `status` shows both. A host install also writes
`host-settings.json` (distro and port) so uninstall reaches the same distro; it is deleted with the
journal.

`install --host --dry-run` on Windows probes the machine read-only and marks each step `in place`
or `will do`.

## What the client component changes (profile `P`)

| What | Where | Value |
|---|---|---|
| directory | `%LOCALAPPDATA%\Programs\P\bin` | created with missing ancestors |
| file | `%LOCALAPPDATA%\Programs\P\bin\goway.exe` | the payload embedded in the installer |
| file | `%LOCALAPPDATA%\Programs\P\goway-setup.exe` | a copy of the installer (what Uninstall runs) |
| user Path | `HKCU\Environment` value `Path` | `...\Programs\P\bin` appended (never duplicated; `REG_EXPAND_SZ` kept, a new `Path` is `REG_EXPAND_SZ`) |
| key | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\P` | created |
| values | that key | `DisplayName` (`goway`, or `goway (P)`), `DisplayVersion`, `Publisher` (`goway`), `InstallLocation` (`...\Programs\P`), `DisplayIcon`, `UninstallString` (`"...\Programs\P\goway-setup.exe" uninstall --profile P`), `NoModify=1`, `NoRepair=1` (dwords) |
| journal | `%LOCALAPPDATA%\P\install-journal.json` | rewritten after every entry (crash-safe); deleted, with its directory if empty, when uninstall completes |

After changing `Path` the installer broadcasts `WM_SETTINGCHANGE` ("Environment") so new shells
see it; already-open terminals keep their old environment.

Nothing else is sent or written: no network access, no admin rights, no secrets.

## What the host component changes (profile `P`, port `N`, distro `D`)

The host component turns a Windows machine with WSL2 into a goway build host, the way the two test
laptops were set up by hand. Windows side:

| What | Value |
|---|---|
| `%USERPROFILE%\.wslconfig` | `[wsl2] networkingMode=mirrored` (ini key; prior value journaled; a no-op when already set) |
| Defender Firewall rule | inbound TCP `N`, Allow, profile Any, display name `WSL SSH N` (default profile; `P WSL SSH N` otherwise) |
| Hyper-V firewall rule | inbound TCP `N` Allow for the WSL VM (`VMCreatorId {40E0AC32-46A5-438A-A0B2-2B479E8F2E90}`), named `WSL SSH N (Hyper-V)`; skipped when the cmdlets do not exist (before Windows 11 22H2). A specific rule is used instead of flipping the default inbound action |
| scheduled task | `WSL Keepalive` (`P WSL Keepalive`): at logon of the invoking user, `conhost.exe --headless wsl.exe -d D --exec /bin/sh -c "exec sleep infinity"`, Interactive, no time limit, runs on battery, one instance. `--keepalive boot` registers `WSL Keepalive (boot)` at startup with an `S4U` principal instead |

WSL side (run as root through `wsl.exe -d D -u root --exec ...`; no password, no shell):

| What | Value |
|---|---|
| `/etc/wsl.conf` | `[boot] systemd=true` (a no-op when set; systemd must already be PID 1, otherwise the install stops with the exact commands to enable it, because that needs a WSL restart goway will not do for you) |
| `openssh-server` | installed with `apt-get install -y` only when dpkg shows it cleanly absent |
| `/etc/ssh/sshd_config.d/20-P-port.conf` | `Port N` (skipped when `sshd -T` already lists `N`); Ubuntu's socket generator turns it into `ssh.socket` listen addresses |
| `/etc/ssh/sshd_config.d/10-P-hardening.conf` | `PasswordAuthentication no`, only with `--harden` and only when the distro's default user already has an authorized key (otherwise a note; `goway ssh setup` handles keys) |
| `ssh.socket`, `ssh.service` | enabled at boot (a unit that does not exist is skipped) |

Unless `--no-activate` is given, a changed sshd is then validated (`sshd -t`), systemd reloaded, and
the socket (or service) restarted only when the port is not yet listening (otherwise only reloaded),
and a newly registered keepalive task is started. Restarting WSL is never done: when `.wslconfig` or
`wsl.conf` changed, the install prints the `wsl --shutdown` / `wsl --terminate` you need.

**Package safety.** The package step is a recorded no-op unless dpkg reports exactly `ii`; if it
reports anything else but "not installed" (half-configured, removed with configuration left,
unpacked, ...) the install refuses that step with a message instead of touching it. Uninstall
**never purges**: if goway itself installed `openssh-server` it runs `apt-get remove`, which keeps
`sshd_config` and the host keys.

**Elevation.** Firewall rules need administrator rights. goway-setup checks its token
(`GetTokenInformation(TokenElevation)`). Over Windows OpenSSH an administrator account already holds a
full token (High mandatory level, verified on both test laptops), so nothing happens. In an
ordinary console it re-runs itself through the `runas` verb (a UAC prompt) with its output captured
in a temporary log that the unelevated copy prints, and returns the elevated copy's exit code. It
does not prompt when it cannot be shown (no `SESSIONNAME`, as in SSH sessions), with `--no-elevate`,
or in the elevated copy itself; it then fails with a message saying how to start an elevated
terminal. The interactive UAC path is built but was not exercised end to end (the test machines are
only reachable over SSH).

## Why uninstall provably restores the machine

* Each entry stores the prior state (absent file, previous registry value and type, whether the
  key or `Path` variable existed, which directories were created). Revert runs entries last to
  first. An entry whose target already held the wanted value is recorded as a no-op and is never
  removed (a `Path` that already contained the directory keeps it).
* An install over a different file is refused rather than overwritten.
* Targets edited since install (a replaced `goway.exe`, a `Path` entry that is gone) are left
  alone and reported as `kept`; they are never clobbered.
* Property tests (`crates/goway-journal/tests/properties.rs`,
  `crates/goway-setup/tests/app.rs`) check `revert(apply(plan)) == initial` over a model machine.
  `crates/goway-setup/tests/host_plan.rs` does the same for the host plan over a model machine in
  which any subset of the targets already exists (pre-existing things are recorded as no-ops and never
  removed); `tests/hostsys.rs` drives the real host `System` through a scripted fake of `wsl.exe` and
  PowerShell (command lines, quoting, dpkg-state handling, activation).
  `scripts/windows/roundtrip.sh HOST` checks the real thing: it snapshots the user `Path` (value
  and type), the Uninstall key, the install and state directories and installer files in `%TEMP%`,
  runs install then uninstall in profile `goway-test`, and fails unless the snapshots are identical.
  It runs a second case where `Path` already contains the directory.
* If an install fails part-way it rolls itself back before reporting the error.

## The running-exe problem

Windows cannot delete a running executable, and the uninstall entry runs the installed
`goway-setup.exe`. So when `uninstall` finds it is running from a file the journal installed, it
copies itself to `%TEMP%\goway-uninstall-<pid>\`, starts that copy detached (job breakaway, so it
survives an OpenSSH session ending) with its output in `uninstall.log` beside it, and exits. The
copy retries any locked file for about five seconds (covering the parent still exiting), reverts the
journal, broadcasts the environment change, and finally schedules a hidden `cmd` that deletes the
copy, its log and its directory after it exits (the standard self-delete trick). When run from
anywhere else (for example the downloaded installer) uninstall is synchronous and its exit code is
the result.

## Proving the host component on a live machine

    scripts/windows/roundtrip-host.sh Helios     # or any host name; symlink it as goway-roundtrip-host

The test machines are in use, so the script is deliberately gentle: profile `goway-test`, port 2299,
profile-prefixed names, `--no-activate` (no sshd, ssh.socket or WSL restart), and a precondition that
the read-only dry run shows the package, both units, `wsl.conf` and the drop-in directory already
`in place` (it refuses otherwise, so it can never install or remove a package). It snapshots firewall
rules (all, with ports and actions), Hyper-V firewall rules and VM settings, non-Microsoft scheduled
tasks, the `.wslconfig` bytes (`snapshot-host.ps1`), and `/etc/wsl.conf`, `/etc/ssh/sshd_config.d`
(hashes), the `ssh.socket` and `ssh.service` units including generated overrides, the package state,
unit enablement and listening ports (`snapshot-wsl.sh`, over the WSL sshd). Then, for a logon
keepalive with `--harden` and for a boot keepalive, it installs, checks the new rules and task, runs
`sshd -t`, starts a second temporary `sshd -p 2299` with the installed configuration (own pid
file; the listener on 2222 is not touched), logs in through the Windows address, kills it, uninstalls
and requires identical snapshots. Port 2222 is logged into once a second throughout and must never
fail. Probes are full public-key logins on purpose: `ssh-keyscan` or a bare connect counts as an
unauthenticated connection, which sshd 9.8+ penalises per source address.

## Building

    rustup target add x86_64-pc-windows-gnu      # once, for the pinned toolchain
    scripts/windows/build.sh                      # needs mingw-w64 (x86_64-w64-mingw32-gcc)

This builds `goway.exe`, then `goway-setup.exe` with it embedded (`GOWAY_PAYLOAD` is read by the
crate's build script; without it the crate still builds, with an empty payload, and refuses to
install). Output: `target/x86_64-pc-windows-gnu/release/goway-setup.exe`. x86_64 binaries run on
x64 Windows and under emulation on Windows on ARM.

## Proving it on a machine

    scripts/windows/roundtrip.sh Helios          # host name resolved through mDNS at run time

The script needs key-based SSH to the host, uploads the installer and helper scripts to
`~\goway-roundtrip`, removes them afterwards, and touches only the `goway-test` profile.
