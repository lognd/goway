#!/usr/bin/env bash
# Prints a deterministic snapshot of the WSL-side state the host component may touch. Run inside
# the distro (the round-trip script feeds it to `ssh -p <port> host bash -s`); it only reads.
# Directory mtimes are left out on purpose: adding and removing a file changes them.
set -u
echo "== /etc/wsl.conf"
if [ -e /etc/wsl.conf ]; then sha256sum /etc/wsl.conf; cat /etc/wsl.conf; else echo absent; fi
echo "== /etc/ssh/sshd_config.d"
if [ -d /etc/ssh/sshd_config.d ]; then
    echo "dir present"
    find /etc/ssh/sshd_config.d -mindepth 1 | LC_ALL=C sort | while IFS= read -r f; do
        if [ -f "$f" ]; then sha256sum "$f"; else echo "$f"; fi
    done
else echo absent; fi
echo "== /etc/ssh entries"
ls -A /etc/ssh 2>/dev/null | LC_ALL=C sort
echo "== systemctl cat ssh.socket"
systemctl cat ssh.socket 2>&1
echo "== systemctl cat ssh.service"
systemctl cat ssh.service 2>&1
echo "== openssh-server package"
dpkg-query -W -f='${db:Status-Abbrev} ${Version}\n' openssh-server 2>&1
echo "== enabled units"
systemctl is-enabled ssh.socket ssh.service 2>&1
echo "== listening tcp ports"
ss -Hltn | awk '{print $4}' | LC_ALL=C sort
