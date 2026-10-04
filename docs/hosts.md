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
2. **configured**: the host's `address`, through the system resolver,
   then Windows. If neither resolves it, ssh is given the name as is.
3. **dns**: the host name through the system resolver.
4. **mdns**: `<name>.local` through the system resolver (works where
   the OS resolves mDNS, such as Windows, macOS, or Linux with
   nss-mdns).
5. **windows-mdns**: `<name>.local` asked of Windows through WSL interop
   (`powershell.exe Resolve-DnsName`). A WSL client in NAT mode cannot
   do mDNS itself. This takes about 1 to 4 seconds, which is why it is
   last and why the result is cached. `GOWAY_WINDOWS_LOOKUP=0` turns
   it off.

Loopback, link-local and duplicate addresses are skipped. Windows also
answers with unrelated adapters (for example VirtualBox
`192.168.56.1`). Those candidates fail the key check and are skipped.
Resolution stops early if the key is not pinned yet or authentication is
refused, because another address of the same machine would not help.
The error lists every candidate tried and why it failed, including the
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

`goway host list` shows the pool with cached addresses.
`goway host remove NAME` deletes the host from the config, the state
and the pinned keys.

## Windows hosts

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
