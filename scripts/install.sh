#!/usr/bin/env bash
# Install goway for the current user, reversibly.
#
#   curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash
#                                 download the release binary, verify, install
#   scripts/install.sh            from a source checkout: build (cargo) and install
#   GOWAY_INSTALL_BINARY=path scripts/install.sh   install a prebuilt binary
#
# A download is checked against the release's SHA256SUMS before anything is
# installed (GOWAY_RELEASE_URL overrides the release location).
#
# Puts goway in ~/.local/bin (GOWAY_PREFIX overrides ~/.local) and, only if
# that directory is not on PATH yet, appends one marked line to ~/.profile.
# Every change is recorded with its prior state in the install journal
# (${XDG_STATE_HOME:-~/.local/state}/goway/install-journal), so
# scripts/uninstall.sh removes exactly what this script added. No root.
set -euo pipefail

say() { printf 'goway-install: %s\n' "$*" >&2; }
die() { say "$*"; exit 1; }

# Run from a source checkout (scripts/install.sh) or piped from curl.
here=${BASH_SOURCE[0]:-}
repo=""
if [ -n "$here" ] && [ -f "$here" ]; then
  candidate=$(cd "$(dirname "$here")/.." && pwd)
  if [ -f "$candidate/crates/goway/Cargo.toml" ]; then repo=$candidate; fi
fi
prefix=${GOWAY_PREFIX:-$HOME/.local}
bin="$prefix/bin"
state="${XDG_STATE_HOME:-$HOME/.local/state}/goway"
journal="$state/install-journal"
profile="$HOME/.profile"
marker="# added by goway install"

[ -e "$journal" ] && die "already installed (journal $journal); run 'goway uninstall' first"
# The bin path is written into ~/.profile; refuse anything a shell could
# read as code (quotes, $, backticks, newlines, ...).
case "$bin" in
  *[!A-Za-z0-9/._+@-]*) die "install prefix '$prefix' has characters goway will not write into ~/.profile; set GOWAY_PREFIX to a plain path" ;;
esac

# Download the release binary for this machine into $dl and verify its
# checksum; sets src.
download() {
  local base target tmp=$dl expected actual
  base=${GOWAY_RELEASE_URL:-https://github.com/lognd/goway/releases/latest/download}
  case "$(uname -m)" in
    x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
    aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
    *) die "no prebuilt goway for $(uname -m); build it from source: https://github.com/lognd/goway" ;;
  esac
  command -v curl >/dev/null 2>&1 || die "curl is needed to download goway (Ubuntu: sudo apt-get install curl)"
  say "downloading goway for $target"
  curl -fsSL "$base/goway-$target.tar.gz" -o "$tmp/goway.tar.gz" || die "download failed: $base/goway-$target.tar.gz"
  curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
  expected=$(awk -v f="goway-$target.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || die "the release lists no checksum for goway-$target.tar.gz; not installing"
  actual=$(sha256sum "$tmp/goway.tar.gz" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || die "checksum mismatch for goway-$target.tar.gz; not installing"
  tar -xzf "$tmp/goway.tar.gz" -C "$tmp" goway || die "the download does not contain goway"
  say "checksum verified"
  src="$tmp/goway"
}

src=${GOWAY_INSTALL_BINARY:-}
if [ -z "$src" ] && [ -n "$repo" ] && command -v cargo >/dev/null 2>&1; then
  say "building goway (release) from $repo"
  cargo build --locked --release -p goway --manifest-path "$repo/Cargo.toml" >&2
  src="$repo/target/release/goway"
elif [ -z "$src" ]; then
  dl=$(mktemp -d)
  trap 'rm -rf -- "$dl"' EXIT
  download
fi
[ -x "$src" ] || die "no goway binary at $src"

# Create DIR and missing parents; print the ones created, outermost first.
make_dirs() {
  local d=$1 missing=()
  while [ ! -d "$d" ]; do missing=("$d" "${missing[@]}"); d=$(dirname "$d"); done
  for d in "${missing[@]}"; do mkdir "$d"; printf '%s\n' "$d"; done
}

created_state=$(make_dirs "$state")
: >"$journal"
record() { printf '%s\n' "$*" >>"$journal"; }
while IFS= read -r d; do [ -n "$d" ] && record "dir $d"; done <<<"$created_state"

while IFS= read -r d; do [ -n "$d" ] && record "dir $d"; done <<<"$(make_dirs "$bin")"

if [ -e "$bin/goway" ]; then
  if cmp -s "$src" "$bin/goway"; then
    say "$bin/goway is already this build; leaving it"
  else
    die "$bin/goway exists and is a different file; remove it first (nothing else was changed except the journal at $journal)"
  fi
else
  install -m 755 "$src" "$bin/goway"
  record "file $bin/goway $(sha256sum "$bin/goway" | cut -d' ' -f1)"
  say "installed $bin/goway"
fi

case ":$PATH:" in
  *":$bin:"*) say "$bin is already on PATH" ;;
  *)
    line="export PATH=\"$bin:\$PATH\" $marker"
    if [ -f "$profile" ] && grep -qxF "$line" "$profile"; then
      say "$profile already adds $bin to PATH"
    else
      if [ -f "$profile" ]; then existed=1; else existed=0; fi
      # Keep the file ending in a newline before appending.
      if [ "$existed" = 1 ] && [ -s "$profile" ] && [ "$(tail -c 1 "$profile" | od -An -c | tr -d ' ')" != '\n' ]; then
        record "newline $profile"
        printf '\n' >>"$profile"
      fi
      record "line $existed $profile $line"
      printf '%s\n' "$line" >>"$profile"
      say "added $bin to PATH in $profile (open a new login shell to use it)"
    fi
    ;;
esac
say "done; undo with: goway uninstall"
