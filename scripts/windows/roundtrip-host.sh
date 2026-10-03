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
# The host install also registers its own machine-wide Add/Remove Programs entry (HKLM Uninstall,
# goway-test-host). Case 1 checks its values and then uninstalls by running the entry's own
# UninstallString (the protected copy in %ProgramData%, never the downloaded file); the snapshots
# include the HKLM entries, so both cases prove the entry is gone again after uninstall.
# Case 3 (nat): --network nat is forced on this machine on its own port 2399. A mirrored machine has
# no WSL NAT adapter, and the relay refuses to forward anywhere but a private address inside that
# adapter's subnet, so there the case proves the refusal and a clean rollback (identical snapshots).
# On a machine in NAT mode it proves the NAT relay: install adds the relay (netsh portproxy 0.0.0.0:2399), the refresh script in the
# administrator-only directory and the refresh task (Highest privileges); .wslconfig is not
# touched. The relay's address is then set to a wrong one by hand and the task is started: it must
# put the WSL address back. A mirrored machine's WSL address is the Windows address itself, so to
# prove traffic really crosses the relay without looping it onto itself (a mirrored host cannot
# reach its own address from itself), the relay is pointed at the live sshd on 127.0.0.1:2222 for
# one login through port 2399 and put back. Uninstall must leave
# the portproxy table, tasks, firewall rules and files exactly as before.
# GOWAY_CASES="nat" (a space-separated subset of "logon boot nat") limits the run to those cases.
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
cases="${GOWAY_CASES:-logon boot nat}"
nat_port=2399

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
    { winps "$here/snapshot-host.ps1"; winps "$here/snapshot-arp.ps1"; } > "$work/$1.win"
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
    setup install --host --port $port --profile "$profile" --no-activate --no-elevate --yes "$@" -v
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
    echo "-- Add/Remove Programs entry (HKLM)"
    local arp; arp="$(winps_cmd "
\$k = Get-ItemProperty -LiteralPath 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\\$profile-host'
'arp name: ' + \$k.DisplayName
'arp uninstall: ' + \$k.UninstallString
'arp exe exists: ' + (Test-Path -LiteralPath \$k.DisplayIcon)
")"
    echo "$arp"
    for want in "arp name: goway helper (host, profile $profile)" "uninstall --host --profile $profile" "arp exe exists: True" 'ProgramData\goway\goway-test\bin\goway-setup.exe'; do
        grep -qF "$want" <<<"$arp" || { echo "FAIL: after install, ARP entry missing: $want"; status=1; }
    done
    arp_cmd="$(sed -n 's/^arp uninstall: //p' <<<"$arp" | head -n1)"
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
    if [[ "$label" == logon && -n "${arp_cmd:-}" ]]; then
        echo "-- uninstall through the entry's own UninstallString (no downloaded file)"
        rsh "cmd /c \"$arp_cmd --no-activate --no-elevate\"" 2>&1 | tr -d '\r'
        local gone; gone="$(winps_cmd "
for (\$i = 0; \$i -lt 60; \$i++) { if (-not (Test-Path -LiteralPath \$env:ProgramData\goway\goway-test)) { break }; Start-Sleep -Seconds 1 }
'admin dir present: ' + (Test-Path -LiteralPath \$env:ProgramData\goway\goway-test)
'arp present: ' + (Test-Path -LiteralPath 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\\$profile-host')
")"
        echo "$gone"
        grep -qF "admin dir present: False" <<<"$gone" && grep -qF "arp present: False" <<<"$gone" || { echo "FAIL: the ARP uninstall left state behind"; status=1; }
    else
        setup uninstall --host --profile "$profile" --no-activate --no-elevate -v
    fi
    snapshot "after-$label"
    compare "$label" "before-$label" "after-$label"
}

run_nat_case() {
    local label=nat
    echo "== case $label (relay on port $nat_port; --network nat forced)"
    echo "-- plan (dry run)"
    setup install --host --dry-run --network nat --port $nat_port --profile "$profile" --color never
    snapshot "before-$label"
    # The relay only forwards to a private address inside the subnet of the WSL NAT adapter
    # (vEthernet (WSL)). A mirrored machine has none, so there the install must refuse cleanly.
    local adapter
    adapter="$(winps_cmd "[System.Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces() | ? { \$_.Name -like 'vEthernet (WSL*' } | % { \$_.Name }")"
    if [[ -z "$adapter" ]]; then
        echo "-- no WSL NAT adapter on this machine: the nat install must refuse and roll everything back"
        local refused
        if refused="$(setup install --host --network nat --port $nat_port --profile "$profile" --no-activate --no-elevate --yes --allow-from 100.64.0.0/10 2>&1)"; then
            echo "FAIL: the nat install succeeded without a WSL adapter"; status=1
        else
            echo "$refused"
            grep -q "WSL virtual network adapter" <<<"$refused" || { echo "FAIL: the refusal does not name the missing adapter"; status=1; }
        fi
        setup uninstall --host --profile "$profile" --no-activate --no-elevate >/dev/null 2>&1 || true
        snapshot "after-$label"
        compare "$label" "before-$label" "after-$label"
        return
    fi
    local ip_before; ip_before="$(rwsl hostname -I | awk '{print $1}')"
    echo "WSL address as the distro reports it: $ip_before"
    setup install --host --network nat --port $nat_port --profile "$profile" --no-activate --no-elevate --yes --allow-from 100.64.0.0/10 -v
    setup status --profile "$profile" --color never
    local win; win="$(winps_cmd "
\$r = & \$env:SystemRoot\System32\netsh.exe interface portproxy show v4tov4
\$r | ForEach-Object { 'proxy: ' + \$_.Trim() }
\$t = Get-ScheduledTask -TaskName 'goway-test WSL Relay'
'relay task: ' + \$t.TaskName + ' ' + \$t.Principal.LogonType + ' run=' + \$t.Principal.RunLevel + ' user=' + (\$t.Principal.UserId -ne \$null)
'relay trigger: ' + \$t.Triggers[0].CimClass.CimClassName + ' every=' + \$t.Triggers[0].Repetition.Interval
'relay action: ' + \$t.Actions[0].Execute + ' ' + \$t.Actions[0].Arguments
'script present: ' + (Test-Path -LiteralPath \$env:ProgramData\goway\goway-test\relay-refresh.ps1)
(Get-NetFirewallRule -DisplayName 'goway-test WSL SSH $nat_port' | Get-NetFirewallAddressFilter | % { 'rule remote: ' + (\$_.RemoteAddress -join ',') })
")"
    echo "$win"
    win="$(tr -s ' ' <<<"$win")"
    for want in "proxy: 0.0.0.0 $nat_port $ip_before $nat_port" "relay task: goway-test WSL Relay " "run=Highest" "script present: True" "rule remote: LocalSubnet"; do
        grep -qF "$want" <<<"$win" || { echo "FAIL: after install, missing: $want"; status=1; }
    done
    echo "-- the refresh task puts a wrong address back (simulated WSL restart)"
    winps_cmd "& \$env:SystemRoot\System32\netsh.exe interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=$nat_port connectaddress=192.0.2.99 connectport=$nat_port" >/dev/null
    winps_cmd "Start-ScheduledTask -TaskName 'goway-test WSL Relay'" >/dev/null
    local fixed=0 i table
    for i in $(seq 1 30); do
        table="$(winps_cmd "& \$env:SystemRoot\System32\netsh.exe interface portproxy show v4tov4")"
        if grep -qE "^0\.0\.0\.0 +$nat_port +$ip_before +$nat_port" <<<"$table"; then fixed=1; break; fi
        sleep 1
    done
    [[ $fixed -eq 1 ]] && echo "ok: the task re-pointed the relay at $ip_before after ${i}s" || { echo "FAIL: relay not re-pointed: $table"; status=1; }
    echo "-- a login through the relay (pointed at the live sshd on the host's loopback for this one check)"
    winps_cmd "& \$env:SystemRoot\System32\netsh.exe interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=$nat_port connectaddress=127.0.0.1 connectport=$live_port" >/dev/null
    sleep 1
    # The firewall rule is limited to Private and Domain networks, so a login from here over a
    # network Windows calls Public is (rightly) blocked: shown for information, never asserted.
    echo "info: a direct login to $ip:$nat_port from this machine: $([[ "$(probe_login $nat_port)" -eq 1 ]] && echo allowed || echo blocked by the firewall scope)"
    # To prove the relay itself forwards, enter it from the host's own loopback (not subject to the
    # firewall) through Windows OpenSSH's TCP forwarding, then log in to the WSL sshd end to end.
    if ssh -n -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$work/known_hosts" -o ProxyCommand="ssh -W 127.0.0.1:$nat_port -o BatchMode=yes $ip" -p $nat_port "$ip" true >/dev/null 2>&1; then echo "ok: logged in to the WSL sshd through the relay (loopback of $ip, port $nat_port)"; else echo "FAIL: no login through the relay"; status=1; fi
    winps_cmd "& \$env:SystemRoot\System32\netsh.exe interface portproxy set v4tov4 listenaddress=0.0.0.0 listenport=$nat_port connectaddress=$ip_before connectport=$nat_port" >/dev/null
    setup uninstall --host --profile "$profile" --no-activate --no-elevate -v
    snapshot "after-$label"
    compare "$label" "before-$label" "after-$label"
}

for c in $cases; do
    case "$c" in
        logon) run_case logon "" no --allow-from 100.64.0.0/10 ;;
        boot) run_case boot " (boot)" yes --no-harden --keepalive boot --allow-from 100.64.0.0/10 ;;
        nat) run_nat_case ;;
        *) echo "unknown case $c" >&2; exit 2 ;;
    esac
done
[[ "$cases" == "logon boot" || "$cases" == "logon boot nat" ]] && compare "restored-original" before-logon after-boot

kill "$poller_pid" 2>/dev/null || true; poller_pid=""
ok=$(grep -c . "$work/live.ok" 2>/dev/null || true); fail=$(grep -c . "$work/live.fail" 2>/dev/null || true)
echo "== port $live_port probes during the run: ${ok:-0} answered, ${fail:-0} failed"
if [[ "${fail:-0}" -ne 0 || "${ok:-0}" -lt 5 ]]; then echo "FAIL: port $live_port did not answer throughout"; status=1; fi
[[ "$(probe_login $live_port)" -eq 1 ]] || { echo "FAIL: $live_port does not answer at the end"; status=1; }

[[ $status -eq 0 ]] && echo "HOST ROUNDTRIP OK on $host_name ($ip)" || echo "HOST ROUNDTRIP FAILED on $host_name ($ip)"
exit $status
