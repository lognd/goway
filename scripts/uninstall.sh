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

mapfile -t entries <"$journal"
dirs=()
for ((i = ${#entries[@]} - 1; i >= 0; i--)); do
  entry=${entries[$i]}
  kind=${entry%% *}
  rest=${entry#* }
  case "$kind" in
    file)
      path=${rest% *}; sum=${rest##* }
      if [ ! -e "$path" ]; then
        say "$path already gone"
      elif [ "$(sha256sum "$path" | cut -d' ' -f1)" = "$sum" ]; then
        rm -f "$path"; say "removed $path"
      else
        say "kept $path: it changed since install"
      fi
      ;;
    line)
      existed=${rest%% *}; rest=${rest#* }
      profile=${rest%% *}; line=${rest#* }
      if [ -f "$profile" ] && grep -qxF "$line" "$profile"; then
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
      if [ -f "$profile" ] && [ "$(tail -c 1 "$profile" | od -An -c | tr -d ' ')" = '\n' ]; then
        truncate -s -1 "$profile"
      fi
      ;;
    dir) dirs+=("$rest") ;;
    *) say "unknown journal entry: $entry" ;;
  esac
done

rm -f "$journal"
# Directories were journaled outermost first; dirs[] holds them innermost
# first now, so empty ones disappear from the inside out.
for d in "${dirs[@]}"; do
  if [ -d "$d" ] && rmdir "$d" 2>/dev/null; then say "removed $d"; fi
done
say "done"
