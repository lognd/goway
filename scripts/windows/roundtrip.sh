#!/usr/bin/env bash
# Proof for the Windows installer: install --client then uninstall in the goway-test profile on a
# real Windows host leaves user Path, the Uninstall key and the files exactly as they were.
#
#   scripts/windows/roundtrip.sh HOST_NAME [path/to/goway-setup.exe]
#
# frob command evidence needs a bare tool name on PATH: symlink this script as goway-roundtrip
# (ln -s .../scripts/windows/roundtrip.sh ~/.local/bin/goway-roundtrip).
# HOST_NAME (for example Helios) is resolved at run time through Windows mDNS, because the
# hosts' addresses are DHCP leases; a dotted IPv4 address is used as given. Two cases run:
#   1. fresh install, uninstalled with the staged exe (synchronous);
#   2. the Path already holds the install dir, uninstalled with the installed copy, which
#      relaunches from %TEMP% (asynchronous, so the script waits for it).
# Exits nonzero unless every before/after snapshot pair is identical. Only the goway-test
# profile is touched, and the machine is restored on failure too.
set -euo pipefail

host_name="${1:?usage: roundtrip.sh HOST_NAME [goway-setup.exe]}"
root="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd)"  # also works through a PATH symlink
here="$root/scripts/windows"
exe="${2:-$root/target/x86_64-pc-windows-gnu/release/goway-setup.exe}"
profile=goway-test
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

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
rsh() { ssh "${ssh_opts[@]}" "$ip" "$@"; }
psf() { # run an uploaded script: psf script.ps1 [args...]
    local s="$1"; shift
    rsh powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "goway-roundtrip\\$s" "$@" 2>/dev/null | tr -d '\r'
}
snapshot() { psf snapshot.ps1 -ProfileName "$profile" > "$work/$1"; }
setup() { rsh "goway-roundtrip\\goway-setup.exe" "$@" 2>&1 | tr -d '\r'; }

cleanup_remote() { # best effort: leave the machine as found, whatever happened
    set +e
    setup uninstall --profile "$profile" >/dev/null 2>&1
    psf wait-uninstalled.ps1 -ProfileName "$profile" >/dev/null 2>&1
    psf path-entry.ps1 -Action remove -ProfileName "$profile" >/dev/null 2>&1
    rsh "rmdir /s /q goway-roundtrip" >/dev/null 2>&1   # the uploaded exe and scripts
}
trap 'cleanup_remote; rm -rf "$work"' EXIT

rsh "powershell -NoProfile -NonInteractive -Command \"New-Item -ItemType Directory -Force goway-roundtrip | Out-Null\"" 2>/dev/null
scp -q -o BatchMode=yes "$exe" "$here"/*.ps1 "$ip:goway-roundtrip/"

status=0
compare() { # compare label before after
    if diff -u "$work/$2" "$work/$3"; then echo "PASS $1: snapshots identical"; else echo "FAIL $1: snapshots differ"; status=1; fi
}

echo "== case 1: fresh install, staged-exe uninstall"
snapshot before1
setup install --client --profile "$profile" -v
psf verify.ps1 -ProfileName "$profile" | tee "$work/verify1" ; grep -q '^OK' "$work/verify1" || status=1
setup status --profile "$profile"
setup uninstall --profile "$profile" -v
snapshot after1
compare "fresh" before1 after1

echo "== case 2: Path already contains the dir, installed-copy uninstall (relaunch)"
psf path-entry.ps1 -Action add -ProfileName "$profile"
snapshot before2
setup install --client --profile "$profile" -v
psf verify.ps1 -ProfileName "$profile" | tee "$work/verify2" ; grep -q '^OK' "$work/verify2" || status=1
rsh "%LOCALAPPDATA%\\Programs\\$profile\\goway-setup.exe uninstall --profile $profile -v" 2>&1 | tr -d '\r'
psf wait-uninstalled.ps1 -ProfileName "$profile" || status=1
snapshot after2
compare "path-preexisting" before2 after2
grep -q "^path.value=.*goway-test" "$work/after2" && echo "ok: pre-existing Path entry survived uninstall" || { echo "FAIL: pre-existing Path entry was removed"; status=1; }
psf path-entry.ps1 -Action remove -ProfileName "$profile"
snapshot final
compare "restored-original" before1 final

[[ $status -eq 0 ]] && echo "ROUNDTRIP OK on $host_name ($ip)" || echo "ROUNDTRIP FAILED on $host_name ($ip)"
exit $status
