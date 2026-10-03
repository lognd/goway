# goway

goway lets you type a build or test command on your main laptop and
have one of your other laptops do the work. It is for software projects
kept in git, for example Rust projects. goway:
- copies your project to the least busy helper laptop
- runs the command there
- shows you the output as if the command ran on your main laptop

You need:
- a **main laptop** running Linux, or Windows with WSL (Linux inside
  Windows)
- one or more **helper laptops** running Windows 11 (22H2 or newer)
  with WSL
- all of them on the **same network**

Words you have not seen before are explained in
[docs/glossary.md](docs/glossary.md).

## Quick start

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

- **Linux (WSL) networking set to "mirrored"** in
  `%USERPROFILE%\.wslconfig`, so your main laptop can reach the Linux
  inside this laptop.
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

Type the line the helper's installer printed in step 1.4, for example:

    goway add helios --fingerprint SHA256:Ei5... --user user --rsudo

goway shows what it is about to do, asks for the **helper's Linux
password** once (not its Windows password), and finishes with
`goway: done: helios is ready`.

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

## Day to day

- `goway run -- COMMAND` runs a command on the least busy helper;
  `--host NAME` picks one; `--shard 2` splits a test run across two
  helpers.
- `goway status` shows how busy each helper is.
- `goway doctor` checks everything and says how to fix what is wrong.
- Press Ctrl-C to stop a run; goway stops it on the helper too.

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

## Something went wrong?

See [docs/troubleshooting.md](docs/troubleshooting.md). Every goway
error ends with a `next:` line saying what to try.

## Reference

- [docs/usage.md](docs/usage.md): every command, exit codes, what is
  sent
- [docs/hosts.md](docs/hosts.md): how goway finds helpers without
  fixed addresses
- [docs/ssh-setup.md](docs/ssh-setup.md): the key setup in detail
- [docs/config.md](docs/config.md): settings
- [docs/install-linux.md](docs/install-linux.md) and
  [docs/install-windows.md](docs/install-windows.md): what the
  installers change
- [docs/glossary.md](docs/glossary.md): words explained

For developers:
- [docs/positioning.md](docs/positioning.md): goway versus other
  build systems
- [docs/design.md](docs/design.md): the design
- [docs/prior-art.md](docs/prior-art.md): related tools
