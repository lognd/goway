<p align="center"><img src="https://raw.githubusercontent.com/lognd/goway/main/docs/assets/goway-banner.svg" alt="goway: run it on your other laptops. Your build or test command runs natively on the least busy laptop on your network, from your git work tree." width="100%"/></p>

# goway

goway lets you type a build or test command on your main laptop and have
your other laptops do the work.
- It copies the files git shows for your project, including uncommitted
  changes, to the least busy helper laptop on your network.
- It runs the command there natively, with a warm build cache, at low
  priority, so the person using that laptop is not slowed down.
- It streams the output back and exits with the command's own result.

Helpers are found by name, not by fixed address, so laptops on changing
Wi-Fi addresses keep working. A test run can also be split across
several helpers. goway is built for Rust projects worked on from many git
worktrees at once, and it runs any command. Every change goway makes to
a machine is journaled, so uninstalling restores it exactly.

[![CI](https://github.com/lognd/goway/actions/workflows/ci.yml/badge.svg)](https://github.com/lognd/goway/actions/workflows/ci.yml)
[![GitHub release](https://img.shields.io/github/v/release/lognd/goway?include_prereleases&sort=semver)](https://github.com/lognd/goway/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![MSRV 1.98](https://img.shields.io/badge/MSRV-1.98-orange.svg)](#versioning-and-compatibility)
[![Platforms](https://img.shields.io/badge/platforms-linux%20%7C%20windows%20%7C%20wsl-lightgrey.svg)](#install)

## Install

goway runs on two kinds of computer: your **main laptop**, the one you
type on, and one or more **helper laptops**, the ones that do the work.
All of them must be on the same network.

| Operating system | As your main laptop | As a helper laptop |
|---|---|---|
| Windows 10 (21H2 or newer) and 11 **with WSL** | yes: install inside WSL ([Linux and WSL](#main-laptop-linux-or-wsl)) | Windows 10 (21H2 or newer) and Windows 11, with WSL: one installer ([Windows with WSL](#helper-windows-with-wsl)). Windows 11 22H2 or newer uses mirrored networking; Windows 10, and Windows 11 in WSL's default NAT mode, use a port relay that goway sets up and keeps current (Windows 10 and NAT mode: untested by the maintainer on real hardware; the relay was verified on Windows 11 by simulation) |
| Windows 10/11 **without WSL** | yes: `goway-setup.exe install` ([Windows](#main-laptop-windows-without-wsl)) | no: a helper needs Linux; install WSL first |
| **Linux** (Ubuntu, Debian, Fedora, Arch, ...) | yes: one command ([Linux and WSL](#main-laptop-linux-or-wsl)) | yes: a few manual steps ([Linux](#helper-linux)) |
| **macOS** (Apple Silicon and Intel) | yes: release binaries and one command ([macOS](#main-laptop-macos)); untested by the maintainer, CI-built and CI-tested only | no: goway's helper side needs Linux tools macOS lacks |

**Downloads:** every version's ready-made files (`goway-setup.exe`,
the Linux programs, `install.sh` and the checksums) are on the
[Releases page](https://github.com/lognd/goway/releases). The newest
one is always at
[releases/latest](https://github.com/lognd/goway/releases/latest).

On your main laptop, one command installs goway:

```bash
# Linux, or WSL on Windows
curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash
```

```powershell
# Windows without WSL (download goway-setup.exe from the Releases page first)
.\goway-setup.exe install
```

```powershell
# a Windows helper laptop with WSL (one command; it prints what to run next)
.\goway-setup.exe install --host
```

From v0.1.0 goway is also on crates.io and PyPI:

```bash
cargo install --locked goway   # available from v0.1.0 (needs a Rust toolchain)
uv tool install goway          # available from v0.1.0
pipx install goway             # available from v0.1.0
```

Each download is checked against the release's published checksums.
The helpers do the building, so they need the toolchain for your
project, such as Rust. `goway add` installs it for you. The main laptop
needs only goway, git and ssh.
Words you have not seen before are explained in
[docs/glossary.md](docs/glossary.md).

## Sixty-second tour

```bash
goway add <YOUR-COMPUTER-NAME-HERE> ...     # once per helper: the line its installer printed
goway status                                 # are the helpers reachable, how busy are they?
goway run -- cargo nextest run --workspace   # run on the least busy helper
goway run --shard 2 -- cargo nextest run     # split one test run across two helpers
goway doctor --fix                           # check everything, fix what it can
goway uninstall                              # remove goway everywhere, exactly
```

<p align="center"><img src="https://raw.githubusercontent.com/lognd/goway/main/docs/assets/goway-tour.svg" alt="A goway session: goway status lists two idle helper laptops, then goway run --shard 2 splits a cargo nextest run across both, 118 and 127 tests pass, and goway finishes with exit 0." width="800"/></p>

Read top to bottom:

- **status**: each helper's current address and how goway found it,
  its processor type and cores, its load, and goway's jobs on it.
- **run --shard 2**: goway picks the two least busy helpers. It copies
  only what changed since the last run and gives each helper half of
  the tests (nextest's own `--partition`). Each output line is
  prefixed with the helper it came from.
- **done / note**: every goway line says its kind in words, and goway
  exits with the first failing shard's code, or 0.

## Quick start: your first helper

### 1. On each helper laptop (once)

1. Download `goway-setup.exe` from the
   [latest release](https://github.com/lognd/goway/releases/latest).
2. Open the Start menu, type **PowerShell**, and open it. In the folder
   where the download went (usually `cd ~\Downloads`), type:

       .\goway-setup.exe install --host

3. Windows asks "Do you want to allow this app to make changes?".
   Choose **Yes**.
4. At the end, the installer prints one line starting with `goway add`.
   Copy that line or write it down; you need it in step 3.

<details><summary>What this changes on the helper laptop, and why</summary>

- **Mirrored networking** (Windows 11 22H2+) or, on Windows 10 and
  Windows 11 in NAT mode, **the port relay** goway-setup installs, so
  your main laptop can reach the Linux inside this laptop. You do not
  set either by hand.
- **An ssh server inside WSL on port 2222.** ssh is the secure way one
  computer logs into another. Password logins are turned off once a key
  is set up, so only your main laptop's key gets in.
- **Two firewall rules for port 2222**, limited to home or work networks
  and your local network. A cafe's Wi-Fi cannot reach it.
- **A "keepalive" task** that keeps WSL running after you sign in, so
  the helper is reachable.
- **An entry in Settings > Apps** named "goway helper". It removes every
  one of these changes again.

Every change is recorded together with what was there before, and the
uninstall puts back exactly that. Details:
[docs/install-windows.md](docs/install-windows.md).
</details>

<details><summary>If the installer says WSL is missing</summary>

It stops before changing anything and prints the steps:
1. Open PowerShell as administrator (right-click it in the Start menu,
   "Run as administrator").
2. Type `wsl --install -d Ubuntu` and restart the laptop.
3. When Ubuntu opens, choose a Linux user name and password. Remember
   both: the password is the one goway asks for in step 3.
4. Run `.\goway-setup.exe install --host` again.
</details>

<details><summary>If the installer warns about a "Public" network</summary>

Windows treats networks it does not know (cafes, airports, and often
your own Wi-Fi when you first connect) as **Public**. goway only opens
its port on home or work (**Private**) networks. On your own network,
open Settings > Network & internet > Wi-Fi > your network and choose
**Private network**. The installer also prints the one-line command
that does the same.
</details>

### 2. On your main laptop (once)

**Linux or WSL:** open the terminal and type:

    curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash

Then close the terminal and open it again.

**Windows (without WSL):** download `goway-setup.exe` from the same
release and run `.\goway-setup.exe install` in PowerShell.

<details><summary>What this changes on your main laptop</summary>

- **Linux:** it copies one program to `~/.local/bin/goway`. If that
  folder is not already on PATH, it adds one marked line to
  `~/.profile`. The download is checked against its published checksum
  first. No administrator rights are used. It keeps a record of what it
  added, so `goway uninstall` can remove exactly that.
- **Windows:** it copies `goway.exe` into your user's program folder
  and adds that folder to your PATH. It also adds an entry in
  Settings > Apps that removes both again.
</details>

### 3. On your main laptop, once per helper

Type the line the helper's installer printed in step 1.4. It looks like
this, with the helper's own values in place of the `<...>` parts:

    goway add <YOUR-COMPUTER-NAME-HERE> --fingerprint <FINGERPRINT-FROM-THE-INSTALLER> --user <YOUR-LINUX-USER-NAME-HERE> --rsudo

goway shows what it is about to do, asks for the **helper's Linux
password** once (not its Windows password), and finishes with
`goway: done: <YOUR-COMPUTER-NAME-HERE> is ready`.

<details><summary>What to put in place of each &lt;...&gt; and how to find it</summary>

The helper's installer prints all three values at the end. If you no
longer have that output, run `.\goway-setup.exe status --host` on the
helper to see it again, or find each value by hand:

- `<YOUR-COMPUTER-NAME-HERE>`: the helper laptop's name, which goway
  uses to find it on the network.
  - On Windows: Settings > System > About, the line **Device name**,
    for example `DESKTOP-4K2J9`.
  - On a Linux helper: type `hostname` in its terminal.
  - Upper or lower case does not matter.
- `<YOUR-LINUX-USER-NAME-HERE>`: the user name you chose when Ubuntu
  (WSL) first started on the helper. In the helper's Ubuntu window,
  type `whoami`. goway asks for this user's password.
- `<FINGERPRINT-FROM-THE-INSTALLER>`: the helper's identity, which looks
  like `SHA256:` followed by 43 letters and digits. goway compares it,
  so it never sends your password to a different machine that answers
  to the same name. In the helper's Ubuntu window:
  `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`.
</details>

<details><summary>What this changes, and why goway needs no password afterwards</summary>

- **The helper's identity:** goway compares the helper's fingerprint
  with the one in the command, so it never talks to an impostor. It
  then remembers that identity in `~/.config/goway/known_hosts`.
- **A key:** goway uses your existing ssh key. If you have none, it
  creates one in `~/.config/goway/`. It adds one tagged line for that
  key to `~/.ssh/authorized_keys` on the helper. From then on your
  laptop proves who it is with the key, so no password is needed.
- **Build tools on the helper:** cargo-nextest and sccache are
  installed for the helper's user. Downloads are checked against fixed
  checksums.
- **With `--rsudo`:** goway lists the changes that need administrator
  rights on the helper, with the reason for each (for example the C
  compiler Rust needs, or turning off password logins). It asks once,
  then runs them all with one `sudo`. You type the password into the
  helper's own sudo; goway never sees it.

`goway uninstall` reverses all of this. See [Uninstall](#uninstall).
</details>

### 4. Try it

    goway status                                  # are the helpers reachable?
    cd ~/my-project                               # a folder kept in git
    goway run -- cargo nextest run --workspace    # runs on the least busy helper

Everything after `--` is the command the helper runs.

<details><summary>What happens during a run, and what is sent</summary>

1. goway asks every helper how busy it is and picks the least busy
   one.
2. It copies the files git shows for your project. It never sends
   `.git`, env files, credential files or private keys.
3. It runs the command in a private folder on the helper, at low
   priority so the person using that laptop is not slowed down.
4. It shows the output as it arrives and ends with the command's own
   result.

Builds stay fast because each helper keeps a build cache, which
disappears on its own after 7 idle days. Details:
[docs/usage.md](docs/usage.md).
</details>

## Install details for each operating system

### Main laptop: Linux or WSL

    curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash

This works on any Linux on Intel/AMD (x86_64) or ARM (aarch64), and
inside WSL. It needs `curl`, `git` and `ssh`, which most systems have.
If one is missing, `goway add ... --lsudo` installs it on Ubuntu or
Debian. Remove goway again with `goway uninstall`.

### Main laptop: Windows (without WSL)

1. Install **Git for Windows** (https://git-scm.com). Windows 10 and 11
   already include the ssh client goway uses.
2. Download `goway-setup.exe` from the
   [latest release](https://github.com/lognd/goway/releases/latest).
   On a Windows-on-ARM laptop, take `goway-setup-arm64.exe`.
3. In PowerShell, in the download folder:

       .\goway-setup.exe install

4. Open a new PowerShell window and type `goway --help`.

Remove it in Settings > Apps > goway, or with
`goway-setup.exe uninstall`. Details:
[docs/install-windows.md](docs/install-windows.md).

### Main laptop: macOS

The release has goway binaries for Apple Silicon (`aarch64-apple-darwin`)
and Intel (`x86_64-apple-darwin`). The same one-line installer as on Linux
picks the right one, verifies it against `SHA256SUMS` with `shasum -a 256`,
and installs it to `~/.local/bin`:

    curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash

The installer and `goway uninstall` use only what macOS ships (bash 3.2 and
the BSD tools). The maintainer has no Mac: the binaries are built, and the
client tests and installer scripts are run, on GitHub's macOS runners only.
Without the release, build from source with Rust (https://rustup.rs):

    cargo install --locked --git https://github.com/lognd/goway goway

A Mac cannot be a helper: goway's helper side needs Linux tools
(GNU findutils, `flock`, `setsid`).

### Helper: Windows with WSL

See step 1 of the [quick start](#1-on-each-helper-laptop-once). The
helper needs WSL 2 and Ubuntu. Windows 10 (21H2 or newer) and Windows 11
both work. With Windows 11 22H2 or newer, goway-setup uses WSL's mirrored
networking. On Windows 10, or with WSL in NAT mode (`networkingMode=nat`),
it instead sets up a Windows port relay (`netsh interface portproxy`) on
the helper's port; a scheduled task re-points the relay whenever WSL's
internal address changes. The firewall rules still limit who can connect,
the `goway add` line is the same, and `goway-setup uninstall --host`
removes the relay and the task. `--network mirrored|nat|auto` overrides
the choice. Windows 10 and NAT mode are untested by the maintainer on real
hardware.
If WSL is missing, the installer stops and tells you how to add it.

### Helper: Linux

There is no installer for Linux helpers yet. These steps are for
Ubuntu or Debian; other distributions use their own package names.

1. On the helper, install the ssh server and goway's needs:

       sudo apt-get install -y openssh-server git tar util-linux findutils

2. Make sure your main laptop can reach it on port 22. Many desktop
   systems allow this by default; with the `ufw` firewall, run
   `sudo ufw allow from <YOUR-LOCAL-NETWORK> to any port 22`, for
   example `192.168.1.0/24`.
3. Find its name with `hostname`, and its fingerprint with
   `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`.
4. On your main laptop:

       goway add <YOUR-COMPUTER-NAME-HERE> --port 22 --fingerprint <FINGERPRINT> --user <YOUR-LINUX-USER-NAME-HERE> --rsudo

goway finds Linux helpers as `<name>.local` when the helper runs the
`avahi-daemon` service (Ubuntu desktop does). Otherwise, add
`--address <ITS-IP-OR-DNS-NAME>`.

## Day to day

- `goway run -- COMMAND` runs a command on the least busy helper;
  `--host NAME` picks one; `--shard 2` splits a test run across two
  helpers.
- `goway status` shows how busy each helper is.
- `goway doctor` checks everything and says how to fix what is wrong.
- Press Ctrl-C to stop a run; goway stops it on the helper too.

Using a screen reader? Add `--plain` (or set `GOWAY_PLAIN=1`), and
tables are printed as labelled lines such as
`host: <YOUR-COMPUTER-NAME-HERE>, load: 0.40, jobs: 0`. Every goway line starts with its
kind in words (`error:`, `warning:`, `done:`, `next:` ...), so color is
never needed.

Old copies and caches on the helpers are cleaned up automatically
(`goway gc --dry-run` shows what would go). More:
[docs/usage.md](docs/usage.md).

## Uninstall

On your main laptop:

    goway uninstall              # lists everything it will remove, then asks
    goway uninstall --rsudo      # also undoes the administrator changes on helpers

Then, on each helper laptop, remove **goway helper** in Settings > Apps.

<details><summary>What each step removes, and what it leaves</summary>

`goway uninstall` removes the following, helpers first, then this
laptop:
- goway's folder on each helper
- the tools goway installed there
- goway's key line on each helper
- the "no password login" setting, with `--rsudo`
- goway's settings and keys on this laptop
- the goway program itself

If a helper is switched off, it stops before touching this laptop, so
you can run it again later.

It deliberately leaves system packages it installed on helpers, such
as the C compiler, because other programs may rely on them. It lists
each one with its removal command.

The helper's entry in Settings > Apps undoes everything the helper
installer did, exactly as it was before.
</details>

## How install and uninstall work: the journal

Everything goway's installers and setup commands change is meant to be
**completely reversible**. That is possible because goway keeps a
**journal**: a written record of each change it makes, together with
what was there before.

<details><summary>How the journal works</summary>

1. **Before each change, goway writes it down.** The journal entry
   says what will change (a file, a setting, a firewall rule, a line
   in a file, a folder) and records the state before it: the old
   contents, the old value, or "this did not exist". The entry is saved
   to disk *before* the change is made, so even a crash or a power cut
   in the middle leaves a record of everything that was touched.
2. **Things that were already in place are recorded as "no change".**
   If the PATH already held goway's folder, or a setting already had
   the value goway wants, goway writes that down and does not touch it.
   Uninstall then leaves it alone too, because it was yours.
3. **Uninstall replays the journal backwards**, newest entry first, and
   puts each recorded "before" state back:
   - a file goway created is deleted
   - a file goway changed gets its old contents back
   - a setting gets its old value back, or is removed if it did not
     exist
   - a folder goway created is removed once it is empty
   - a rule or task goway created is deleted
4. **Your later changes are never overwritten.** If you edited something
   after goway installed it, uninstall leaves your version in place and
   tells you, instead of clobbering it.
5. **Running uninstall twice is harmless.** Entries already undone are
   skipped, and an interrupted uninstall can simply be run again.
</details>

<details><summary>Where each journal lives</summary>

| What | Journal |
|---|---|
| Linux / WSL main laptop install | `~/.local/state/goway/install-journal` |
| Windows main laptop install | `%LOCALAPPDATA%\goway\install-journal.json` |
| Helper laptop install (Windows) | `%ProgramData%\goway\goway\host-journal.json`, writable only by administrators, so no ordinary program can plant entries that an administrator's uninstall would then carry out |
| Key setup for a helper (`goway add`) | `~/.config/goway/ssh-setup-<helper>.json` on the main laptop; it records the changes on the helper too |
| Tools `goway add` installed on a helper | `~/.config/goway/installed-<helper>.json` on the main laptop |
</details>

<details><summary>What "completely reversible" covers, and the two deliberate exceptions</summary>

Uninstalling restores every file, setting, PATH entry, registry value,
firewall rule, scheduled task, folder and `authorized_keys` line that
goway added or changed, exactly as it was before.

The two exceptions are deliberate:
- **System packages installed on a helper with `--rsudo`**, such as the
  C compiler Rust needs, stay installed, because other software on that
  helper may come to rely on them. `goway uninstall` lists each one
  with the command that removes it.
- **Your own changes after install** are kept, as described above.

goway's build caches on helpers are not journaled; they are goway's own
files in one marked folder, and `goway uninstall` deletes that whole
folder.
</details>

<details><summary>How this is checked</summary>

- **Property tests** generate thousands of random machines and install
  plans. For each one they check that uninstall after install gives
  back exactly the starting machine, that installing twice changes
  nothing more, and that uninstalling twice is harmless
  (`crates/goway-journal/tests/properties.rs`).
- **Snapshot tests on real machines** install and uninstall the Windows
  helper and client components on a test laptop. They require the
  before and after snapshots to be identical. The snapshots cover the
  registry, PATH, firewall rules, scheduled tasks, `.wslconfig` and the
  ssh server configuration (`scripts/windows/roundtrip*.sh`).
- **Linux tests** install, uninstall and compare every file, mode and
  byte of the home folder (`crates/goway/tests/install_scripts.rs`).
  The `goway add` / `goway uninstall` tests do the same for the helper
  side (`crates/goway/tests/ssh_setup.rs`).
</details>

## What goway does

| Command | Purpose | Docs |
|---------|---------|------|
| `goway add HOST` | register a helper: confirmed identity, key login with one password, its toolchain | [docs/ssh-setup.md](docs/ssh-setup.md) |
| `goway run -- CMD` | run a command on the least busy helper; `--host`, `--shard N`, `--keep`, `--report` | [docs/usage.md](docs/usage.md) |
| `goway status` | address, cores, load, jobs and disk of every helper (`--plain` for screen readers) | [docs/usage.md](docs/usage.md#status) |
| `goway doctor [HOST] --fix` | check helpers and this laptop; fix what it can, root fixes with `--rsudo` | [docs/usage.md](docs/usage.md#doctor) |
| `goway gc` | remove stale state on helpers (it also happens on its own) | [docs/usage.md](docs/usage.md#clean-up) |
| `goway uninstall` | remove everything goway added, here and on every helper | [docs/usage.md](docs/usage.md#uninstall) |
| `goway host add/list/remove` | manage the pool by hand | [docs/hosts.md](docs/hosts.md) |
| `goway-setup.exe install [--host]` | Windows installer, main laptop or helper; journaled, exact uninstall | [docs/install-windows.md](docs/install-windows.md) |
| `install.sh` | Linux and WSL installer; checksum-verified download | [docs/install-linux.md](docs/install-linux.md) |

More: [config](docs/config.md), [how helpers are found](docs/hosts.md),
[troubleshooting](docs/troubleshooting.md), [glossary](docs/glossary.md),
[goway versus other build systems](docs/positioning.md),
[design](docs/design.md), [releases](docs/release.md).

## Something went wrong?

See [docs/troubleshooting.md](docs/troubleshooting.md). Every goway
error ends with a `next:` line saying what to try, and `goway doctor`
checks everything at once.

## Development

```bash
git clone https://github.com/lognd/goway.git && cd goway
cargo nextest run --workspace                                # the test suite (Linux or WSL)
cargo clippy --workspace --all-targets -- -D warnings        # lints, warnings are errors
cargo clippy --target x86_64-pc-windows-gnu --workspace --all-targets -- -D warnings
```

The suite never touches a real remote machine: a fake `ssh` runs
goway's remote side locally. The workspace has three crates:
- `goway`: the command-line tool
- `goway-journal`: the reversible change journal
- `goway-setup`: the Windows installer

The work itself is tracked with frob, a work-accounting tool: its
tickets and evidence live in `tickets/`, and the changelog fragments
live in `changelog.d/`.

## Versioning and compatibility

goway follows [Semantic Versioning](https://semver.org). Before 1.0, a
minor version may change flags, output layout or the config file; the
config file rejects unknown keys, so a renamed setting is reported
rather than ignored. The minimum supported Rust version is 1.98, pinned
in `rust-toolchain.toml` and checked in CI. Releases are cut as
described in [docs/release.md](docs/release.md).

## Contributing

Contributions are welcome, from a typo fix to a new feature. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request; it
covers the local setup, the gate, the commit format and the
[AI-assisted contributions policy](CONTRIBUTING.md#ai-assisted-contributions).
Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability;
please do not open a public issue for one.

## License

MIT. See [LICENSE](LICENSE).
