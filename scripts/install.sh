#!/usr/bin/env bash
# Install goway for the current user, reversibly.
#
#   scripts/install.sh            build (cargo, release) and install
#   GOWAY_INSTALL_BINARY=path scripts/install.sh   install a prebuilt binary
#
# Puts goway in ~/.local/bin (GOWAY_PREFIX overrides ~/.local) and, only if
# that directory is not on PATH yet, appends one marked line to ~/.profile.
# Every change is recorded with its prior state in the install journal
# (${XDG_STATE_HOME:-~/.local/state}/goway/install-journal), so
# scripts/uninstall.sh removes exactly what this script added. No root.
set -euo pipefail

say() { printf 'goway-install: %s\n' "$*" >&2; }
die() { say "$*"; exit 1; }

repo=$(cd "$(dirname "$0")/.." && pwd)
prefix=${GOWAY_PREFIX:-$HOME/.local}
bin="$prefix/bin"
state="${XDG_STATE_HOME:-$HOME/.local/state}/goway"
journal="$state/install-journal"
profile="$HOME/.profile"
marker="# added by goway install"

[ -e "$journal" ] && die "already installed (journal $journal); run scripts/uninstall.sh first"
# The bin path is written into ~/.profile; refuse anything a shell could
# read as code (quotes, $, backticks, newlines, ...).
case "$bin" in
  *[!A-Za-z0-9/._+@-]*) die "install prefix '$prefix' has characters goway will not write into ~/.profile; set GOWAY_PREFIX to a plain path" ;;
esac

src=${GOWAY_INSTALL_BINARY:-}
if [ -z "$src" ]; then
  command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust (https://rustup.rs) or set GOWAY_INSTALL_BINARY"
  say "building goway (release)"
  cargo build --locked --release -p goway --manifest-path "$repo/Cargo.toml" >&2
  src="$repo/target/release/goway"
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
say "done; undo with scripts/uninstall.sh"
