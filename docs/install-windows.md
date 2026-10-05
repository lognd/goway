# Installing goway on Windows

## Set up a helper laptop (the main path)

A helper laptop is a Windows laptop that runs your builds and tests for you. Setting one up is
one command here and one command on your main laptop.

**Before you start**, the laptop needs WSL (Linux inside Windows) with Ubuntu, with a Linux user and
password created. If it does not have them, the installer stops before changing anything and tells
you the steps: open PowerShell as administrator, run `wsl --install -d Ubuntu`, restart, create the
Linux user and password when the Ubuntu window opens, then run the command below again. goway never
installs WSL for you.

**1. On the helper laptop**, in a terminal, run:

    goway-setup.exe install --host

Windows asks permission once (the administrator prompt); say yes. What you are asked: nothing else,
except possibly "Restart WSL now? [y/N]". Answer `n` (the default) if you have Linux windows open: the
restart closes them, and the installer then prints the exact command to run when you are ready.
(`--yes` asks no questions and prints the command instead.)

When it finishes it prints this block (the values are the ones of that laptop):

    ======================================================================
     THIS LAPTOP IS READY TO BE A HELPER. Next, on your MAIN laptop.
    ======================================================================

    Name to use for this helper:  orion-notebook  (this laptop's Windows name)
    Its ssh host key fingerprint: SHA256:...
    Its Linux user:               user  (goway asks for this user's password once: ...)

    On your main laptop run exactly this:

        goway add orion-notebook --fingerprint SHA256:... --user user

    goway shows the same fingerprint there; it must match the one above.
    ======================================================================

`goway-setup.exe status --host` prints the same block again later.

**2. On your main laptop**, run the `goway add ...` line it printed. goway compares the helper's
fingerprint with the one you passed, so you know you are talking to the laptop you just set up and
not to something else on the network.

**Windows 10, or Windows 11 without "mirrored" networking.** The installer works out by itself how
other computers can reach the Linux inside this laptop. Windows 11 22H2 or newer can mirror its
network into Linux; older Windows (10 21H2 and later, or 11 before 22H2), or a laptop whose
`.wslconfig` already says `networkingMode=nat`, cannot, so there the installer sets up a small
**relay** instead: Windows itself listens on the helper's port and hands each connection on to
Linux, and a scheduled task keeps the relay pointed at Linux even though Linux gets a new internal
address every time WSL restarts. You do nothing different: the same command, the same `goway add`
line (the relay listens on the same port number), the same firewall limits on who may connect, and
the same uninstall, which removes the relay and the task again. The finished block says which mode
was used ("Network mode: mirrored" or "Network mode: nat"); `--network mirrored|nat|auto` forces
one. In `nat` mode the installer does not touch `.wslconfig` at all.

**To remove the helper again**, use Windows Settings, Apps, "goway helper (host)", Uninstall (the
installer keeps its own protected copy, so you do not need the file you downloaded), or run
`goway-setup.exe uninstall --host`.

**If the laptop is on cafe or hotel Wi-Fi**: Windows treats such networks as Public, and goway only
opens its door on home or work (Private) networks, so the helper cannot be reached there. The
installer says so and prints the one line that fixes it for a network you trust (in PowerShell as
administrator: `Set-NetConnectionProfile -Name 'NETWORK' -NetworkCategory Private`).

Everything below explains exactly what the installer changes and why, for those who want to check.

`goway-setup.exe` is a single-file installer (the client component is per-user and needs no
administrator rights; only the host component elevates) built around the
journal in `crates/goway-journal`. Every change it makes is recorded together with the state it
replaced, so `uninstall` replays the journal backwards and restores the machine.

## Commands

<details><summary>Details</summary>

    goway-setup install [--client] [--host] [--native [--authorized-key KEY]] [--profile NAME] [--dry-run]
                        [--port N] [--distro NAME] [--keepalive logon|boot [--allow-elevated-wsl]] [--no-harden]
                        [--allow-from CIDR]... [--allow-wide] [--no-activate] [--no-elevate] [--yes]
    goway-setup tune [--memory SIZE] [--swap SIZE] [--processors N] [--nested-virtualization BOOL] [--sparse]
                     [--distro NAME] [--yes] [--dry-run] [--profile NAME]
    goway-setup uninstall [--client] [--host] [--profile NAME] [--no-activate] [--no-elevate]
    goway-setup status [--profile NAME]

`status` also prints the helper block (name, fingerprint, Linux user, the `goway add` command) when
the host component is installed.

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

</details>

## What the client component changes (profile `P`)

<details><summary>What this changes and why</summary>

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

</details>

## What the host component changes (profile `P`, port `N`, distro `D`)

<details><summary>What this changes and why</summary>

The host component turns a Windows machine with WSL2 into a goway build host, the way the two test
laptops were set up by hand. Windows side:

| What | Value |
|---|---|
| `%USERPROFILE%\.wslconfig` | `[wsl2] networkingMode=mirrored` (ini key; prior value journaled; a no-op when already set). Only in mirrored mode; in nat mode the file is never touched |
| Add/Remove Programs entry | `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\P-host` (machine-wide, because the host component is): `DisplayName` `goway helper (host)` (`goway helper (host, profile P)` for another profile), `DisplayVersion`, `Publisher`, `InstallLocation` (`%ProgramData%\goway\P`), `DisplayIcon` and `UninstallString` running the protected copy: `"%ProgramData%\goway\P\bin\goway-setup.exe" uninstall --host --profile P`; `NoModify=1`, `NoRepair=1`. It is the first change of the plan, so it is reverted last: a stopped uninstall can still be finished from Settings. The elevated validator accepts exactly these values (`DisplayVersion` may be that of another goway-setup version). Because Windows opens a console just for this entry, the uninstaller waits for Enter before closing it |
| Defender Firewall rule | inbound TCP `N`, Allow, profiles Private and Domain only, remote address `LocalSubnet` (plus any `--allow-from`), display name `WSL SSH N` (default profile; `P WSL SSH N` otherwise) |
| Hyper-V firewall rule | inbound TCP `N` Allow for the WSL VM (`VMCreatorId {40E0AC32-46A5-438A-A0B2-2B479E8F2E90}`), named `WSL SSH N (Hyper-V)`; skipped when the cmdlets do not exist (before Windows 11 22H2). Scoped like the Windows rule (Private and Domain profiles, `LocalSubnet` plus any `--allow-from`). A specific rule is used instead of flipping the default inbound action |
| scheduled task | `WSL Keepalive` (`P WSL Keepalive`): at logon of the invoking user and then every 5 minutes (so a distro that WSL shut down is started again; a running keepalive is never duplicated), `conhost.exe --headless wsl.exe -d D --exec /bin/sh -c "exec sleep infinity"`, Interactive, no time limit, runs on battery, one instance. An older install's task without a repetition is replaced on the next install, through the journal (its exported definition is kept as the entry's prior state, so uninstall restores it exactly). `--keepalive boot` registers `WSL Keepalive (boot)` at startup with an `S4U` principal instead |

In **nat mode** (`--network nat`, or `auto` when the Windows build is older than 22621 / Windows 11
22H2, the build is unknown, or `.wslconfig` explicitly says `networkingMode=nat`; `--network
mirrored` on such a build is refused with an explanation) the plan swaps the `.wslconfig` change for
the relay, placed right after the keepalive task so it is reverted before it:

| What | Value |
|---|---|
| refresh script | `%ProgramData%\goway\P\relay-refresh.ps1`, a `write file` entry in the host journal, in the administrator-only directory. It reads the IPv4 of the distro (`wsl.exe -d D --exec hostname -I`, first address), accepts only a dotted quad in a private (RFC 1918) range that lies inside the subnet of the WSL virtual adapter (`vEthernet (WSL...)`, read with .NET, not a module) and is not the adapter's own address, reads the relay's current target with `netsh interface portproxy show v4tov4` and runs `netsh interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=N connectaddress=<ip> connectport=N` only when it differs. `wsl.exe` and `netsh.exe` are started by absolute path under the directory Windows reports as the system directory (never `%SystemRoot%`, which a user-level variable could override), and `PSModulePath` is reset first. Distro and port are literals from the validated settings |
| relay | `netsh interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=N connectaddress=<WSL IPv4> connectport=N`, journaled as resource `0.0.0.0:N` (created only when absent; uninstall deletes only a rule that still looks like the relay, one listening on `0.0.0.0:N` and forwarding to port `N` on a private address, and leaves any other alone). The Defender rule above (Private and Domain, `LocalSubnet` plus `--allow-from`) decides who can reach port `N` |
| scheduled task | `WSL Relay` (`P WSL Relay`; `WSL Relay (boot)` with `--keepalive boot`): runs `conhost.exe --headless <System32>\cmd.exe /D /S /C "(for ... do set VAR=) & <System32>\WindowsPowerShell\v1.0\powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%ProgramData%\goway\P\relay-refresh.ps1""` (the `cmd.exe` stub first deletes the .NET and PowerShell injection variables, see "Known limit of the relay task") at logon of the invoking user (so after the keepalive has started WSL) and then every 5 minutes, for at most 5 minutes per run, one instance. It runs as the invoking user (only that user can see the distro) with the highest privileges (netsh needs administrator rights); `Interactive` logon type for the logon variant and `S4U` for the boot variant, so no password is ever stored |

If another portproxy rule (one goway did not create) already listens on port `N`, a nat install
refuses before changing anything and prints the `netsh interface portproxy delete` command for that
rule or suggests another `--port`; goway never edits or removes a rule it did not create. Rules on
other ports are never touched. The elevated uninstall accepts exactly these entries (relay
resource, refresh task, script file under the admin directory) for the mode recorded in the host
settings and nothing else; entries of the other mode, another port, another listen address, wildcard
names or another path are refused. `goway-setup status --host` shows the mode, and in nat mode
that the helper is reached through the relay on port `N`.

The Hyper-V firewall rule is still created in nat mode when the cmdlets exist: it is scoped like the
Defender rule and harmless there.

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
and the newly registered tasks (the keepalive and, in nat mode, the relay refresh) are started.

WSL is restarted only when you say so: if `.wslconfig` (mirrored networking; never in nat mode) or `wsl.conf` changed,
the install asks on the console "Restart WSL now? [y/N]", explaining that a restart closes open
Linux windows; the default is No. With `--yes`, `--no-activate` or no console (a pipe, an SSH
session) it asks nothing and prints the exact `wsl --shutdown` / `wsl --terminate D` instead. Before
anything else (even before the administrator prompt) `install --host` checks that `wsl.exe` exists
and lists the distro; if not it stops, changes nothing and prints the steps to install WSL.

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
shows the prompt only where it can be clicked: it compares its own session id (`ProcessIdToSessionId`)
with the active console session (`WTSGetActiveConsoleSessionId`), so a run started from WSL interop or
over ssh into WSL on a logged-in laptop (session 1, no `SESSIONNAME`) still gets the prompt, while a
service, a boot task (session 0) or another account's ssh session does not (a named session such as
a remote desktop also counts). It does not prompt when it cannot be shown, with `--no-elevate`,
or in the elevated copy itself; it then fails with a message saying how to start an elevated
terminal. The interactive UAC path is built but was not exercised end to end (the test machines are
only reachable over SSH). How the elevated step is kept safe is described next.

</details>

## A helper without WSL (`install --host --native`)

A Windows laptop that has no WSL can be a helper through Windows' own OpenSSH Server. Run, in a
terminal on that laptop:

    goway-setup.exe install --host --native --authorized-key C:\path\to\main-laptop.pub

Windows asks permission once (the administrator prompt). `--authorized-key` takes one ssh public key
line or the path of a `.pub` file (it is read with your own rights before the prompt and passed on as a
validated line); without it no key is authorized and the printed block says so. The installer
prints the same hand-over block as the WSL install: the name, the host key fingerprint and the one
`goway add NAME --fingerprint SHA256:... --user USER --port 22` line to run on the main laptop.
`status --host` prints it again, and `uninstall --host` reverts every change below.

What it changes (each entry is journaled in `%ProgramData%\goway\P\host-journal.json` with the
state it replaced; the journal is replayed backwards on uninstall):

| What | Where | Revert |
|---|---|---|
| Add/Remove Programs entry | `HKLM\...\Uninstall\P-host` | removed (reverted last) |
| OpenSSH Server capability | `OpenSSH.Server~~~~0.0.1` (`Add-WindowsCapability`) | removed **only if the install added it**: a capability already present records nothing and stays |
| the capability's own firewall rule | `OpenSSH-Server-In-TCP`, which Windows creates open to every address and profile | narrowed to Private and Domain networks and the local subnet (plus `--allow-from`), only when the install caused the rule to exist; restored to Windows' own default on uninstall. A rule that was already there is the user's: it is not touched, and the installer warns if it admits every address |
| a firewall rule of goway's | `OpenSSH SSH 22` (`P OpenSSH SSH 22` for a test profile), inbound TCP 22, Private and Domain profiles, local subnet only | deleted |
| default shell | `HKLM\SOFTWARE\OpenSSH\DefaultShell` = Windows PowerShell 5.1 by absolute path | the previous value (or none) is put back; the installer says when it replaces another shell |
| the key | one line tagged `# goway-setup P` in `%ProgramData%\ssh\administrators_authorized_keys` for an administrator account (DACL `D:P(A;;FA;;;SY)(A;;FA;;;BA)`, SYSTEM and Administrators only), or in `%USERPROFILE%\.ssh\authorized_keys` for a standard account (the user and SYSTEM only) | only that line is removed, the file is deleted only if the install created it, and the DACL before the install is restored (a key file the user already had keeps its other keys) |
| the sshd service | automatic start, running | stopped and set back to its previous startup type only if it was not running or not automatic before; a service that was already automatic and running is left as it was |

Native installs keep sshd on port 22: `--port` must be 22 or absent, because moving the port means
editing the user's `sshd_config`, which goway does not do.

Safety notes:

- Nothing is restarted: an sshd that already runs is left running (the install only changes what is
  missing), and WSL is not touched at all.
- The elevated uninstall rebuilds the set of changes the install could have made from
  `host-settings.json` (the key, the account and the profile) and refuses a journal entry outside it, as
  for the WSL host: a planted entry cannot make an elevated uninstall edit another file, service or
  firewall rule.
- Only the capability named above, the service `sshd` and the rule `OpenSSH-Server-In-TCP` can be
  acted on; any other name is refused before PowerShell runs. The sshd resource's name carries what to
  restore (`sshd:Manual:stopped`), because removal is given the name only.
- The DACL is read back and written as a DACL only (the owner is not changed). Windows adds the
  auto-inheritance flags `AI` and `AR` when it reads a DACL; goway drops them when it compares and records,
  so a restored DACL is the same entries and protection but without those two flags.

What was and was not proven: the plan, the journal, the exact reversal on a model machine (with a
pre-existing capability, sshd, shell and key file), the journal check the elevated uninstall applies and every
PowerShell script (through a scripted fake) are tested on any machine (`crates/goway-setup/tests/native.rs`).
The PowerShell scripts themselves have **not** been run on a real Windows machine yet: the one test
helper has a hand-made Windows OpenSSH on port 22 that its owner uses, and a native install would change
that sshd (a second sshd on another port would need its own configuration, which this version
does not create), so it was not tried there. Before relying on it, run it on a machine you can spare,
in a test profile (`--profile goway-test`), and compare what `uninstall` leaves with what you had.

## Hardening notes (elevation, relay, user names)

* **The UAC relaunch elevates a locked copy, not the download.** Before asking Windows for
  administrator rights, the install copies its own exe into a fresh, randomly named directory under the
  user's temp folder (owner-only ACL), reopens the copy with a share mode that denies writing,
  renaming and deleting for the whole elevated run, hashes it through that handle and passes the
  SHA-256 on the elevated command line; the elevated side hashes its own image and refuses to continue on
  a mismatch. Another process of the same account can therefore no longer swap the file between
  the start of goway-setup and the UAC prompt. The remaining window is between process start
  and the copy (the first thing the install does); telling a swapped download from the real one before
  that needs a code signature.
* **Elevated edits do not follow links.** `.wslconfig` is a file the user controls; the elevated child
  opens it without following symbolic links or reparse points and refuses one (so a link to an
  administrator-only file cannot be written through). The journal's recorded prior values are
  checked against the change that produced them before an uninstall replays them.
* **Names printed for you to paste are validated.** The WSL user shown in the `goway add` command must be
  a name `useradd` accepts; otherwise the command shows `YOUR-LINUX-USER` and a warning. All
  system-derived text (distro and user names, fingerprints, netsh and PowerShell output) has control and
  bidi characters replaced before it is printed.
* **The relay's forward target is checked twice** (by goway-setup when it creates the relay and by the
  refresh script every five minutes): a private address, inside the WSL adapter's subnet, not the
  gateway. Anyone with root in the distro can make `hostname -I` print anything; none of it can steer the
  relay to a public address or another machine. On a mirrored machine (no WSL adapter) a forced
  `--network nat` install is refused and rolled back.
* **Known limit of the relay task, and what is done about it.** The task runs elevated as you, in your
  environment, because only your user can see the distro. The script and every path it uses are
  administrator-only or absolute. The one thing a script cannot protect itself from is what the runtime
  loads before the first script line: Windows PowerShell is a .NET program, and the .NET runtime loads a
  profiler DLL named by the user-level variables `COR_ENABLE_PROFILING` and `COR_PROFILER` (and the
  `CORECLR_*`, `COMPlus_*`, `DOTNET_*` families) into the process. So the task does not start PowerShell
  directly. It starts a native `cmd.exe` (not .NET, so it ignores those variables) with `/D`, so your
  `HKCU\Software\Microsoft\Command Processor\AutoRun` command is skipped; that `cmd.exe` deletes from its own
  environment every variable starting with `COR_`, `CORECLR_`, `COMPlus_` or `DOTNET_` and the variables
  `PSModulePath`, `PSExecutionPolicyPreference`, `__PSLockdownPolicy` and `__COMPAT_LAYER`, and then starts
  PowerShell, which inherits the cleaned environment. All three programs are named by absolute path under the
  system directory Windows reports.

  What is still true, honestly: (1) this has been checked in tests that build the command and decide which
  names it removes, not by running a profiler against an elevated task on a real machine, so it is a
  mitigation of the cheap routes through the environment, not a proof. (2) An account that is itself an
  administrator and runs a hostile program at normal integrity is not protected by User Account Control:
  Microsoft does not treat the step from "you" to "you, elevated" as a security boundary, and such a
  program has other ways to get elevated (it can ask Windows to run an auto-elevating program, edit your
  other per-user settings, or wait for you to approve a prompt). This change closes the silent
  environment routes into this one task; it does not make an administrator account safe against its own
  malware. (3) The variable list is a list: a new runtime knob that loads code from the environment is
  not covered until added to `SCRUB_PREFIXES` or `SCRUB_NAMES` in `crates/goway-setup/src/relay.rs`.
  Treat the helper's Windows account as you treat any administrator account. The alternative that would
  remove the question, a small signed helper program instead of a script, is not built.

## Giving WSL more of the machine

`goway doctor HELPER` shows what WSL got of the laptop's RAM, swap and processors (the laptop's own
numbers come from `powershell.exe` through interop, so a helper with interop off cannot be compared)
and warns when WSL is far below a suggestion that leaves Windows at least 4 GiB or 25% of the RAM
(memory under 60% of it, processors under 70%, swap under half). It prints the exact command:

    goway-setup.exe tune --memory 12GB --swap 6GB --processors 14 [--nested-virtualization true]

`tune --sparse` (and `install --host`, unless WSL is older than 2.0) also marks the distro's virtual disk
sparse with `wsl --manage DISTRO --set-sparse true`, journaled and undone with `--set-sparse false`, so space
goway frees inside WSL flows back to the Windows drive (see docs/usage.md, "The disk budget").

`tune` edits only the `[wsl2]` keys you name in `%UserProfile%\.wslconfig`, through a journal of its own
(`tune-journal.json` in the profile's per-user state directory: `.wslconfig` is yours, so no administrator
rights are involved and the elevated host uninstall never reads this journal). Running it again extends
the journal. `goway-setup uninstall --host` first restores the exact previous `.wslconfig` values (or
removes the keys it added), in your own process. The new values apply only after `wsl --shutdown`, which
stops every process in every WSL distro, so `tune` refuses while goway jobs hold work directories in the
distro (`--yes` overrides that and then never restarts WSL for you), explains the consequence and asks
before it runs the restart, and never runs it without a console. After the shutdown it starts the
**logon keepalive task** (Limited) so WSL comes back with non-elevated interop; it never starts WSL
itself with `wsl.exe -d` (see "WSL interop and elevation"). If only a boot keepalive exists, or the task
cannot be started, it tells you to start WSL from a normal terminal. Never restart it from an
administrator terminal.

GPU: when Windows lists a GPU that WSL cannot see, `goway doctor` says to install the current Windows
driver with WSL support (never a Linux driver inside WSL) and to run `wsl --shutdown`. When an NVIDIA GPU
is visible but `nvcc` is missing, `goway doctor HELPER --fix --rsudo` installs NVIDIA's `cuda-toolkit`
from the WSL-Ubuntu repository (x86_64, apt, no driver) and records it for `goway uninstall`.

## WSL interop and elevation

WSL interop lets any process in the distro run Windows programs (`powershell.exe`, `cmd.exe`). They run
with the token of **whatever started WSL**, not of the user inside the distro; over ssh into WSL interop
works too (it falls back to `/run/WSL/1_interop`). If WSL was started by an elevated process, everyone
who can log in to the distro is a Windows administrator. Measured on real helpers: WSL started by the
logon task (Interactive, filtered token) has non-elevated interop; WSL started by a boot task with an
`S4U` logon and run level `Limited` has **elevated** interop for an administrator account, because UAC
filtering applies only to interactive logons, and it stays elevated across `wsl --shutdown` and a restart
through that task.

What goway does about it:

* **Elevated goway code never starts WSL.** The elevated install and uninstall ask
  `wsl.exe --list --running` first and stop with "start it from a normal terminal" when the distro is
  not running; the NAT relay refresh task (highest privileges) exits quietly when the distro is not
  running instead of calling `wsl.exe -d` on it. WSL is started by the Limited keepalive task or by you.
* **`--keepalive boot` on an administrator account is refused**, unless the distro's `/etc/wsl.conf`
  already has `[interop] enabled=false`, or you pass `--allow-elevated-wsl` (the install then prints
  the risk). goway refuses rather than editing `wsl.conf` itself: disabling interop also removes
  `powershell.exe` from the distro (goway's own interop transport on a laptop needs it) and takes effect
  only after a WSL restart that goway never does unasked. The default logon keepalive stays the
  recommended mode.
* **`goway doctor` and `goway status` warn** (`wsl interop`, SECURITY) when a WSL helper's interop runs as
  administrator, and report disabled interop as safe. The probe gives `powershell.exe` 3 seconds, because
  with interop disabled it neither runs nor fails for about 10 seconds. Fix: from a normal, non-admin
  terminal run `wsl --shutdown`; the logon keepalive restarts it limited. Do not start WSL from an
  administrator ssh session (or an elevated terminal) on the Windows side.

## Elevating a Windows-side step from your main laptop

`goway add HELPER --rsudo` and `goway ssh setup HELPER --rsudo` run a
Windows administrator step on a helper without anyone typing a password
into goway. Set it up once, whichever you prefer:

1. Unattended: give the helper's Windows OpenSSH server an administrator
   account that logs in with a key. On the helper, as administrator, put your
   public key in `%ProgramData%\ssh\administrators_authorized_keys` (owned
   by Administrators and SYSTEM only), then pass `--windows-admin THAT-USER`.
   goway logs in once with its normal pinned host key and your own key.
2. Attended (for helpers that run WSL; goway's library supports it, no
   command uses it yet): when someone is logged in at the helper, goway
   starts `goway-setup` from the helper's WSL, so Windows shows its UAC
   prompt on that desktop and the person at the helper approves it.
3. Otherwise goway stops and prints the exact command to run in an
   administrator PowerShell, and `goway add` can be repeated afterwards.

**Which `goway-setup` the administrator step runs.** Never "whatever the name
`goway-setup` finds on PATH": on an administrator token that would run a
program any process of the Windows user could have planted first (the
per-user `WindowsApps` folder leads a default PATH and is writable by the
user). The step script ignores PATH, PATHEXT and the current directory and
looks only in fixed protected places: `%ProgramFiles%\goway\goway-setup.exe`
and `%ProgramData%\goway\<profile>\bin\goway-setup.exe` (the copy a host
install keeps in its administrator-only directory). It runs a copy by its
absolute path only when the file and each directory above it (up to the first
one under `%ProgramFiles%` or `%ProgramData%`) is owned by SYSTEM,
Administrators or TrustedInstaller, is not a link, and grants nobody else
write, append, delete, permission or owner rights. Otherwise it exits with a
message and goway falls through to route 3. So the first `--rsudo` on a helper
whose only goway-setup is the one you downloaded (a per-user location) is
refused on purpose: run the printed command yourself, in an administrator
PowerShell, from the installer you chose; later steps use the protected copy.
Tested with a model of the ACL lookup under a real PowerShell:
`crates/goway/tests/winadmin_setup.rs`.

An administrator session must never start WSL: WSL started from an
elevated token lends that token to every WSL user through interop. goway's
steps that touch a distro first run `wsl --list --running` (with
`WSL_UTF8=1`) and stop when the distro is not running. Do not add commands
that run `wsl -d` to an administrator ssh session either. Note that
`goway-setup tune` changes the account's own `.wslconfig`, so run such steps
as the Windows user that owns the WSL installation.

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
  or CIDR; `0.0.0.0/0` and `::/0` are refused because they would undo the scoping, as is any range
  that contains `0.0.0.0` or `::` (a firewall reads those as "every address") or IPv4 multicast and
  broadcast space) and are kept in the host settings so uninstall rebuilds the same plan. Ranges wider
  than `/8` (IPv4) or `/16` (IPv6) are refused too, because two halves such as `0.0.0.0/1` and
  `128.0.0.0/1` would re-open every address; `--allow-wide` lets a deliberately wide range such as
  `128.0.0.0/1` through, and the Private and Domain profile limit still applies.
* **Password login is switched off by default** by the `PasswordAuthentication no` drop-in, but only
  when the distro's default user already has an authorized key, so you cannot lock yourself out.
  `--no-harden` opts out. With no key yet, the install finishes with a loud warning: run
  `goway ssh setup HOST` from your main laptop, then `goway doctor HOST --fix --rsudo` (or uninstall
  and rerun the install) to turn passwords off.
* **Public networks are called out.** If a connected network is classified Public (Windows does
  this for cafe and hotel Wi-Fi), the install warns, in plain words, that goway only opens its door on
  home or work networks, so the helper cannot be reached over this one, and shows the one-line
  command to mark a network you trust as Private. `--dry-run` shows the same warning.

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
  snapshots before and after uninstall are identical. It also checks the host's Add/Remove Programs
  entry after install and, in the first case, uninstalls by running that entry's own
  `UninstallString` (the protected copy, not the downloaded file); the snapshots include the HKLM
  entries, so the entry must be gone afterwards.

</details>

## Why uninstall provably restores the machine

<details><summary>Details</summary>

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

</details>

## The running-exe problem

<details><summary>Details</summary>

Windows cannot delete a running executable, and the uninstall entry runs the installed
`goway-setup.exe`. So when `uninstall` finds it is running from a file the journal installed, it
copies itself to `%TEMP%\goway-uninstall-<pid>\`, starts that copy detached (job breakaway, so it
survives an OpenSSH session ending) with its output in `uninstall.log` beside it, and exits. The
copy retries any locked file for about five seconds (covering the parent still exiting), reverts the
journal, broadcasts the environment change, and finally schedules a hidden `cmd` that deletes the
copy, its log and its directory after it exits (the standard self-delete trick). When run from
anywhere else (for example the downloaded installer) uninstall is synchronous and its exit code is
the result.

</details>

## Proving the host component on a live machine

<details><summary>Details</summary>

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

`GOWAY_CASES="nat" scripts/windows/roundtrip-host.sh Helios` runs only the third case, which
forces `--network nat` on its own port 2399 and needs no WSL restart or change of `.wslconfig`: it
checks the relay, the script file and the refresh task (highest privileges, logon trigger repeating
every 5 minutes), sets the relay to a wrong address by hand and starts the task to see it put the
WSL address back, then points the relay at the live sshd on the host's own 127.0.0.1:2222 for one
full login through port 2399 (entered through Windows OpenSSH's TCP forwarding, because a login from
a Public network is rightly blocked by the firewall scope and a mirrored host cannot reach its own
address), and requires the portproxy table, tasks, firewall rules, `.wslconfig` and `%ProgramData%`
to be identical after uninstall. A mirrored machine's WSL address is the Windows address itself, so
this proves the mechanism but not a real NAT address change; that needs a Windows 10 machine.

</details>

## Building

<details><summary>Details</summary>

    scripts/windows/build.sh                      # x64: needs mingw-w64 on Linux/WSL
    scripts/windows/build.sh --arch arm64         # ARM64: cargo-xwin on Linux, MSVC on Windows
    scripts/windows/build.sh --arch all           # both

This builds `goway.exe`, then the installer with it embedded (`GOWAY_PAYLOAD` is read by the
crate's build script; without it the crate still builds, with an empty payload, and refuses to
install). Output in `dist/`: `goway-setup.exe` (x64) and `goway-setup-arm64.exe` (ARM64). The
ARM64 installer is a native aarch64 binary and runs on Windows on ARM without emulation; the x64
one runs there under emulation. `scripts/windows/pe-machine.ps1 FILE` prints which CPU a file is
built for. The release workflow runs this same script on `windows-2025` (x64) and `windows-11-arm`
(ARM64), checks each file's CPU, and runs it with `--version`, so the ARM64 files are proven to
run natively on every release build.

</details>

## Proving it on a machine

<details><summary>Details</summary>

    scripts/windows/roundtrip.sh Helios          # host name resolved through mDNS at run time

The script needs key-based SSH to the host, uploads the installer and helper scripts to
`~\goway-roundtrip`, removes them afterwards, and touches only the `goway-test` profile.

</details>
