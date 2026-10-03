# Installing goway on Windows

`goway-setup.exe` is a single-file, per-user installer (no administrator rights) built around the
journal in `crates/goway-journal`. Every change it makes is recorded together with the state it
replaced, so `uninstall` replays the journal backwards and restores the machine.

## Commands

    goway-setup install [--client] [--profile NAME] [--dry-run]
    goway-setup uninstall [--profile NAME]
    goway-setup status [--profile NAME]

`--profile` (default `goway`) names the install directory, the journal and the Add/Remove Programs
key, so a test profile never touches a real install. `--dry-run` prints the plan and changes
nothing. `-v` raises diagnostics, `--color` controls color. Components are selectable; today only
`client` exists (the default), and the `host` component (firewall, keepalive task, `.wslconfig`,
WSL sshd) will plan its changes the same way and reuse the same journal.

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
