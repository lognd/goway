# Guided ssh setup

    goway ssh setup HOST            # make key login work, reversibly
    goway ssh setup HOST --undo     # remove exactly what setup added
    goway ssh setup NEWHOST --address 192.168.1.20 --port 2222 --user me

goway itself only ever uses key authentication. `ssh setup` handles the
first-time step that usually means ssh-keygen, ssh-copy-id and a few
chmods, and it can be undone.

## What it does

1. **Find the host** with the normal resolution (docs/hosts.md). A
   configured host must present its pinned key. For a new host, goway
   shows the fingerprint of the key it presents. You confirm it at the
   terminal or with `--fingerprint SHA256:...`, which you get on the
   helper with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`. This
   happens **before any password is asked for**, so an impostor
   answering for the name never sees your password. If key login
   already works, setup says so and only pins the confirmed key.
2. **Pick a public key**, without ever reading private keys
   (`--key FILE.pub` chooses one; only `.pub` files are accepted):
   - goway's own key from an earlier setup, if there is one, else
   - the ssh agent's first key (`ssh-add -L`), else
   - the first default identity (from `ssh -G`) that has a `.pub`
     next to it, else
   - a new ed25519 key in **goway's config dir**
     (`~/.config/goway/id_ed25519`, `%LOCALAPPDATA%\goway\id_ed25519`).
     goway never writes to your `~/.ssh` on the client. On Windows the
     key's ACL is reduced to your user
     (`icacls KEY /inheritance:r /grant:r USER:F`), because Windows
     OpenSSH refuses private keys that others can read. goway then
     offers this key for that host only (`identity` in the host config).
3. **Log in once with a password** (ssh prompts; on Unix clients one
   multiplexed connection carries every step, so you type it once).
   goway checks that the machine runs Linux and that its hostname
   matches, unless the host is already pinned or you gave `--address`
   as an IP. Then it ensures on the host:
   - `~/.ssh` exists, with mode 700
   - the key line is in `~/.ssh/authorized_keys`, prefixed with
     `no-agent-forwarding,no-port-forwarding,no-X11-forwarding` and tagged
     `goway:<id>` (skipped if the key is already there)
   - `authorized_keys` has mode 600
4. **Verify** that a key-only login now works, add the host to the
   pool if it was new (pinning exactly the key you confirmed in step 1),
   and save the record.

## Undo

Every change is a journal entry with its prior state (the same journal
that drives the Windows installer, crates/goway-journal). The record
lives at `<config dir>/ssh-setup-<host>.json`. `--undo` reverts:
- on the host: the tagged line, the previous modes, and `~/.ssh` if
  setup created it and it is empty again
- goway's own key, if setup created it
- the `identity` entry, or the whole host if setup added it

Anything changed since is left alone. Tests compare full snapshots
(paths, modes, bytes) of the "remote" home and goway's config dir
before setup and after undo (crates/goway/tests/ssh_setup.rs). They
cover a fresh home and a pre-existing `~/.ssh` with loose modes.

## Host side: sshd at startup

On a Windows machine with WSL, `goway-setup install --host`
(docs/install-windows.md) makes sshd start with the machine and keeps it
reachable:
- systemd in WSL with `ssh.socket` enabled on the port
- a keepalive task
- the firewall rules
- mirrored networking

`goway doctor HOST` warns while sshd still accepts passwords. Its fix
(with `--sudo`) adds a drop-in that turns password login off. Do this
only after `ssh setup` has made key login work.

## Windows OpenSSH server (not used by goway)

goway reaches hosts through the WSL sshd and never logs into the Windows
OpenSSH server, so setup does not touch it. To use keys there yourself
as an administrator, Microsoft documents the steps: put the key in
`C:\ProgramData\ssh\administrators_authorized_keys`, then run
`icacls.exe "C:\ProgramData\ssh\administrators_authorized_keys" /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"`.
