#!/usr/bin/env bash
# Undo scripts/install.sh exactly, by replaying its journal backwards.
#
# Removes only what the install added: the goway binary (if it is still the
# installed build), the marked PATH line in ~/.profile (and the file if the
# install created it and it is otherwise empty), the newline the install
# had to add, and directories the install created (if they are empty).
# Anything changed since install is left alone and reported.
set -euo pipefail

say() { printf 'goway-uninstall: %s\n' "$*" >&2; }

state="${XDG_STATE_HOME:-$HOME/.local/state}/goway"
journal="$state/install-journal"
[ -f "$journal" ] || { say "not installed (no journal at $journal)"; exit 0; }

# SHA-256 of a file: sha256sum on Linux, shasum -a 256 on macOS.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# The journal is a plain file, so it is not trusted: only a file named goway
# under $HOME, $HOME/.profile and directories under $HOME are ever touched.
in_home() { case "$1" in "$HOME"/*/../* | */.. | "$HOME"/../*) return 1 ;; "$HOME"/*) return 0 ;; *) return 1 ;; esac; }

# Not mapfile: macOS ships bash 3.2.
entries=()
while IFS= read -r l || [ -n "$l" ]; do entries+=("$l"); done <"$journal"
dirs=()
for ((i = ${#entries[@]} - 1; i >= 0; i--)); do
  entry=${entries[$i]}
  kind=${entry%% *}
  rest=${entry#* }
  case "$kind" in
    file)
      path=${rest% *}; sum=${rest##* }
      if ! in_home "$path" || [ "${path##*/}" != goway ]; then
        say "skipped journal entry for $path: not a file the install makes"
      elif [ ! -e "$path" ]; then
        say "$path already gone"
      elif [ "$(sha256 "$path")" = "$sum" ]; then
        rm -f "$path"; say "removed $path"
      else
        say "kept $path: it changed since install"
      fi
      ;;
    line)
      existed=${rest%% *}; rest=${rest#* }
      profile=${rest%% *}; line=${rest#* }
      if [ "$profile" != "$HOME/.profile" ]; then
        say "skipped journal entry for $profile: not the profile the install edits"
      elif [ -f "$profile" ] && grep -qxF "$line" "$profile"; then
        tmp="$profile.goway-uninstall.$$"
        grep -vxF "$line" "$profile" >"$tmp" || true
        if [ "$existed" = 0 ] && [ ! -s "$tmp" ]; then
          rm -f "$tmp" "$profile"; say "removed $profile (created by install)"
        else
          cat "$tmp" >"$profile"; rm -f "$tmp"; say "removed the PATH line from $profile"
        fi
      else
        say "PATH line already gone from $profile"
      fi
      ;;
    newline)
      profile=$rest
      if [ "$profile" = "$HOME/.profile" ] && [ -f "$profile" ] && [ "$(tail -c 1 "$profile" | od -An -c | tr -d ' ')" = '\n' ]; then
        # Drop the last byte (truncate -s -1 is GNU only; macOS has no truncate).
        size=$(wc -c <"$profile")
        tmp="$profile.goway-uninstall.$$"
        dd if="$profile" of="$tmp" bs=1 count=$((size - 1)) 2>/dev/null
        cat "$tmp" >"$profile"; rm -f "$tmp"
      fi
      ;;
    dir)
      if in_home "$rest"; then dirs+=("$rest"); else say "skipped journal entry for $rest: not a directory the install makes"; fi
      ;;
    *) say "unknown journal entry: $entry" ;;
  esac
done

rm -f "$journal"
# Directories were journaled outermost first; dirs[] holds them innermost
# first now, so empty ones disappear from the inside out.
for d in ${dirs[@]+"${dirs[@]}"}; do
  if [ -d "$d" ] && rmdir "$d" 2>/dev/null; then say "removed $d"; fi
done
say "done"
