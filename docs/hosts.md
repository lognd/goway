# Hosts without static IPs

A goway host is a **name plus a pinned ssh host key**. Its address is
whatever currently answers with that key. Laptops on DHCP Wi-Fi change
addresses all the time, and the measured network here is a /13 that is
far too large to scan, so goway never depends on an address staying
valid.

## Pinning (identity)

Every ssh call goway makes uses:

    -o HostKeyAlias=goway-<name> -o StrictHostKeyChecking=yes
    -o UserKnownHostsFile=<config dir>/known_hosts -o GlobalKnownHostsFile=none

ssh therefore checks the key under the host's *name*. A stale address
that now belongs to another machine fails the check ("host key
mismatch"), and goway moves on to the next candidate. It never connects
to the wrong machine.

## Resolution order

goway tries candidates in stages and probes each one with its real
remote command, so finding the host costs no extra round trip. A later
stage only runs if every earlier one failed:

1. **cached**: the last address that worked (state file).
2. **peer hint**: an unsigned address learned when this machine paired over the
   LAN (`goway circle pair-host` / `pair-join`; at most 4 per host in the state
   file). It is only a candidate: the pinned host key is verified like any other.
3. **circle roster**: only for hosts the circle roster supplies (label
   `circle`, no `address`): the roster's name through the system resolver, then
   Windows. The host key is still verified against the pinned roster fingerprint.
   A manual `[[host]]` of the same name never uses this stage.
4. **configured**: the host's `address`, through the system resolver,
   then Windows. If neither resolves it, ssh is given the name as is.
5. **dns**: the host name through the system resolver.
6. **mdns**: `<name>.local` through the system resolver (works where
   the OS resolves mDNS, such as Windows, macOS, or Linux with
   nss-mdns).
7. **windows-mdns**: `<name>.local` asked of Windows through WSL interop
   (`powershell.exe Resolve-DnsName`). A WSL client in NAT mode cannot
   do mDNS itself. This takes about 1 to 4 seconds, which is why it is
   last and why the result is cached. `GOWAY_WINDOWS_LOOKUP=0` turns
   it off.

Loopback, link-local and duplicate addresses are skipped. Windows also
answers with unrelated adapters (for example VirtualBox
`192.168.56.1`). Those candidates fail the key check and are skipped.
An address whose key is not pinned, or that refuses authentication, does
not stop the search: another machine may answer first for the name.
goway never uses an address whose key it has not confirmed, so two
machines answering for one name cannot make it pick the wrong one.
When no candidate answers, the error says which case it looks like: a
name that resolves to an unreachable address (client isolation, a
partial VPN), no address at all (mDNS blocked), or several machines
answering for one name. See "A network that hides the helpers" in
troubleshooting.md.
When the port a host would use (the default 2222, or the one cached
from the last success) is closed at an address, and the config does not
pin `port = N`, goway also tries ports 22 and 2222 there, so a helper
that moved between the WSL sshd and Windows OpenSSH is still found; the
port that answers is the one cached. The usual host key check applies
on every port. A host with `port` set in the config is only tried on
that port.
The error lists every candidate tried (with the port) and why it failed, including the
reason a candidate was rejected (for example a hostname that does not
match).

## `goway host add NAME`

```
goway host add <YOUR-COMPUTER-NAME-HERE>   # finds it by name, pins the key
goway host add laptop2 --address <ITS-DNS-NAME-OR-IP>
goway host add box --address 192.168.1.20 --port 22 --user me --max-jobs 2
```

Pinning is trust on first use, so `host add` is strict about which
machine it trusts:

- Ports are tried in order: `--port`, else the default 2222 (WSL sshd),
  then 22. goway looks for the Linux machine first (`uname -s` =
  Linux); a Windows OpenSSH server (which runs `cmd.exe`, has no
  `uname`, and is recognised by that) is only taken when no Linux
  machine answered as this host, after asking it again in PowerShell.
  It is then recorded as `os = "windows"` and never confused with the
  same computer's WSL host. To register the Windows side of a computer
  that also has a WSL helper, add it under another name with an explicit
  port: `goway host add box-win --address Box --port 22`.
- The machine's hostname must match `NAME` (or the first label of
  `--address`), ignoring case and domain. A candidate that fails this
  is rejected and its key is discarded. Giving `--address` as an
  explicit IP means you are vouching for that machine, so the hostname
  check is skipped.
- Candidates are probed against a scratch `known_hosts`. The key the
  machine presents is pinned only after you confirm it. goway shows the
  fingerprint and asks at the terminal, or you pass
  `--fingerprint SHA256:...`. Without either, nothing is pinned.
  Anyone on the same Wi-Fi can answer for `NAME.local`, and a hostname
  is easy to fake, so this confirmation is the check that counts.
  On the helper laptop, get the real fingerprint from its WSL terminal
  with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`.
- Local ssh problems are reported, with only metadata read and never
  key contents: no agent keys and no identity file, an identity file
  readable by others, or no ssh client.

- A Windows OpenSSH server that does not yet accept goway's key (the
  usual state of a new Windows helper) is recognised by its ssh banner
  (`OpenSSH_for_Windows`). With `--address` as an IP or `--fingerprint`
  (so you vouch for the machine, as its hostname cannot be read without
  a login) it is pinned and recorded as `os = "windows"`, appears in
  `goway host list`, and goway prints the Windows fixes instead of the
  Linux password advice.
- `goway host add NAME --windows` only prints those commands (install
  `OpenSSH.Server`, authorize goway's key in
  `C:\ProgramData\ssh\administrators_authorized_keys` with `icacls`, set
  the `DefaultShell` to PowerShell) and changes nothing.

`goway host list` shows the pool with cached addresses.
`goway host remove NAME` deletes the host from the config, the state
and the pinned keys.

## The default pool: one OS family

A plain `goway run` (no `--host`, no `--needs os=...`) only considers hosts
of the laptop's own OS family: `linux`, `darwin` or `windows`, with WSL
counting as Linux. A Windows host in the config therefore never receives a
Linux laptop's `cargo` runs by accident, and sharded runs never mix
operating systems. Ask for the other family with `--needs os=windows` (or
a `goway.toml` rule), or pin a host with `--host NAME`; both behave as
before. A host that has not reported its OS stays in the pool. `goway
status` has a `default pool` column: `yes`, or `no (windows)`.

## Windows hosts

Setting a Windows laptop up as a helper (`goway-setup install --host`, docs/install-windows.md)
uses its WSL default distro when that is Ubuntu or Debian, else the only such distro installed;
with several and no default it asks for `--distro NAME`. `goway doctor` on a Windows host also
warns when the helper's `.wslconfig` gives WSL so much memory that Windows keeps under 3 GB (or a
quarter of the RAM), and prints the `goway-setup tune --memory` command that fixes it. Its probe
of rustup and the Build Tools has a 10 second limit on the host, so a hung tool shows up as a
warning rather than a stuck doctor.

A `[[host]]` has an `os` (`linux`, the default, or `windows`) and a
`transport` (`ssh`, the default, or `interop`):

- **Windows over OpenSSH** (`os = "windows"`): goway runs `powershell`
  on the machine's OpenSSH server (port 22 by default). The script is
  passed as an encoded command, so every argument reaches PowerShell
  exactly as given (quotes, dollar signs and spaces included). The host
  key is pinned like any other host's, and no password is ever used.
- **The Windows side of this machine** (`os = "windows"`,
  `transport = "interop"`): from WSL, goway starts `powershell.exe`
  through WSL's interop. There is no ssh, no key, no address and no
  network listener, and nothing of the WSL environment is handed to
  Windows. Set it by hand in the config; it only works inside WSL.

**Registering a Windows machine over OpenSSH.** Put the host in the config
with `os = "windows"` (and its address, port 22), then run
`goway add NAME`. goway never logs in with a password to a Windows host:
it picks or creates its key as for any host, prints one command and waits
for you to press Enter after running it on that machine, in PowerShell as
Administrator (the installer it names is the one that set the machine up
with `goway-setup install --host --native`):

    goway-setup install --host --native --authorized-key "ssh-ed25519 AAAA... goway@laptop"

`goway-setup` authorizes the key (administrator accounts use
`administrators_authorized_keys`, others the profile's `authorized_keys`),
records it for its own uninstall, and goway then checks that key-only login
works, stores the identity and writes the undo record. `goway ssh setup NAME
--undo` removes goway's side; `goway-setup uninstall` removes the key there.

The Windows side of the protocol is `remote.ps1` (see docs/design.md
2a): the same verbs as on Linux, installed once per version under
`%LOCALAPPDATA%\goway` on the host. Not supported on Windows hosts:
symlinks in your tree (skipped with a note) and `.bat` or `.cmd` files
with unusual quoting as the command.

Both kinds share one transport abstraction (`crates/goway/src/transport.rs`),
so running on this machine in place can be one more kind.

## What git a machine needs

goway needs no git on a helper: it syncs with `git ls-files -co
--exclude-standard -z` and `git rev-parse --git-common-dir` on your laptop,
which git 2.25 and newer (tested) both handle. A
helper that has git also keeps gitignored paths in its slots (see
`keep_ignored` in the config); without it only the detected dependency and
build directories stay. The test suite itself runs on git 2.25 (Ubuntu 20.04):
it creates repositories with `git init` and `git symbolic-ref HEAD
refs/heads/main`, never `git init -b`, which needs 2.28.
