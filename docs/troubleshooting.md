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

<details><summary>Runs queue for a long time while a helper looks idle, or a run fails with "recorded peak"</summary>

The queue is first come, first served, but a waiter that cannot fit
anywhere right now (its repository's recorded memory peak is above the free
memory while other jobs run) is passed by up to three later runs that do
fit, then holds the line until it can go. A helper that runs no goway jobs
takes such a run alone, with a warning, instead of waiting. A repository
with no peak anywhere runs alone on a host until its first run records one.
If the peak is above every helper's total memory the run fails at once and
names the peak and the sizes; the peak may be stale (an OOM-killed run is
recorded a quarter higher): `goway gc --repo ID` forgets it, and
`--ignore-footprint` skips the check for one run. See "Waves of runs queue"
and "Memory: a repository's peak" in usage.md.
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

<details><summary>"clock is N s ahead of / behind this machine"</summary>

The helper's wall clock differs from your main laptop's by more than two
seconds. goway measures this on every probe: the helper reports its time,
and goway subtracts your own time at the midpoint of the round trip.
`goway status` and `goway doctor` warn about offsets over 2 s. A skewed
clock makes build tools see files as newer or older than they are
(endless rebuilds, or stale results). The usual cause is a WSL helper that
slept: its clock stops with the VM. Fix it from Windows with
`wsl --shutdown` (WSL restarts on next use), or on the helper with
`sudo hwclock -s`. On a Windows helper, resync the time (Settings, Time
& language, Sync now, or `w32tm /resync` as administrator). The offset is
only as exact as the round trip is symmetric; it is a warning, never a
reason for goway to refuse a run.
</details>

## The Windows drive is full: a WSL helper's disk only grows

<details><summary>doctor says "windows drive" or "wsl sparse disk", or WSL fails to start with HCS_E_CONNECTION_TIMEOUT</summary>

A WSL distro lives in one file, `ext4.vhdx`, on a Windows drive. Inside WSL
`df` shows the file's virtual size (often 1 TB), so a full Windows drive looks
like plenty of room, and a vhdx that is not sparse never shrinks: deleting files
in WSL (goway's gc and eviction included) frees blocks inside Linux but the file
on the drive keeps its size. A drive at 0 bytes free stops WSL from starting.

goway now reads the drive itself (docs/usage.md, "The disk budget"), keeps a
reserve on it and refuses a run below the reserve, naming the drive. To stop the
file only growing: on the helper's Windows side, in a normal terminal, run
`goway-setup tune --sparse` (journaled; `goway-setup uninstall --host` undoes
it). It needs WSL 2.0 or later (`wsl --update`); `wsl --version` shows it. Setting
the disk sparse may stop the distro, so run it while no goway job runs.
`goway-setup install --host` does the same, and enables `fstrim.timer`.

A vhdx that has **already grown** keeps its size until compacted once. As
administrator on the helper, with no job running:

    wsl --shutdown
    # either (Hyper-V tools installed):
    Optimize-VHD -Path C:\path\to\ext4.vhdx -Mode Full
    # or with diskpart:
    diskpart
      select vdisk file="C:\path\to\ext4.vhdx"
      attach vdisk readonly
      compact vdisk
      detach vdisk
      exit

(`goway doctor` shows both figures, and the vhdx size against what Linux uses; the
file sits under `%LocalAppData%\Packages\<distro package>\LocalState\` or
`%LocalAppData%\wsl\<guid>\`, and `wsl --manage DISTRO --move` can relocate it to
a roomier drive.) WSL starts again on next use. If the drive is already at 0 bytes, free a
little space on it first (the recycle bin, the temp directory), because compaction needs
scratch room.

Unverified on real hardware by the goway test suite: the exact WSL behaviour of
`--set-sparse` (which WSL versions stop the distro, whether `fstrim` is needed on top
of the root mount's `discard`), and doctor reads the sparse flag from the distro's
registry entry through interop only; with interop off it says it cannot tell and
judges by the vhdx's size.
</details>

## A network that hides the helpers

goway finds a helper by name (DNS, then mDNS) and confirms it by its
pinned ssh key. Some networks defeat that. The error says which case
it looks like; each has the same two exits: use a network where your
devices can see each other, or put a reachable address in the config
(`address = "192.0.2.10"` on the host's entry).

- **Client isolation** (guest Wi-Fi, some hotel and office networks):
  the name resolves but nothing answers at the address. The error says
  the network "isolates devices from each other". Another network
  fixes it; so does a VPN or tunnel that both machines join.
- **A VPN that routes only some addresses**: the same symptom. Leave
  split-tunnel mode, or add a route for the helper's subnet.
- **Blocked mDNS**: no address is found at all. The error says the
  network "may block name discovery". Give the helper's address in the
  config, or reserve a fixed address for it in the router.
- **Two machines answering for one name**: the error lists the
  addresses and says that several machines answered without the pinned
  key. goway never uses an address whose key it has not confirmed, so
  it will not run on the wrong machine. Rename one machine so the names
  differ, or give the right address in the config.

## Every build fails with "sccache: Failed to create temp dir"

A shared sccache server started by an older goway runs under a per-run
temporary directory that is removed when that run ends. Constant use keeps
its idle timeout from ever firing, so every later build fails. Current goway
checks the repository's server before each run (it must answer, and it must
have been started under goway's stable `sccache-tmp` directory), stops a
broken one, starts a fresh one and prints one note:
`restarting the shared sccache server (...)`. Nothing to do by hand; to
force it, run `sccache --stop-server` with `SCCACHE_SERVER_UDS` pointing at
the repository's socket under the helper's goway cache.

## A WSL helper stopped answering: check it by hand

`goway doctor HOST` does these steps itself when the helper's ssh port
is closed but the same address answers Windows OpenSSH (port 22): it
reports "unreachable" when nothing answers, "distro stopped" with the
command that starts it, a keepalive task that never repeats, or "WSL
service is not responding" (see step 7). It only reads; it never starts
anything, and every Windows command it runs is killed on the helper
after 10 seconds, so a deadlocked `wsl.exe` cannot pile up sshd sessions. To do the same by hand, from your main
laptop (HELIOS is the helper, USER its Windows user, 192.0.2.7 its
address; use your own):

1. Is the machine up at all?

       ping 192.0.2.7

   No answer: it is off, asleep, off the network, or the network blocks
   ping. This is not a WSL problem; wake it or check the network.

2. Do the two ports answer? WSL's sshd (2222 by default) and Windows
   OpenSSH (22):

       bash -c 'for p in 2222 22; do timeout 3 bash -c "</dev/tcp/192.0.2.7/$p" && echo "$p open" || echo "$p closed"; done'

   On Windows: `Test-NetConnection 192.0.2.7 -Port 22`. 22 open and 2222
   closed means Windows is up and WSL is not serving.

3. Ask Windows whether the distro runs (the output is UTF-16; Windows
   prints it fine in a terminal):

       ssh -p 22 USER@192.0.2.7 wsl -l -v

   `Stopped` next to your distro means a WSL shutdown (a Windows update,
   `wsl --shutdown`, a tune) ended it and nothing started it again.

4. Look at the keepalive task and its triggers:

       ssh -p 22 USER@192.0.2.7 schtasks /query /tn "WSL Keepalive" /v /fo list

   (`"WSL Keepalive (boot)"` for a boot keepalive; a named profile puts
   its name in front.) `Repeat: Every:` should show 5 minutes. `Disabled`
   there means an old boot-only task: a shutdown leaves the helper off
   until the next boot.

5. Start it now:

       ssh -p 22 USER@192.0.2.7 schtasks /run /tn "WSL Keepalive"

   then try `goway status` after a few seconds.

6. Re-run the installer when step 4 shows no repetition or no task:
   on the helper, as an administrator, run `goway-setup install --host`
   again (after `uninstall` if it says the install already exists). The
   new install replaces the old task and records the old one in the
   journal, so `uninstall` puts it back exactly.

7. `wsl -l -v` never returns (doctor says "WSL service is not
   responding"): the WSL service is deadlocked. Do not keep probing from
   the laptop: every hung ssh command leaves a stuck `sshd.exe` session on
   the helper, which eventually burns CPU and refuses new logins. On the
   helper, in an administrator PowerShell, in this order, stopping when
   `wsl -l -v` answers again:

       Stop-Process -Name wsl -Force
       Stop-Process -Name sshd -Force; Start-Service sshd
       wsl --shutdown
       Stop-Process -Name wslservice -Force

   The last line is the last resort; the service starts again on the next
   `wsl` call. Then run the keepalive (step 5).

If Windows asks for a password or an unknown host key at step 3, fix
that once by hand with ssh before relying on `goway doctor`.
