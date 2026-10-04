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

<details><summary>The helper laptop is slow while goway runs</summary>

goway runs jobs at low priority, so the person using the helper comes
first. To keep goway off a busy helper entirely, set `max_load` for it
in the config (docs/config.md).
</details>

<details><summary>Something else</summary>

Run the failing command again with `-vv` (for example
`goway -vv run -- true`) to see what goway does step by step. Then
check `goway doctor`.
</details>
