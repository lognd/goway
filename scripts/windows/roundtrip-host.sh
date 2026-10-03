#!/usr/bin/env bash
# Proof for the Windows installer's host component: `install --host` then `uninstall` in the
# goway-test profile on a real, LIVE goway host leaves the firewall rules, Hyper-V firewall,
# scheduled tasks, .wslconfig and the WSL sshd configuration exactly as they were.
#
#   scripts/windows/roundtrip-host.sh HOST_NAME [path/to/goway-setup.exe]
#
# frob command evidence needs a bare tool name on PATH: symlink this script as
# goway-roundtrip-host (ln -s .../scripts/windows/roundtrip-host.sh ~/.local/bin/goway-roundtrip-host).
# HOST_NAME (for example Helios) is resolved through Windows mDNS; a dotted IPv4 is used as given.
#
# Safety, because the machine is in use: the profile is goway-test, the port is 2299, all names
# are profile-prefixed, and sshd/WSL are never restarted (install and uninstall run with
# --no-activate). To prove sshd really answers on the new port without touching the listener on
# 2222, a SECOND, temporary sshd is started with the installed configuration on 2299
# (`sshd -p 2299`, its own pid file), logged into from this machine through the Windows address (so
# the Windows firewall rule and the Hyper-V firewall are on the path), and killed again. Port
# 2222 is logged into once a second for the whole run and must never fail.
#
# Two cases: (1) logon keepalive task with the default password hardening; (2) boot (S4U)
# keepalive task with --no-harden. Both pass --allow-from 100.64.0.0/10 because this script logs in
# over the host's Tailscale address, which the default local-subnet scope would (rightly) refuse.
# The rules must be limited to the Private and Domain profiles and to LocalSubnet plus that range.
# Needs key-based ssh to the host's Windows OpenSSH (port 22) and to its WSL sshd (port 2222).
# Exits nonzero unless every before/after snapshot pair is identical.
set -euo pipefail

host_name="${1:?usage: roundtrip-host.sh HOST_NAME [goway-setup.exe]}"
root="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd)"
here="$root/scripts/windows"
exe="${2:-${GOWAY_SETUP_EXE:-$root/target/x86_64-pc-windows-gnu/release/goway-setup.exe}}"
profile=goway-test
port=2299
live_port=2222
distro="${GOWAY_DISTRO:-Ubuntu}"
remote_dir=goway-roundtrip-host
work="$(mktemp -d)"
poller_pid=""

[[ -f "$exe" ]] || { echo "missing $exe; run scripts/windows/build.sh" >&2; exit 2; }

resolve() {
    if [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then echo "$1"; return; fi
    local fqdn="$1"; [[ "$fqdn" == *.* ]] || fqdn="$fqdn.local"
    powershell.exe -NoProfile -Command "Resolve-DnsName $fqdn -Type A | % IPAddress" 2>/dev/null \
        | tr -d '\r' | grep -v '^192\.168\.56\.' | head -n1
}
ip="$(resolve "$host_name")"
[[ -n "$ip" ]] || { echo "cannot resolve $host_name" >&2; exit 2; }
echo "== host $host_name -> $ip"

ssh_opts=(-n -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new)
rsh() { ssh "${ssh_opts[@]}" "$ip" "$@"; }              # Windows OpenSSH, lands in cmd.exe
wsh() { ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$work/known_hosts" -p "$live_port" "$ip" "$@"; }  # the WSL sshd (user shell; stdin is passed through)
winps() { # run a script file through powershell -EncodedCommand (no quoting layers)
    # Comment lines and indentation are dropped: cmd.exe caps a command line at 8191 characters.
    local enc
    enc="$( (echo '$ProgressPreference="SilentlyContinue"'; grep -v '^[[:space:]]*#' "$1" | sed 's/^[[:space:]]*//') | iconv -f utf-8 -t utf-16le | base64 -w0)"
    rsh powershell -NoProfile -NonInteractive -EncodedCommand "$enc" 2>/dev/null | tr -d '\r\0' | grep -a -v '^#< CLIXML' || true
}
winps_cmd() { local f="$work/cmd.ps1"; printf '%s\n' "$1" > "$f"; winps "$f"; }
rwsl() { rsh "wsl.exe -d $distro -u root --exec $*" 2>&1 | tr -d '\r\0'; }  # as root inside the distro
setup() { rsh "$remote_dir\\goway-setup.exe" "$@" 2>&1 | tr -d '\r'; }
# Probe a port with a full, successful public-key login. Never use ssh-keyscan or a bare TCP
# connect here: sshd (OpenSSH 9.8+) penalises sources that disconnect before authenticating
# and would start refusing this machine's real connections.
probe_login() { ssh -n -o BatchMode=yes -o ConnectTimeout=4 -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$work/known_hosts" -p "$1" "$ip" true >/dev/null 2>&1 && echo 1 || echo 0; }

snapshot() { # snapshot NAME
    winps "$here/snapshot-host.ps1" > "$work/$1.win"
    wsh bash -s < "$here/snapshot-wsl.sh" > "$work/$1.wsl" 2>&1
    [[ -s "$work/$1.win" && -s "$work/$1.wsl" ]] || { echo "empty snapshot $1" >&2; exit 2; }
}

status=0
compare() { # compare LABEL BEFORE AFTER
    local d=0
    diff -u "$work/$2.win" "$work/$3.win" || d=1
    diff -u "$work/$2.wsl" "$work/$3.wsl" || d=1
    if [[ $d -eq 0 ]]; then echo "PASS $1: Windows and WSL snapshots identical"; else echo "FAIL $1: snapshots differ"; status=1; fi
}

cleanup() { # best effort: leave the machine as found, whatever happened
    set +e
    [[ -n "$poller_pid" ]] && kill "$poller_pid" 2>/dev/null
    rwsl pkill -F /run/goway-test-sshd.pid >/dev/null 2>&1
    rwsl rm -f /run/goway-test-sshd.pid >/dev/null 2>&1
    setup uninstall --host --profile "$profile" --no-activate >/dev/null 2>&1
    rsh "rmdir /s /q $remote_dir" >/dev/null 2>&1
    rm -rf "$work"
}
trap cleanup EXIT

# Preconditions: the live listener answers, and our test port is free.
[[ "$(probe_login $live_port)" -eq 1 ]] || { echo "FAIL: no login on $live_port before the run" >&2; exit 2; }
[[ "$(probe_login $port)" -eq 0 ]] || { echo "FAIL: port $port already answers; refusing to continue" >&2; exit 2; }

rsh "powershell -NoProfile -NonInteractive -Command \"New-Item -ItemType Directory -Force $remote_dir | Out-Null\"" >/dev/null 2>&1
scp -q -o BatchMode=yes "$exe" "$ip:$remote_dir/"

# Guard: on a live host every WSL-side resource must already be in place (the dry run probes the
# machine read-only), so the run can never install, remove or purge a package or unit. Only the
# goway-test firewall rules, task and sshd drop-ins are created and removed.
plan="$(setup install --host --dry-run --port $port --profile "$profile" --color never)"
echo "$plan"
for need in "ensure WSL package openssh-server" "ensure enabled WSL systemd unit ssh.socket" "ensure enabled WSL systemd unit ssh.service" "set [boot] systemd=true in /etc/wsl.conf" "ensure directory /etc/ssh/sshd_config.d"; do
    grep -F "$need" <<<"$plan" | grep -q "in place" || { echo "REFUSING: '$need' is not already in place on this host" >&2; exit 2; }
done

# Watch the live port for the whole run.
( while :; do
    if [[ "$(probe_login $live_port)" -eq 1 ]]; then echo ok >> "$work/live.ok"; else echo fail >> "$work/live.fail"; fi
    sleep 1
  done ) &
poller_pid=$!

run_case() { # run_case LABEL EXPECT_TASK_SUFFIX EXPECT_PASSWORDAUTH install-args...
    local label="$1" suffix="$2" expect_password="$3"; shift 3
    echo "== case $label"
    snapshot "before-$label"
    setup install --host --port $port --profile "$profile" --no-activate --no-elevate "$@" -v
    setup status --profile "$profile" --color never
    local win; win="$(winps_cmd "
Get-NetFirewallRule -DisplayName 'goway-test WSL SSH $port' | % { 'rule: ' + \$_.DisplayName + ' ' + \$_.Direction + ' ' + \$_.Action + ' profiles=' + \$_.Profile }
(Get-NetFirewallRule -DisplayName 'goway-test WSL SSH $port' | Get-NetFirewallAddressFilter | % { 'rule remote: ' + (\$_.RemoteAddress -join ',') })
(Get-NetFirewallRule -DisplayName 'goway-test WSL SSH $port' | Get-NetFirewallPortFilter | % { 'rule port: ' + \$_.Protocol + '/' + \$_.LocalPort })
Get-NetFirewallHyperVRule -Name 'goway-test WSL SSH $port (Hyper-V)' | % { 'hyperv: ' + \$_.Direction + ' ' + \$_.Action + ' ' + \$_.Protocol + '/' + \$_.LocalPorts + ' vm=' + \$_.VMCreatorId + ' profiles=' + \$_.Profiles + ' remote=' + (\$_.RemoteAddresses -join ',') }
Get-ScheduledTask -TaskName 'goway-test WSL Keepalive$suffix' | % { 'task: ' + \$_.TaskName + ' ' + \$_.State + ' ' + \$_.Principal.LogonType + ' ' + \$_.Triggers[0].CimClass.CimClassName }
")"
    echo "$win"
    for want in "rule: goway-test WSL SSH $port Inbound Allow profiles=" "rule port: TCP/$port" "hyperv: Inbound Allow TCP/$port vm={40E0AC32-46A5-438A-A0B2-2B479E8F2E90}" "task: goway-test WSL Keepalive$suffix"; do
        grep -qF "$want" <<<"$win" || { echo "FAIL: after install, missing: $want"; status=1; }
    done
    # Scope: Private and Domain only (never Public or Any), local subnet plus the allowed range.
    local scoped
    for scoped in "rule: goway-test" "hyperv: Inbound"; do
        local line; line="$(grep -F "$scoped" <<<"$win" | head -n1)"
        if [[ "$line" == *Private* && "$line" == *Domain* && "$line" != *Public* && "$line" != *Any* ]]; then echo "ok: '$scoped' limited to Private,Domain"; else echo "FAIL: '$scoped' profiles are not Private,Domain: $line"; status=1; fi
    done
    for scoped in "rule remote: " "hyperv: Inbound"; do
        local line; line="$(grep -F "$scoped" <<<"$win" | head -n1)"
        if [[ "$line" == *LocalSubnet* && "$line" =~ 100\.64\.0\.0/(10|255\.192\.0\.0) && "$line" != *Any* ]]; then echo "ok: '$scoped' limited to LocalSubnet + 100.64.0.0/10"; else echo "FAIL: '$scoped' remote scope wrong: $line"; status=1; fi
    done
    echo "-- sshd configuration check as root (sshd -t) and effective ports"
    rwsl /usr/sbin/sshd -t && echo "sshd -t ok"
    local effective; effective="$(rwsl /usr/sbin/sshd -T | grep -E '^(port|passwordauthentication) ' | sort | tr '\n' ' ')"
    echo "$effective"
    [[ "$effective" == *"passwordauthentication $expect_password"* ]] || { echo "FAIL: expected passwordauthentication $expect_password"; status=1; }
    echo "-- temporary sshd on $port with the installed configuration"
    rwsl /usr/sbin/sshd -p $port -o PidFile=/run/goway-test-sshd.pid
    sleep 1
    if [[ "$(probe_login $port)" -eq 1 ]]; then echo "ok: logged in over $ip:$port"; else echo "FAIL: no login on $ip:$port"; status=1; fi
    rwsl pkill -F /run/goway-test-sshd.pid || true
    rwsl rm -f /run/goway-test-sshd.pid
    sleep 1
    [[ "$(probe_login $port)" -eq 0 ]] && echo "ok: temporary sshd on $port is gone" || { echo "FAIL: $port still answers"; status=1; }
    setup uninstall --host --profile "$profile" --no-activate --no-elevate -v
    snapshot "after-$label"
    compare "$label" "before-$label" "after-$label"
}

run_case logon "" no --allow-from 100.64.0.0/10
run_case boot " (boot)" yes --no-harden --keepalive boot --allow-from 100.64.0.0/10
compare "restored-original" before-logon after-boot

kill "$poller_pid" 2>/dev/null || true; poller_pid=""
ok=$(grep -c . "$work/live.ok" 2>/dev/null || true); fail=$(grep -c . "$work/live.fail" 2>/dev/null || true)
echo "== port $live_port probes during the run: ${ok:-0} answered, ${fail:-0} failed"
if [[ "${fail:-0}" -ne 0 || "${ok:-0}" -lt 5 ]]; then echo "FAIL: port $live_port did not answer throughout"; status=1; fi
[[ "$(probe_login $live_port)" -eq 1 ]] || { echo "FAIL: $live_port does not answer at the end"; status=1; }

[[ $status -eq 0 ]] && echo "HOST ROUNDTRIP OK on $host_name ($ip)" || echo "HOST ROUNDTRIP FAILED on $host_name ($ip)"
exit $status
