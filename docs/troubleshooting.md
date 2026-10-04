# Troubleshooting

Every goway error ends with a `next:` line saying what to try. This page
explains the common ones in more detail. `goway doctor` checks
everything at once and prints the exact fix for each problem.

<details><summary>"cannot reach host NAME ... unreachable"</summary>

Your main laptop could not connect to the helper at all. Check, in this
order:
1. Is the helper switched on and awake? A closed lid often means asleep.
2. Is it on the same network as your main laptop?
3. Is WSL running on it? Signing in to Windows starts it; the
   installer's keepalive task keeps it running.
4. Is its network marked Private? See "Public network" in the README.

Then run `goway status`. It shows each helper that answers.
</details>

<details><summary>"key authentication refused"</summary>

The helper answered but did not accept your key. Run
`goway add NAME` again. It sets up the key and asks for the helper's
Linux password once. If the user name on the helper differs from yours,
add `--user THAT-NAME`.
</details>

<details><summary>"a different machine (host key mismatch)"</summary>

Something at that address presents a different identity than the helper
goway knows. goway refuses it on purpose: it may be another computer
that now has that address, which is harmless and goway moves on. It may
also be someone pretending to be your helper.

If you reinstalled Linux or WSL on the helper, its identity really
changed. Then run `goway host remove NAME`, and `goway add NAME` with
the new fingerprint the helper's installer prints
(`goway-setup.exe status --host` shows it again).
</details>

<details><summary>"the host key ... needs confirmation"</summary>

goway only trusts a new helper after you confirm its fingerprint. Run
the command in a terminal and answer the question, or pass
`--fingerprint SHA256:...` as printed by the helper's installer. On the
helper you can also see it in its WSL terminal with
`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`.
</details>

<details><summary>"the host no longer answers: it is asleep, off, or off the network"</summary>

The ssh connection dropped while your job ran and the helper did not
answer right after. goway reports that with its own exit code 125, so it
is never mistaken for a failure of your command (a command that itself
exits 255 on a helper that still answers keeps that code).

goway holds a sleep inhibitor on the helper for exactly as long as the
job runs (`systemd-inhibit` on Linux, `caffeinate` on macOS), but that
cannot stop a closed lid, an empty battery or Wi-Fi going away. Wake the
helper (open the lid, plug it in) and run again; `goway status` shows
when it answers. The job was stopped by the helper's watchdog, so nothing
keeps running there.
</details>

<details><summary>The helper's shell prints text, or is not bash</summary>

sshd runs every goway call through the account's login shell, which
reads its startup files first. A `.bashrc` (or `.profile`, `config.fish`,
`.cshrc`) that prints a greeting, a `fortune` or an `echo` for
non-interactive sessions would otherwise land in goway's protocol.
goway is built for that:

- The command line the login shell sees is plain text (`bash -c` with a
  base64 payload), so fish, csh, tcsh and others run bash with the script
  and your command unchanged. Quotes, backslashes and `!` in your command
  are never parsed by the login shell.
- goway's output starts with a marker line; everything a startup file
  printed before it is dropped, for `goway run` and for every internal
  call. (Text printed on stderr is shown as is.)

Dropping the noise is a workaround, not a fix: it costs nothing, but the
text is still produced on every call. Guard it so it only runs for a
person, at the top of the helper's `~/.bashrc`:

    case $- in *i*) ;; *) return ;; esac

or print only on a terminal: `[ -t 1 ] && echo ...`. If a startup file
makes goway's output differ from what you expect, `goway -v` logs how many
bytes of startup text it ignored for each call. `goway doctor` warns about a helper whose startup files print text,
showing the first line, and names the guard to add.
</details>

<details><summary>goway add says Permission denied</summary>

On a terminal `goway add` first asks "Do you know the password of USER
on NAME? [Y/n]". Answer `n` (or pass `--no-password`) when the account
has no password or you are not sure: goway goes straight to the by-hand
path below. Otherwise it logs in once with that password to install its
key. ssh asks for the password itself, under the name you typed (goway
never sees, stores or sends a password other than through ssh's own
prompt), and asks exactly once (`NumberOfPasswordPrompts=1`, password
methods only, no agent keys), so a wrong or empty password is one failed
attempt, never several toward a fail2ban limit. goway then says "That
password did not work on NAME", names the most likely cause and
continues with "Let's do it the other way". `--yes` skips the question
and goes on to the password; without a terminal there is no question.
"Permission denied (publickey,password)" means the helper refused the
login. Check the causes on the helper, in this order (`USER` is the
Linux account you pass with `--user`):

1. **No password is set for the account.** Common with automatic login
   on a native Linux install. `passwd -S USER` shows `NP`. sshd never
   accepts an empty password, however it is typed. Either set one
   (`sudo passwd USER`) or add goway's key by hand (below).
2. **A mistyped password.** Try again, or use the by-hand path.
3. **Password login is switched off** (or a second factor is
   required): `sudo sshd -T | grep -i passwordauthentication` says `no`.
   Use the by-hand path.
4. **The wrong user.** Pass the helper's own Linux account:
   `goway add NAME --user USER`.
5. **A lockout or a ban after failed attempts:** `faillock --user USER`
   (clear it with `faillock --user USER --reset`) and
   `sudo fail2ban-client status sshd` (unban with
   `sudo fail2ban-client set sshd unbanip ADDRESS`).
6. **AllowUsers or AllowGroups** leaves the account out:
   `sudo sshd -T | grep -iE 'allowusers|allowgroups'`.

**Adding the key by hand.** goway does this itself when the password
login is refused, or from the start with `goway add NAME --no-password`.
It prints three commands with goway's public key already filled in
(only the `.pub` text is ever printed, never a private key). Run them in
a terminal on the helper:

```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo 'no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 AAAA... goway@laptop goway:1' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

The line carries the same restrictions as the one the password path
installs. In a terminal goway then says "Then press Enter here (Ctrl-C to
stop)", checks key login with the pinned host key and goes
on with the normal setup. Without a terminal it stops with exit code 125
after printing the commands; run them on the helper and repeat the same
`goway add` command. `goway ssh setup NAME --undo` cannot remove a line
you added by hand: delete the line tagged `goway:` from
`~/.ssh/authorized_keys` yourself.
</details>

<details><summary>"... is not in one" (not a git project)</summary>

goway runs a command from inside a project folder that git tracks. Use
`cd` to go into your project first. To start a new project in the
current folder, run `git init`.
</details>

<details><summary>"no usable host"</summary>

Every helper is unreachable or too busy. `goway status` shows which.
Helpers with `max_jobs` or `max_load` in the config are skipped while
they are at their limit (docs/config.md).
</details>

<details><summary>"needs root" / "need administrator rights"</summary>

Some fixes need administrator rights on the helper, such as installing
the C compiler that Rust uses. Run `goway doctor NAME --fix --rsudo`,
or `goway add NAME --rsudo`. goway lists the changes with their reasons
and asks once. sudo on the helper then asks for its Linux password.
</details>

<details><summary>doctor says "wsl size": WSL has much less than the laptop</summary>

WSL starts with half the laptop's RAM and often less swap than a big
build wants. `goway doctor NAME` compares what WSL got with what the
laptop has and prints the command, for example
`goway-setup.exe tune --memory 12GB --swap 6GB --processors 14`. The
suggestion leaves Windows at least 4 GiB or 25% of the RAM. Run it in a
normal (non-administrator) terminal on the helper's Windows side. It
edits `.wslconfig` through a journal, refuses while goway jobs run (unless
`--yes`), and asks before `wsl --shutdown`, which stops everything in
WSL. `goway-setup uninstall --host` puts the old values back. See
docs/install-windows.md ("Giving WSL more of the machine").
</details>

<details><summary>doctor says the GPU is invisible to WSL, or CUDA is missing</summary>

If Windows lists an NVIDIA or AMD GPU that WSL does not see, install the
current Windows driver with WSL support, never a Linux GPU driver inside
WSL, then run `wsl --shutdown` from a normal terminal and check
`nvidia-smi` in WSL. If the GPU is visible but `nvcc` is missing,
`goway doctor NAME --fix --rsudo` installs NVIDIA's `cuda-toolkit` for
WSL-Ubuntu (x86_64, apt; no driver) and records it, so `goway uninstall`
removes it again.
</details>

<details><summary>"kept N secret-looking file(s) on this machine"</summary>

goway did not send files that look like passwords or keys (`.env`,
`*.pem`, `id_rsa`, ...). If your tests need one of them, for example a
test certificate, list it in `secret_allow` in `~/.config/goway/config.toml`:

    [defaults]
    secret_allow = ["tests/fixtures/*.pem"]
</details>

<details><summary>goway says it is holding back, or the helper looks banned</summary>

Many helpers run fail2ban or sshguard, which ban a machine after about
5 failed logins in 10 minutes (the fail2ban default). goway keeps its own
count of the failed logins it causes, per helper, in `auth-failures.json`
in its state directory, and stays well under that:

- Automatic probing (finding a helper, `goway status`, `goway doctor`)
  causes at most 2 failed logins per helper per 10 minutes, then stops
  trying and says "holding back" with the minutes left. Candidates are
  tried with the pinned key only, never a password.
- Steps you drive come on top: the one password attempt of `goway add`
  and the key re-check each time you press Enter after pasting the key
  lines. In all, goway causes at most 4 failed logins per helper per 10
  minutes (under fail2ban's 5), so the paste flow is never blocked by
  background probing; if the total is reached goway stops and tells you
  to wait and rerun the same `goway add` command.
- Only failed authentications count; a host that is simply off does not.

If a connection is refused right after failed logins, goway reports
"probably banned". On the helper, `sudo fail2ban-client status sshd`
lists the banned addresses and
`sudo fail2ban-client set sshd unbanip ADDRESS` lifts one.
</details>

<details><summary>The helper laptop is slow while goway runs</summary>

goway runs jobs at low priority, so the person using the helper comes
first. To keep goway off a busy helper entirely, set `max_load` for it
in the config (docs/config.md).
</details>

<details><summary>doctor warns "logind kills ... processes at logout" or about the remote root's file system</summary>

`goway doctor` reads two things from each Linux helper. First, whether
systemd-logind is set to `KillUserProcesses=yes` while lingering is off for
the helper's user: then everything that user left running dies when the
last login session ends, so a run whose ssh connection drops, or goway's own
background work, is killed with it. The fix is `loginctl enable-linger USER`;
it needs root, so `goway doctor --fix --rsudo` offers it with the usual
confirmation (undo: `loginctl disable-linger USER`). goway reads the setting
from `logind.conf` and its drop-in directories, so an override somewhere else
can differ.

Second, the file system under goway's remote root (`defaults.remote_root`,
`.cache/goway` in the helper's home by default). An encrypted home (ecryptfs,
encfs, gocryptfs) is only mounted while its owner is logged in, and a network
file system (NFS, CIFS, sshfs) goes away with the network, so the work trees
and caches would vanish between sessions. doctor reports the type; point
`defaults.remote_root` at a directory on an ordinary local disk, such as
`/srv/goway` (create it and give your user ownership first). goway cannot
choose that for you.
</details>

<details><summary>A tool works in my login shell on the helper but goway cannot find it</summary>

ssh commands do not read `.profile` or `.bashrc`, so a tool added to
PATH there is invisible. goway never reads those files (they may print
text or run anything). Instead, runs and `goway doctor` add the usual
per-user directories to PATH: `~/.local/bin` (mold, uv tools), then
`~/.cargo/bin`, then, where they exist, the uv, Volta, nvm and fnm
locations, `/usr/local/go/bin` and `~/go/bin`. Install the tool in one
of those, for example with `cargo install` or `uv tool install`.
</details>

<details><summary>Something else</summary>

Run the failing command again with `-vv` (for example
`goway -vv run -- true`) to see what goway does step by step. Then
check `goway doctor`.
</details>

<details><summary>"Session open refused by peer" / "ControlSocket already exists"</summary>

You may see this from older goway versions when many runs started at
once. A helper's sshd allows 10 sessions per connection, and goway used
one shared connection per helper. goway now keeps up to four shared
connections per helper (a few runs each). When all are busy, a run
connects on its own, quietly. A leftover socket from a connection that
ended is removed and replaced automatically. Nothing to do.
</details>

<details><summary>"selinux" or "apparmor" warning in `goway doctor`</summary>

A security module on the helper denied one of the programs goway runs
jobs with (`setsid`, `flock`, the shell or goway itself). The run then
fails with a plain "Permission denied". doctor reads the latest matching
denial from the audit log (`ausearch`) or the kernel log (`dmesg`), names
the denied program and domain or profile, and prints the change to make:
`restorecon` or a reviewed `audit2allow` policy module for SELinux, a
local override plus `apparmor_parser -r` for AppArmor. Both logs need
root on many systems, so no warning does not prove there is no denial;
run the printed `ausearch` command with sudo to be sure. goway never
changes a security policy itself.
</details>
