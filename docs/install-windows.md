# Installing goway on Windows

`goway-setup.exe` is a single-file installer (the client component is per-user and needs no
administrator rights; only the host component elevates) built around the
journal in `crates/goway-journal`. Every change it makes is recorded together with the state it
replaced, so `uninstall` replays the journal backwards and restores the machine.

## Commands

    goway-setup install [--client] [--host] [--profile NAME] [--dry-run]
                        [--port N] [--distro NAME] [--keepalive logon|boot] [--no-harden]
                        [--allow-from CIDR]... [--no-activate] [--no-elevate]
    goway-setup uninstall [--client] [--host] [--profile NAME] [--no-activate] [--no-elevate]
    goway-setup status [--profile NAME]

`--profile` (default `goway`) names the install directory, the journal and the Add/Remove Programs
key, so a test profile never touches a real install. `--dry-run` prints the plan and changes
nothing. `-v` raises diagnostics, `--color` controls color. Components are selectable: `--client`
(the default when neither is named) and `--host`. Each component has its own journal, so either
can be installed and removed on its own; `uninstall` without flags removes every component that has a
journal (host first, then client) and `status` shows both. The client journal
(`install-journal.json`) lives in the profile's per-user state directory; the host journal
(`host-journal.json`) and `host-settings.json` (distro and port, so uninstall reaches the same
distro) live in an administrator-only directory, `%ProgramData%\goway\P` (see "Security of the
elevated host steps" below). They are deleted with the component.

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
| Defender Firewall rule | inbound TCP `N`, Allow, profiles Private and Domain only, remote address `LocalSubnet` (plus any `--allow-from`), display name `WSL SSH N` (default profile; `P WSL SSH N` otherwise) |
| Hyper-V firewall rule | inbound TCP `N` Allow for the WSL VM (`VMCreatorId {40E0AC32-46A5-438A-A0B2-2B479E8F2E90}`), named `WSL SSH N (Hyper-V)`; skipped when the cmdlets do not exist (before Windows 11 22H2). Scoped like the Windows rule (Private and Domain profiles, `LocalSubnet` plus any `--allow-from`). A specific rule is used instead of flipping the default inbound action |
| scheduled task | `WSL Keepalive` (`P WSL Keepalive`): at logon of the invoking user, `conhost.exe --headless wsl.exe -d D --exec /bin/sh -c "exec sleep infinity"`, Interactive, no time limit, runs on battery, one instance. `--keepalive boot` registers `WSL Keepalive (boot)` at startup with an `S4U` principal instead |

WSL side (run as root through `wsl.exe -d D -u root --exec ...`; no password, no shell):

| What | Value |
|---|---|
| `/etc/wsl.conf` | `[boot] systemd=true` (a no-op when set; systemd must already be PID 1, otherwise the install stops with the exact commands to enable it, because that needs a WSL restart goway will not do for you) |
| `openssh-server` | installed with `apt-get install -y` only when dpkg shows it cleanly absent |
| `/etc/ssh/sshd_config.d/20-P-port.conf` | `Port N` (skipped when `sshd -T` already lists `N`); Ubuntu's socket generator turns it into `ssh.socket` listen addresses |
| `/etc/ssh/sshd_config.d/10-P-hardening.conf` | `PasswordAuthentication no`, written by default as soon as the distro's default user has an authorized key (`--no-harden` opts out); without a key it is skipped and the install ends with a loud warning naming the next step |
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
ordinary console it re-runs only the host component through the `runas` verb (a UAC prompt) and
returns the elevated copy's exit code; the client component never runs elevated. It
does not prompt when it cannot be shown (no `SESSIONNAME`, as in SSH sessions), with `--no-elevate`,
or in the elevated copy itself; it then fails with a message saying how to start an elevated
terminal. The interactive UAC path is built but was not exercised end to end (the test machines are
only reachable over SSH). How the elevated step is kept safe is described next.

## Security of the elevated host steps

In plain words: the part of goway-setup that runs with administrator rights no longer believes
anything a normal program running as you could have written. Before this change the host journal
lived in your profile, where any program you run can edit it, and the elevated uninstall replayed it
as administrator; a planted entry could overwrite a system file or delete every firewall rule. Now:

* **Host state is in an administrator-only place.** The host journal and settings live in
  `%ProgramData%\goway\P`, created by the elevated process, writable only by Administrators and
  SYSTEM (everyone else may read). Before reading anything from it goway-setup checks that the
  directory is a real directory (not a link), is owned by Administrators or SYSTEM, and grants no one
  else more than read access; otherwise it refuses and says why.
* **Only entries goway itself would have written are replayed.** The elevated uninstall rebuilds
  what the host plan can contain from the validated settings (profile, port, distro) and refuses the
  whole journal, before reverting anything, if one entry is not in that set: other paths, registry
  keys, ACLs, or resource names with wildcards.
* **Names are matched exactly.** The PowerShell lookups for firewall rules and scheduled tasks
  escape wildcards and filter on exact equality, so a rule named `*` can no longer mean "all rules".
* **Only the host component is elevated**, started with explicit arguments (no copy of your raw
  command line), and only for the same account that asked: elevating as a different administrator is
  refused, because your profile and your WSL distros belong to you.
* **The elevated program is not the one in your profile.** A host install keeps a copy of
  `goway-setup.exe` in the administrator-only directory, and a later uninstall that needs elevation
  starts that copy, not `%LOCALAPPDATA%\Programs\P\goway-setup.exe`, which any program you run
  could have replaced. (The very first install still runs the exe you started: you chose it, and the
  UAC prompt names it.)
* **No shell, no search path.** The UAC relaunch passes the exe and its arguments directly (no
  `cmd.exe`, so characters like `&` or `%` in an argument mean nothing), and `powershell.exe`,
  `wsl.exe` and `cmd.exe` are started by absolute path under the Windows System32 directory, with
  DLL loading limited to System32.
* **Temp and log files are created exclusively** (`CREATE_NEW`) in the administrator-only
  directory; the journal's temporary file is never written through a pre-existing file or link.

<details><summary>Details for reviewers</summary>

* Directory ACL (SDDL): `O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;BU)` for both
  `%ProgramData%\goway` and `%ProgramData%\goway\P`: owner Administrators, protected (no inherited
  entries), full control for Administrators and SYSTEM, read and execute for Users. The check
  (`admin::check_sddl`) requires an Administrators, SYSTEM or TrustedInstaller owner, a DACL, and no
  allow entry for any other trustee with a write, delete, change-permissions or take-ownership bit
  (symbolic or hexadecimal). It is pure and unit tested off Windows (`tests/elevated.rs`). Parent and
  child directories are both verified; a reparse point is refused. Known folders, the Windows
  directory and the token SID come from system calls, never from environment variables.
* Validation (`host::validate_journal`): every entry's change must equal one of the changes of
  `host_plan` over both keepalive modes, with and without hardening, the Hyper-V rule and the port
  drop-in, for the saved port and distro. A directory-removal prior may list only ancestors of the
  directory the entry ensured. Resource names with `*?[]` or a backtick are refused (in the
  validator and again in `HostSystem`).
* The elevated re-run gets `--elevated-child --invoker-sid SID --elevated-log NAME`. It compares
  the SID with its own token, creates the directory (verifying it), deletes older
  `elevated-*.log` files, creates its log with `create_new` and redirects stdout and stderr to it;
  the unelevated parent reads that file (only after verifying the directory) and prints it. Early
  failures before the log exists are visible only as the exit code.
* The legacy location: a `host-journal.json` left in `%LOCALAPPDATA%\P` by an older version is never
  read or replayed; `status` and `uninstall` print a notice with the path.
* The log of an elevated uninstall and, when it ran from the protected copy, that copy cannot be
  deleted while in use; a hidden `cmd` (absolute path, same token) removes the directory about five
  seconds after the process exits. Runs that never go through UAC (an administrator terminal or SSH
  session) remove everything immediately.
* Invariant `INV-ELEV-001`: elevated code never acts on data from a location a non-administrator can
  write (marked at `enter_elevated_child` in `crates/goway-setup/src/cli.rs`).
* Not covered: a program already running with your administrator token, or one that replaced the
  exe before the very first install, is out of scope; so is the ssh key ACL of `goway ssh setup`
  (the client crate).

</details>

## Who can reach sshd (network scope and password login)

In plain words: by default only machines on your own local network can connect, and only while the
network is one you marked Private (or a domain network); and once a key works, passwords stop
working. Before this change the firewall rules were open on every network profile and to every
address, so a laptop on cafe Wi-Fi (a Public network) exposed the WSL sshd, with password login
on, to everyone on that network.

* **Firewall rules** (the Windows Defender rule and the Hyper-V rule for the WSL VM) apply only on
  the Private and Domain profiles and admit only the local subnet.
* **Widening is explicit.** `--allow-from CIDR` (repeatable) adds a remote address or range to the
  rules, for example `--allow-from 100.64.0.0/10` for Tailscale. The values are checked (an address
  or CIDR; `0.0.0.0/0` and `::/0` are refused because they would undo the scoping) and are kept in the
  host settings so uninstall rebuilds the same plan.
* **Password login is switched off by default** by the `PasswordAuthentication no` drop-in, but only
  when the distro's default user already has an authorized key, so you cannot lock yourself out.
  `--no-harden` opts out. With no key yet, the install finishes with a loud warning: run
  `goway ssh setup HOST` from your main laptop, then `goway doctor HOST --fix --rsudo` (or uninstall
  and rerun the install) to turn passwords off.
* **Public networks are called out.** If a connected network is classified Public, the install
  warns that the rules do not apply there (sshd is not reachable over it) and shows the command to
  mark a network you trust as Private. `--dry-run` shows the same warning.

<details><summary>Details</summary>

* Windows rule: `New-NetFirewallRule ... -Profile 'Private','Domain' -RemoteAddress 'LocalSubnet'[,...]`.
  Hyper-V rule: `New-NetFirewallHyperVRule ... -Profiles 'Private','Domain' -RemoteAddresses
  'LocalSubnet'[,...]` (the cmdlet takes the same keyword and CIDR forms). Both keep the `-Enabled`
  and action settings of before; only the scope changed. `ps::firewall_create` never emits
  `-Profile Any` (a missing, `Any` or damaged scope in a journal falls back to Private and Domain), and
  `tests/hostsys.rs` asserts it.
* The scope is part of the rule's recorded spec, so uninstall removes exactly what was created. A
  journal written before this change (no scope fields) is still accepted by the elevated validator
  and uninstalled.
* The `--allow-from` list is saved in `host-settings.json` and the expected-plan validator of the
  elevated uninstall is built from it; settings with an invalid CIDR are refused.
* Tailscale: its adapter must itself be classified Private (or Domain) for the rule to apply; if
  Windows labels it Public, run `Set-NetConnectionProfile -InterfaceAlias Tailscale -NetworkCategory Private`
  in an administrator PowerShell, or leave it Public and reach the host over the local network.
* `--harden` is still accepted and does nothing (it is the default); combining it with `--no-harden`
  is an error.
* The live proof (`scripts/windows/roundtrip-host.sh`) installs with `--allow-from 100.64.0.0/10`
  (it logs in over the host's Tailscale address), then checks that both rules show only the
  Private and Domain profiles and `LocalSubnet` plus that range, that `sshd -T` reports
  `passwordauthentication no` for the default case and `yes` with `--no-harden`, and that
  snapshots before and after uninstall are identical.

</details>

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
keepalive with the default hardening and for a boot keepalive with `--no-harden` (both with `--allow-from 100.64.0.0/10`), it installs, checks the new rules and task, runs
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
