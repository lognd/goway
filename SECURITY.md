# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | yes       |

Only the latest 0.1.x release receives security fixes.

## Reporting a vulnerability

Please do not open a public GitHub issue for a suspected vulnerability.

Use GitHub private vulnerability reporting: open the "Security" tab on
[lognd/goway](https://github.com/lognd/goway), then "Report a
vulnerability". This creates a private advisory that only the
maintainer and you can see.

### What to include

- What an attacker can do, and from where: another computer on the
  network, a helper laptop, another user on a helper or on your laptop,
  or a malicious repository.
- A minimal reproduction: the commands, and the goway version
  (`goway --version`) on each machine involved.
- The operating systems: main laptop, helper (Windows build, WSL
  distribution).
- What you expected, what happened, and why you believe it is a
  security issue rather than a bug.

### Response expectations

The maintainer aims to acknowledge a report within 7 days. There is no
bounty program. The fix timeline depends on severity and is discussed
with you in the private advisory.

## Scope notes specific to goway

goway logs into your helper laptops over ssh and runs commands there,
and its Windows installer changes firewall, scheduled-task and WSL
settings with administrator rights. The design and the protections are
described in [docs/positioning.md](docs/positioning.md),
[docs/hosts.md](docs/hosts.md) and
[docs/install-windows.md](docs/install-windows.md). Reports most likely
fall into one of these areas:

- **Host identity.** goway pins each helper's ssh host key under its
  name and only trusts a new helper after the user confirms its
  fingerprint. Any way to make goway send files, commands or a password
  to a different machine is in scope.
- **What is sent.** goway sends the git-visible work tree. It keeps
  secret-looking files local, never reads through symlinked directories,
  passes `--env` values over ssh's stdin, and starts ssh with a minimal
  environment. Any way a secret leaves the main laptop without the user
  allowing it is in scope.
- **Helper side.** goway's state on a helper lives in one marked
  directory, and gc and uninstall remove only entries goway labelled.
  Deleting or changing anything outside it, or another user on the
  helper reaching goway's files, sockets or processes, is in scope.
- **The Windows installer.** The host component runs with
  administrator rights and reads its state only from an
  administrator-only directory. Any way for a non-administrator to
  influence what it does is in scope, and is treated as high severity.
- **WSL interop and elevation.** WSL interop runs Windows programs
  (`powershell.exe`) with the token of whatever started WSL. If an
  elevated process started it, every user who can log in to the distro,
  including over ssh, acts as a Windows administrator. goway's elevated
  code never starts WSL (it only asks `wsl.exe --list --running` and
  refuses to act on a distro that is not running), the NAT relay refresh
  task does the same, and an administrator ssh session on the Windows
  side must not start WSL either. A boot-time keepalive (an S4U task) of
  an administrator account gets the full token whatever its run level,
  so `goway-setup install --host --keepalive boot` refuses for such an
  account unless interop is already disabled in the distro or
  `--allow-elevated-wsl` is given. `goway doctor` and `goway status`
  probe every WSL helper and warn when interop is elevated. A way to get
  goway to start WSL elevated is in scope and high severity.
- **Copy integrity.** goway verifies the helper's copy of your tree by
  SHA-256 before every run and again when a command fails (see
  [docs/usage.md](docs/usage.md)). These checks guard against goway's own
  bugs (a stale or damaged copy), not against a compromised helper: a
  helper under an attacker's control can already report any hash, run any
  command and read everything sent to it. Helper answers are parsed
  strictly and with bounds, but a verified copy is not a trusted helper.
- **Output.** goway strips control and invisible format characters
  (including bidi controls) from everything it prints itself, indents the
  continuation lines of host-supplied text so none can pass for a
  `goway:` line, and caps its length; helper answers are size-capped. Terminal escape injection through goway's own lines is in
  scope. The output of the command you run is passed through byte for byte
  when it goes to a pipe or file; on a terminal goway strips OSC, DCS,
  APC, PM and SOS strings, every CSI except colors, and other control
  characters (`--output=raw` or `GOWAY_OUTPUT=raw` turns that off).
- **Installers and releases.** `install.sh` verifies the release archive
  against `SHA256SUMS`, and releases carry build provenance
  attestations (see [docs/release.md](docs/release.md)).

Out of scope: the commands you run on a helper run as your user there,
with that user's rights. goway does not sandbox them.

If you are unsure whether something is in scope, report it anyway.
