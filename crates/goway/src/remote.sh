# goway remote side. Sent inline with every ssh call and run as
#   bash -c 'eval "$(printf %s <base64 of: set -- VERB ARGS; this script> | base64 -d)"' goway
# so the remote needs nothing installed beyond bash, GNU findutils, tar,
# coreutils and util-linux (flock); on macOS the same GNU tools from Homebrew. Every directory goway owns carries a
# meta.json label and a lock file that is flock-held while in use.
set -Eeuo pipefail
umask 077

# Portability. On macOS (BSD userland, bash 3.2) put Homebrew's GNU tools
# first: coreutils, findutils, gnu-sed, gnu-tar and grep ship "gnubin"
# directories of unprefixed GNU names, flock and util-linux (setsid) their
# own bin. Nothing here changes what runs on Linux.
IS_DARWIN=0
FDDIR=/proc/self/fd
if [ "$(uname -s)" = Darwin ]; then
  IS_DARWIN=1
  FDDIR=/dev/fd
  # sysctl, vm_stat and perl live in system directories a caller's PATH may omit.
  PATH="$PATH:/usr/sbin:/sbin:/usr/bin:/bin"
  for brew in /opt/homebrew /usr/local; do
    for d in "$brew"/opt/*/libexec/gnubin "$brew"/opt/util-linux/bin "$brew"/opt/util-linux/sbin "$brew"/opt/flock/bin "$brew/bin"; do
      [ -d "$d" ] && PATH="$d:$PATH"
    done
  done
  export PATH
  # Without util-linux's setsid, perl (always on macOS) starts a new session.
  if ! command -v setsid >/dev/null 2>&1; then
    setsid() { perl -e 'use POSIX qw(setsid); setsid(); exec @ARGV or exit 127' -- "$@"; }
  fi
fi

die() { printf 'goway-remote: %s\n' "$*" >&2; exit 125; }
# Any failure of the script itself exits 125, never a command-like code.
trap 'printf "goway-remote: failed at line %s\n" "$LINENO" >&2; exit 125' ERR

# Resolve the state root (relative paths are under $HOME).
root_dir() {
  case "$1" in
    /*) printf '%s' "$1" ;;
    *) printf '%s/%s' "$HOME" "$1" ;;
  esac
}

# Mark ROOT as goway's (gc refuses to remove anything under an unmarked root).
# A directory that already exists, holds anything besides goway's own work,
# seed and cache directories, and carries no marker is somebody else's: refuse
# to adopt it, so a mis-set remote_root can never later be purged.
mark_root() {
  local e
  [ -e "$1/.goway-root" ] && return 0
  mkdir -p "$1"
  for e in "$1"/* "$1"/.[!.]* "$1"/..?*; do
    [ -e "$e" ] || [ -L "$e" ] || continue
    case "${e##*/}" in
      work | seed | cache | gpu | footprints | mempeaks | gc.lock | evicted.log | .goway-root) ;;
      *) die "$1 exists, is not empty and is not goway state; pick a dedicated remote_root" ;;
    esac
  done
  printf 'goway state; safe to delete with goway gc --all\n' >"$1/.goway-root"
}

# same_fd PATH FD: whether FD is open on the file PATH names now (a lock
# file gc removed and something re-created is a different file). Linux
# compares with /proc/self/fd; macOS has no such link, so perl fstat()s the
# inherited descriptor.
if [ "$IS_DARWIN" = 1 ]; then
  same_fd() {
    local want
    want=$(stat -c %d:%i "$1" 2>/dev/null) || return 1
    [ "$want" = "$(perl -e 'open(my $f, "<&=", $ARGV[0]) or exit 1; my @s = stat($f); print "$s[0]:$s[1]"' "$2")" ]
  }
else
  same_fd() { [ "$1" -ef "$FDDIR/$2" ]; }
fi

# lock_dir FD DIR FLOCK_OPT: create DIR if needed and hold DIR/lock on FD
# (FLOCK_OPT is -x or -s). gc may remove DIR at any moment, so the open can
# fail (DIR vanished) or lock an unlinked file; both retry until the held
# lock file is the one DIR currently has. GOWAY_TEST_HOOK (tests only) is
# evaluated once between the open and the flock to make that race happen.
lock_dir() {
  local fd=$1 dir=$2 opt=$3 tries=0
  while [ $tries -lt 200 ]; do
    tries=$((tries + 1))
    mkdir -p "$dir" 2>/dev/null || true
    if eval "exec $fd>\"\$dir/lock\"" 2>/dev/null; then
      if [ -n "${GOWAY_TEST_HOOK:-}" ]; then
        local hook=$GOWAY_TEST_HOOK
        unset GOWAY_TEST_HOOK
        eval "$hook"
      fi
      flock "$opt" "$fd"
      if same_fd "$dir/lock" "$fd"; then return 0; fi
    fi
    sleep 0.05
  done
  die "cannot lock $dir (kept vanishing)"
}

# user_tool_path: make the usual per-user tool directories visible to a
# non-interactive ssh command, whose PATH never saw .profile or .bashrc.
# ~/.local/bin (mold, uv tools, goway's own fixes) goes first, then
# ~/.cargo/bin if it is not already on PATH; the uv, node and go locations
# that exist are appended (a system install still wins). Startup files are
# never sourced: they may print text or run anything. Shared by run and doctor.
user_tool_path() {
  local d n
  case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) PATH="$HOME/.local/bin:$PATH" ;; esac
  case ":$PATH:" in *":$HOME/.cargo/bin:"*) ;; *) [ -d "$HOME/.cargo/bin" ] && PATH="$HOME/.cargo/bin:$PATH" ;; esac
  n=$(ls -d "$HOME"/.nvm/versions/node/*/bin 2>/dev/null | sort -V | tail -1 || true)
  for d in "$HOME/.local/share/uv/bin" "$HOME/.volta/bin" "$n" "$HOME/.local/share/fnm/aliases/default/bin" /usr/local/go/bin "$HOME/go/bin"; do
    [ -n "$d" ] && [ -d "$d" ] || continue
    case ":$PATH:" in *":$d:"*) ;; *) PATH="$PATH:$d" ;; esac
  done
  export PATH
}

# Change-log entries a seed keeps.
LOG_KEEP=64

new_generation() { printf '%s-%s-%s\n' "$(date +%s%N)" "$$" "$RANDOM"; }

# Start a new worktree's seed as a hard-link copy of the most recently used
# seed of the same repository, so its first sync only sends differences.
# Safe because receive replaces files by unlink and recreate.
seed_from_sibling() {
  local seed=$1 m d sib=""
  for m in $(ls -t "$(dirname "$seed")"/*/meta.json 2>/dev/null); do
    d=$(dirname "$m")
    if [ "$d" != "$seed" ] && [ -d "$d/tree" ]; then sib=$d; break; fi
  done
  [ -n "$sib" ] || return 0
  lock_dir 6 "$sib" -s
  rm -rf "$seed/tree.new"
  cp -al "$sib/tree" "$seed/tree.new"
  new_generation >"$seed/generation"
  mv "$seed/tree.new" "$seed/tree"
  exec 6>&-
}

# nearest_dir PATH: PATH, or its closest existing ancestor.
nearest_dir() {
  local d=$1
  while [ ! -d "$d" ] && [ "$d" != / ] && [ "$d" != . ]; do d=$(dirname "$d"); done
  printf '%s' "$d"
}

# case_insensitive DIR: succeed when the file system under DIR ignores case
# (macOS APFS by default, exFAT, a Windows drive under WSL, ext4 casefold).
# GOWAY_ASSUME_CASE_INSENSITIVE=1 forces it (a test hook).
case_insensitive() {
  local d probe
  [ "${GOWAY_ASSUME_CASE_INSENSITIVE:-0}" = 1 ] && return 0
  d=$(nearest_dir "$1")
  probe="$d/.goway-Case-$$"
  : >"$probe" 2>/dev/null || return 1
  if [ -e "$d/.goway-case-$$" ]; then rm -f "$probe"; return 0; fi
  rm -f "$probe"
  return 1
}

# case_clashes LIST TREE [DELETIONS]: read tar member names from LIST plus
# the names under TREE (less the NUL-separated paths in DELETIONS, which the
# sync removes first); print each pair of distinct paths that differ only
# in case.
case_clashes() {
  local gone=/dev/null
  [ -n "${3:-}" ] && [ -f "$3" ] && { gone=$(mktemp "$(dirname "$2")/gone.XXXXXX"); tr '\0' '\n' <"$3" >"$gone"; }
  { cat "$1"
    (cd "$2" 2>/dev/null && find . -mindepth 1 \( -type f -o -type l \) -print | sed 's|^\./||' | { grep -vxF -f "$gone" || true; }); } |
    sed 's|/$||' | sort -u |
    awk '{ k = tolower($0); if ((k in seen) && seen[k] != $0) print "  " seen[k] "  and  " $0; else seen[k] = $0 }'
  [ "$gone" = /dev/null ] || rm -f "$gone"
}

# manifest ROOT SEED: print the seed tree as NUL-terminated records
#   type TAB size TAB mtime TAB mode TAB linktarget TAB path
manifest() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  mark_root "$root"
  if [ ! -d "$seed/tree" ]; then
    lock_dir 8 "$seed" -x
    [ -d "$seed/tree" ] || [ -e "$seed/fresh" ] || seed_from_sibling "$seed"
  fi
  lock_dir 8 "$seed" -s
  [ -d "$seed/tree" ] || return 0
  # The generation names this incarnation of the tree; receive refuses to
  # patch a tree whose generation changed since this manifest (gc removed
  # and something recreated it), so a delta never lands on the wrong base.
  printf 'G\t0\t0\t0\t\t%s\0' "$(cat "$seed/generation" 2>/dev/null || true)"
  find "$seed/tree" -mindepth 1 \( -type f -o -type l \) \
    -printf '%y\t%s\t%T@\t%m\t%l\t%P\0'
}

# hashes ROOT SEED: NUL-separated paths on stdin; print "sha256  path\0"
# for each regular file (goway compares content when only mtimes differ).
hashes() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  lock_dir 8 "$seed" -s
  cd "$seed/tree"
  xargs -0 -r sha256sum -z -- 2>/dev/null || true
}

# deletions ROOT SEED ATTEMPT: NUL-separated paths on stdin to delete at
# the receive of the same sync ATTEMPT (stdin, not an argument: one
# argument is limited to 128 KiB). A list left by a failed attempt is
# never applied by a later one.
deletions() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  case "$3" in *[!A-Za-z0-9-]* | "") die "deletions: bad attempt id" ;; esac
  mark_root "$root"
  lock_dir 8 "$seed" -x
  cat >"$seed/deletions.$3"
}

# changes ROOT SEED ATTEMPT: NUL-separated paths on stdin that the receive of
# the same sync ATTEMPT will write. receive appends them to the seed's change
# log, which tells a slot exactly which files may differ although their size
# and mtime (whole seconds) match.
changes() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  case "$3" in *[!A-Za-z0-9-]* | "") die "changes: bad attempt id" ;; esac
  mark_root "$root"
  lock_dir 8 "$seed" -x
  cat >"$seed/changes.$3"
}

# receive ROOT SEED META_B64 GENERATION RUN_ID WORK_META_B64 KEEP ATTEMPT:
# under the seed's exclusive lock, check that the seed is still the one the
# manifest described (GENERATION, empty for "no tree yet"), apply pending
# deletions, extract the tar on stdin, and (when RUN_ID is given) snapshot
# the tree into the run's fresh work dir (a hard-link farm) in the same
# critical section. Files are replaced by unlink and recreate, so
# hard-linked snapshots keep their content. Exit 75 with "seed changed" when the generation differs.
receive() {
  local root seed gen work
  root=$(root_dir "$1"); seed="$root/seed/$2"
  mark_root "$root"
  lock_dir 8 "$seed" -x
  gen=$(cat "$seed/generation" 2>/dev/null || true)
  if [ ! -d "$seed/tree" ]; then gen=""; fi
  local attempt=${8:-}
  case "$attempt" in *[!A-Za-z0-9-]*) die "receive: bad attempt id" ;; esac
  if [ "$gen" != "$4" ]; then
    rm -f "$seed"/deletions.* "$seed"/changes.*
    printf 'goway-remote: seed changed (have "%s", expected "%s")\n' "$gen" "$4" >&2
    cat >/dev/null
    exit 75
  fi
  if [ ! -d "$seed/tree" ]; then
    mkdir -p "$seed/tree"
    new_generation >"$seed/generation"
  fi
  # On a case-insensitive file system two paths that differ only in case
  # would silently become one file: refuse, naming them, before touching
  # anything. The stream is spooled to a file so it can be listed first.
  local tarsrc=-
  if case_insensitive "$root"; then
    tarsrc="$seed/incoming.tar"
    cat >"$tarsrc"
    local clash
    clash=$(tar -tf "$tarsrc" | case_clashes /dev/stdin "$seed/tree" "$seed/deletions.$attempt")
    if [ -n "$clash" ]; then
      rm -f "$tarsrc" "$seed"/deletions.* "$seed"/changes.*
      printf 'goway-remote: this repository has paths that differ only in case, and this host'"'"'s file system ignores case:\n%s\n' "$clash" >&2
      printf 'goway-remote: next: use a host with a case-sensitive file system (set remote_root there), or rename one of each pair\n' >&2
      exit 76
    fi
  fi
  printf '%s' "$3" | base64 -d >"$seed/meta.json"
  if [ -n "$attempt" ] && [ -f "$seed/deletions.$attempt" ]; then
    (cd "$seed/tree" && xargs -0 -r rm -f -- <"$seed/deletions.$attempt")
  fi
  rm -f "$seed"/deletions.*
  # The change log: one numbered file per sync that wrote files (the last
  # LOG_KEEP are kept). A slot reconciled up to number n only needs the
  # entries after n to know which files to compare by content.
  mkdir -p "$seed/changes"
  local seq
  seq=$(cat "$seed/seq" 2>/dev/null || echo 0)
  if [ -n "$attempt" ] && [ -f "$seed/changes.$attempt" ]; then
    seq=$((seq + 1))
    mv "$seed/changes.$attempt" "$seed/changes/$seq"
    printf '%s' "$seq" >"$seed/seq"
    (cd "$seed/changes" && ls | sort -n | head -n -"$LOG_KEEP" | xargs -r rm -f --)
  fi
  rm -f "$seed"/changes.*
  rm -f "$seed/fresh"
  # Files dated in this host's future (the laptop's clock runs ahead) are not
  # an error: the slot's copies get this host's time (sync_slot), so tar's
  # "time stamp is in the future" warnings are only noise.
  tar -x --unlink-first --recursive-unlink --no-same-owner --warning=no-timestamp -C "$seed/tree" -f "$tarsrc"
  if [ "$tarsrc" != - ]; then rm -f "$tarsrc"; fi
  find "$seed/tree" -mindepth 1 -depth -type d -empty -delete
  if [ -n "${5:-}" ]; then
    work="$root/work/$5"
    mkdir -p "$work"
    # What keeps gc from removing this dir before its run takes the lock,
    # whatever the wall clock does: this process while it lives, then the
    # monotonic time since boot (see work_young).
    printf '%s %s\n' "$$" "$(proc_start "$$")" >"$work/creator"
    printf '%s\n' "$(uptime_secs)" >"$work/born"
    printf '%s' "$6" | base64 -d >"$work/meta.json"
    if [ "${7:-0}" = 1 ]; then : >"$work/keep"; fi
    printf '%s' "$2" >"$work/seed"
    printf '%s %s' "$(cat "$seed/generation")" "$seq" >"$work/seqinfo"
    cp -al "$seed/changes" "$work/changes"
    # A hard-link snapshot: no data is copied. The seed is only ever
    # changed by unlink and recreate (above and in seed_from_sibling), so a
    # later sync never changes what this snapshot holds, and nothing writes
    # through the snapshot: the job runs in a slot tree (see sync_slot).
    cp -al "$seed/tree" "$work/tree"
  fi
}

# One tree entry as a record sorted and compared as a whole: type, size,
# mtime, mode, link target, then the path last (so it may hold any byte
# but NUL). Fields are separated by SOH.
SOH=$'\001'
REC='%y\001%s\001%T@\001%m\001%l\001%P\0'

# The path part of each record on stdin.
rec_paths() { sed -z "s/^\\([^$SOH]*$SOH\\)\\{5\\}//"; }

# Escape glob characters so a directory name matches only itself in find -path.
glob_escape() {
  local s=$1
  s=${s//\\/\\\\}; s=${s//\*/\\*}; s=${s//\?/\\?}; s=${s//\[/\\[}
  printf '%s' "$s"
}

# detect_keep TREE: the dependency and build directories this project is
# known to produce, from the marker files present, one entry per line. An
# entry with a leading / is relative to the tree root (and a glob); one
# without matches a directory of that name at any depth.
detect_keep() {
  local tree=$1 name rel dir pre us=$'\037'
  # Fields are split on US, not SOH: bash 3.2 (macOS) mishandles IFS=$'\001' in read.
  while IFS=$us read -r -d '' name rel; do
    dir=""; [ "$rel" = "${rel%/*}" ] || dir=$(glob_escape "${rel%/*}")
    pre="/${dir:+$dir/}"
    case "$name" in
      package.json) printf '%s\n' node_modules "${pre}.next" "${pre}.nuxt" ;;
      pyproject.toml | requirements*.txt)
        printf '%s\n' __pycache__ "${pre}.venv" "${pre}venv" "${pre}.tox" "${pre}.nox" \
          "${pre}.pytest_cache" "${pre}.mypy_cache" "${pre}.ruff_cache" ;;
      CMakeLists.txt) printf '%s\n' "${pre}build" "${pre}cmake-build-*" ;;
      pom.xml) printf '%s\n' "${pre}target" ;;
      build.gradle | build.gradle.kts) printf '%s\n' "${pre}.gradle" "${pre}build" ;;
    esac
  done < <(find "$tree" \( -type d \( -name node_modules -o -name .git \) -prune \) -o \
    -type f \( -name package.json -o -name pyproject.toml -o -name 'requirements*.txt' \
    -o -name CMakeLists.txt -o -name pom.xml -o -name build.gradle -o -name build.gradle.kts \) \
    -printf "%f\\037%P\\0")
}

# sha_records DIR LIST: "sha256  path" records (NUL-terminated, sorted) of the
# files named in LIST (NUL-separated, relative to DIR); missing files are skipped.
sha_records() {
  (cd "$1" && xargs -0 -r sha256sum -z -- <"$2" 2>/dev/null || true) | sort -z
}

# sync_slot FARM SLOT WORK KEEP_IGNORED KEEP_B64 SEEDKEY TARGET: update SLOT
# in place so it holds exactly the files of FARM (the run's snapshot) plus
# what the keep set preserves; no run sees another's leftovers.
#
# A file is written only when it may differ: it is new or changed in the
# snapshot since the slot's last reconcile (SLOT.farm holds the snapshot
# records then), the job changed or deleted it (SLOT.slot holds the slot's own
# records then), it is missing, or the seed's change log names it (same-size
# edits within one second are invisible to size and whole-second mtime). A
# candidate whose content already equals the snapshot's is left alone, so a
# worktree switch keeps the mtimes of files that did not change. Every file
# actually written gets the current time as mtime, like git checkout: make,
# ninja and cargo rebuild only when a source is newer than its output, and
# an older branch's file must never look older than another branch's build.
# If anything in the slot or TARGET is dated in the future (clock steps,
# archives), written files are stamped one second after the newest such file.
# The keep set: detected dependency/build dirs, the configured entries, and
# (when KEEP_IGNORED is 1 and git exists) every path the snapshot's .gitignore
# rules ignore. Prints "written=N removed=M" to the file SLOT.stats.
sync_slot() {
  local farm=$1 slot=$2 work=$3 keepignored=$4 keepb64=$5 seedkey=${6:-} target=${7:-}
  local tmp="$work/reconcile" e kexpr=() prune=() written removed now newest stamp
  local have_key="" have_gen="" have_seq="" cur_gen="" cur_seq="" n full=1
  export LC_ALL=C
  rm -rf "$tmp"; mkdir -p "$tmp" "$slot"
  : >"$tmp/empty"

  # The keep set as a find expression over SLOT.
  { detect_keep "$farm"; printf '%s' "$keepb64" | base64 -d; printf '\n'; } | sort -u >"$tmp/keep"
  while IFS= read -r e; do
    [ -n "$e" ] || continue
    e=${e#./}
    case "$e" in
      /*) kexpr+=(-o -path "$(glob_escape "$slot")$e") ;;
      */*) kexpr+=(-o -path "$(glob_escape "$slot")/$e") ;;
      *) kexpr+=(-o -name "$e") ;;
    esac
  done <"$tmp/keep"
  if [ ${#kexpr[@]} -gt 0 ]; then
    prune=('(' "${kexpr[@]:1}" ')' -prune -o)
  fi

  find "$farm" -mindepth 1 \( -type f -o -type l \) -printf "$REC" | sort -z >"$tmp/snap"
  find "$slot" -mindepth 1 ${prune[@]+"${prune[@]}"} \( -type f -o -type l \) -printf "$REC" | sort -z >"$tmp/slot"
  [ -f "$slot.farm" ] || cp "$tmp/empty" "$slot.farm"
  [ -f "$slot.slot" ] || cp "$tmp/empty" "$slot.slot"
  rec_paths <"$tmp/snap" | sort -z >"$tmp/snap.p"
  rec_paths <"$tmp/slot" | sort -z >"$tmp/slot.p"
  { grep -z '^f' "$tmp/snap" || true; } | rec_paths | sort -z >"$tmp/reg.p"

  # Candidates: paths that may differ.
  #  A: new or changed in the snapshot since the last reconcile
  comm -z -23 "$tmp/snap" "$slot.farm" | rec_paths >"$tmp/c.a"
  #  B: changed or removed in the slot since the last reconcile (a job wrote there)
  comm -z -23 "$slot.slot" "$tmp/slot" | rec_paths | sort -z | comm -z -12 - "$tmp/snap.p" >"$tmp/c.b"
  #  C: missing from the slot listing (new, or under a kept dir)
  comm -z -23 "$tmp/snap.p" "$tmp/slot.p" >"$tmp/c.c"
  #  D: files the seed's change log names since the slot's last reconcile;
  #     all regular files when the slot's last reconcile was another seed's
  if [ -f "$slot.state" ]; then read -r have_key have_gen have_seq <"$slot.state" || true; fi
  if [ -f "$work/seqinfo" ]; then read -r cur_gen cur_seq <"$work/seqinfo" || true; fi
  : >"$tmp/c.d"
  if [ -n "$seedkey" ] && [ "$have_key" = "$seedkey" ] && [ "$have_gen" = "$cur_gen" ] &&
    [[ "$have_seq" =~ ^[0-9]+$ ]] && [[ "$cur_seq" =~ ^[0-9]+$ ]]; then
    full=0
    for ((n = have_seq + 1; n <= cur_seq; n++)); do
      if [ -f "$work/changes/$n" ]; then cat "$work/changes/$n" >>"$tmp/c.d"; else full=1; break; fi
    done
  fi
  if [ $full = 1 ]; then cp "$tmp/reg.p" "$tmp/c.d"; fi
  cat "$tmp"/c.[abcd] | sort -zu | comm -z -12 - "$tmp/snap.p" >"$tmp/cand"
  # Regular-file candidates already holding the snapshot's content are not written.
  comm -z -12 "$tmp/cand" "$tmp/reg.p" >"$tmp/cand.reg"
  sha_records "$slot" "$tmp/cand.reg" >"$tmp/h.slot"
  sed -z 's/^.\{66\}//' "$tmp/h.slot" >"$tmp/exist.p"
  sha_records "$farm" "$tmp/exist.p" >"$tmp/h.farm"
  comm -z -12 "$tmp/h.farm" "$tmp/h.slot" | sed -z 's/^.\{66\}//' | sort -z >"$tmp/same.p"
  comm -z -23 "$tmp/cand" "$tmp/same.p" >"$tmp/todo.p"

  # Slot paths the snapshot does not have at all, minus the ignored ones.
  comm -z -23 "$tmp/slot.p" "$tmp/snap.p" >"$tmp/stale"
  if [ "$keepignored" = 1 ] && [ -s "$tmp/stale" ] && command -v git >/dev/null 2>&1; then
    git init -q --bare "$tmp/git"
    GIT_DIR="$tmp/git" GIT_WORK_TREE="$farm" git -c core.excludesFile=/dev/null \
      check-ignore --no-index -z --stdin <"$tmp/stale" >"$tmp/ignored" 2>/dev/null || true
    sort -z "$tmp/ignored" | comm -z -23 "$tmp/stale" - >"$tmp/stale.final"
  else
    mv "$tmp/stale" "$tmp/stale.final"
  fi
  removed=$(tr -cd '\0' <"$tmp/stale.final" | wc -c)
  written=$(tr -cd '\0' <"$tmp/todo.p" | wc -c)
  (cd "$slot" && xargs -0 -r rm -f -- <"$tmp/stale.final")
  # Directories left empty (not the kept ones) go too, innermost first.
  while :; do
    find "$slot" -mindepth 1 ${prune[@]+"${prune[@]}"} -type d -empty -print0 >"$tmp/emptydirs"
    [ -s "$tmp/emptydirs" ] || break
    xargs -0 -r rmdir -- <"$tmp/emptydirs"
  done
  if [ "$written" -gt 0 ]; then
    now=$(date +%s.%N)
    # Written files are stamped with the current time; the newest future date
    # in the slot or its target dir (if any) pushes the stamp past it.
    newest=$(find "$slot" ${target:+"$target"} -newermt "@$now" -printf '%T@\n' 2>/dev/null |
      sort -n | tail -1 || true)
    stamp=0
    if [ -n "$newest" ]; then stamp=$((${newest%%.*} + 1)); fi
    (cd "$farm" && xargs -0 -r cp --no-dereference --preserve=mode --parents \
      --remove-destination --reflink=auto -t "$slot" -- <"$tmp/todo.p")
    if [ "$stamp" -gt 0 ]; then
      (cd "$slot" && xargs -0 -r touch -h -d "@$stamp" -- <"$tmp/todo.p")
    fi
  fi
  # What the copy-integrity check (verify_gate) looks at: the regular files
  # and symlinks this sync wrote, and all of the snapshot's.
  comm -z -12 "$tmp/todo.p" "$tmp/reg.p" >"$work/written.reg"
  comm -z -23 "$tmp/todo.p" "$tmp/reg.p" >"$work/written.lnk"
  cp "$tmp/reg.p" "$work/all.reg"
  comm -z -23 "$tmp/snap.p" "$tmp/reg.p" >"$work/all.lnk"
  # What the next reconcile compares against: the snapshot and the slot as
  # they are now (the job's changes show up as differences from the latter).
  cp "$tmp/snap" "$slot.farm"
  find "$slot" -mindepth 1 ${prune[@]+"${prune[@]}"} \( -type f -o -type l \) -printf "$REC" | sort -z >"$slot.slot"
  printf '%s %s' "$seedkey" "$(cat "$work/seqinfo" 2>/dev/null)" >"$slot.state"
  printf 'written=%s removed=%s\n' "$written" "$removed" >"$slot.stats"
  rm -rf "$tmp"
}

# claims DIR REG_LIST LNK_LIST: what DIR holds for the named paths, as
# NUL-terminated records "f SOH sha256 SOH path" (regular files) and
# "l SOH target SOH path" (symlinks). The lists are NUL-separated paths
# relative to DIR. GOWAY_TEST_CORRUPT (tests only) falsifies the first hash.
claims() {
  local dir=$1 p corrupt=0
  # Test hook: 1 falsifies every verification, first only attempt 1's, after
  # only the post-failure check of attempt 1.
  case "${GOWAY_TEST_CORRUPT:-}" in
    1) corrupt=1 ;;
    first) [ "${attempt:-1}" = 1 ] && corrupt=1 ;;
    after) [ "${attempt:-1}" = 1 ] && [ "${phase:-1}" = 2 ] && corrupt=1 ;;
  esac
  (cd "$dir" && xargs -0 -r sha256sum -z -- <"$2" 2>/dev/null || true) |
    sed -z "s/^\\(.\\{64\\}\\)  /f$SOH\\1$SOH/" |
    if [ $corrupt = 1 ]; then
      sed -z "1s/^f$SOH.\\{64\\}/f${SOH}0000000000000000000000000000000000000000000000000000000000000000/"
    else cat; fi
  while IFS= read -r -d '' p; do
    printf 'l%s%s%s%s\0' "$SOH" "$(cd "$dir" && readlink -- "$p")" "$SOH" "$p"
  done <"$3"
}

# slot_wipe SLOT_NUMBER CACHE [SEEDKEY ROOT]: throw away a slot's tree, its
# state and its cargo target dir; with SEEDKEY also the seed this run was
# given (marked so it is rebuilt from the laptop, never from a sibling seed),
# so the next attempt copies everything again.
slot_wipe() {
  local slot=$1 cache=$2 seedkey=${3:-} root=${4:-}
  rm -rf "$cache/tree-$slot" "$cache/target-$slot" "$cache"/tree-"$slot".{farm,slot,state,stats}
  case "$seedkey" in "" | *[!A-Za-z0-9._/-]* | */../* | ../* | *..) return 0 ;; esac
  if [ -d "$root/seed/$seedkey" ]; then
    rm -rf "$root/seed/$seedkey/tree" "$root/seed/$seedkey/generation" "$root/seed/$seedkey/changes"
    : >"$root/seed/$seedkey/fresh"
  fi
  return 0
}

# tree_stamps DIR: sorted records "ctime SOH size SOH path" of DIR's files
# and links (NUL-terminated).
tree_stamps() {
  find "$1" -mindepth 1 \( -type f -o -type l \) -printf "%C@${SOH}%s${SOH}%P\\0" | sort -z
}

# verify_gate WORK PHASE DIR REG LNK: publish the claims about the named files
# (verify.PHASE) and block until goway answers with a verdict (verdict.PHASE:
# ok or bad). Returns 0 for ok; 1 for bad, or when no answer comes (goway
# went away or never answered within two minutes).
verify_gate() {
  local work=$1 phase=$2 i v
  { printf 'goway-verify1\0'; claims "$3" "$4" "$5"; } >"$work/verify.$phase.tmp"
  mv "$work/verify.$phase.tmp" "$work/verify.$phase"
  for ((i = 0; i < 1200; i++)); do
    if [ -f "$work/verdict.$phase" ]; then
      v=$(cat "$work/verdict.$phase")
      [ "$v" = ok ] && return 0
      return 1
    fi
    kill -0 "$PPID" 2>/dev/null || return 1
    sleep 0.1
  done
  return 1
}

# lost_note WORK: say why the helper stopped this run, when its lifeline did.
lost_note() {
  local why
  [ -s "$1/lost" ] || return 0
  why=$(head -1 "$1/lost" 2>/dev/null || true)
  printf 'goway: this run was stopped by the helper: the client was considered gone because %s\n' "$why" >&2
}

# remove_work DIR: best-effort removal of a finished run's work dir. It never
# fails and never changes the command's exit code: something may still write
# there (a job's detached child, a concurrent gc), so it retries a few times
# and otherwise leaves the directory for gc, which collects any unlocked work
# dir past its orphan age.
remove_work() {
  local n
  for n in 1 2 3 4 5; do
    if rm -rf "$1" 2>/dev/null; then return 0; fi
    sleep 0.2
  done
  printf 'goway: note: could not remove the work dir of this run; gc will collect it\n' >&2
  return 0
}

# verify_failed PHASE: goway judged the copy bad (or never answered): wipe the
# slot and the seed, say so, and stop. Uses run's variables (dynamic scope).
verify_failed() {
  printf 'goway-remote: the copy of the tree on this host did not verify (phase %s); slot %s is discarded\n' \
    "$1" "$slot" >&2
  slot_wipe "$slot" "$cache" "$(cat "$work/seed" 2>/dev/null || true)" "$root"
  remove_work "$work"
  exit 125
}

# verify_wait ROOT RUN_ID PHASE [SECONDS]: for goway's control call. Prints
# "ready" and the claims once the run published them, "ended" once the run is
# over, or "pending" after SECONDS (default 10, at most 55: ask again).
verify_wait() {
  local root work i seen=0 window=${4:-10}
  case "$window" in "" | *[!0-9]*) die "verify-wait: bad window" ;; esac
  [ "$window" -le 55 ] || window=55
  root=$(root_dir "$1"); work="$root/work/$2"
  case "$2$3" in *[!A-Za-z0-9-]* | "") die "verify-wait: bad argument" ;; esac
  for ((i = 0; i < window * 10; i++)); do
    if [ -f "$work/verify.$3" ]; then
      printf 'ready\n'
      cat "$work/verify.$3"
      return 0
    fi
    if [ -f "$work/lock" ]; then
      if flock -n "$work/lock" true 2>/dev/null; then
        seen=$((seen + 1))
        [ $seen -ge 3 ] && { printf 'ended\n'; return 0; }
      else
        seen=0
      fi
    elif [ ! -d "$work" ]; then
      printf 'ended\n'
      return 0
    fi
    sleep 0.1
  done
  printf 'pending\n'
}

# verify_verdict ROOT RUN_ID PHASE ok|bad: goway's answer to verify_gate.
verify_verdict() {
  local root work
  root=$(root_dir "$1"); work="$root/work/$2"
  case "$2$3" in *[!A-Za-z0-9-]* | "") die "verify-verdict: bad argument" ;; esac
  case "$4" in ok | bad) ;; *) die "verify-verdict: bad verdict" ;; esac
  [ -d "$work" ] || die "verify-verdict: no work dir"
  printf '%s' "$4" >"$work/verdict.$3.tmp"
  mv "$work/verdict.$3.tmp" "$work/verdict.$3"
}

# stop_group PID: stop the process group PID (a job's session leader):
# SIGTERM, up to 5 seconds for the group to empty, then SIGKILL for whatever
# is left. Bounded; never fails.
stop_group() {
  local pid=$1 n
  kill -TERM -- "-$pid" 2>/dev/null || true
  for n in $(seq 1 25); do
    kill -0 -- "-$pid" 2>/dev/null || return 0
    sleep 0.2 || true
  done
  kill -KILL -- "-$pid" 2>/dev/null || true
}

# run_pids RUN_ID [RUNNER]: the pids of every process that carries the run's GOWAY_RUN_ID in its
# environment, one per line, however it regrouped (setsid, double fork), except this shell
# and the run's own shell RUNNER (it must live on to clean up). Linux only (it reads /proc):
# nothing on macOS, where a job that leaves its session and process group is not caught.
run_pids() {
  local f me=$$ self=${BASHPID:-$$} p runner=${2:-} kv
  [ -d /proc/self ] || return 0
  # Builtins only (no grep, no pipeline): a helper process of this scan would carry the
  # run's tag itself and be taken for a leftover.
  for f in /proc/[0-9]*/environ; do
    p=${f#/proc/}
    p=${p%/environ}
    if [ "$p" = "$me" ] || [ "$p" = "$self" ] || [ "$p" = "$runner" ] || [ ! -r "$f" ]; then continue; fi
    { while IFS= read -r -d '' kv; do
      if [ "$kv" = "GOWAY_RUN_ID=$1" ]; then printf '%s\n' "$p"; break; fi
    done <"$f"; } 2>/dev/null || true
  done
}

# kill_run RUN_ID RUNNER: stop every process tagged with the run (run_pids), the backstop for a
# job that left its process group and its scope. SIGTERM, a short grace, then SIGKILL.
kill_run() {
  local pids
  pids=$(run_pids "$1" "$2" || true)
  [ -n "$pids" ] || return 0
  # shellcheck disable=SC2086 # the list is words by construction
  kill -TERM $pids 2>/dev/null || true
  sleep 1 || true # the sweep may kill this very sleep (it carries the run's tag)
  pids=$(run_pids "$1" "$2" || true)
  [ -n "$pids" ] || return 0
  # shellcheck disable=SC2086
  kill -KILL $pids 2>/dev/null || true
}

# scope_procs CGROUP_DIR: whether the cgroup still has a process.
scope_procs() {
  [ -n "$(cat "$1/cgroup.procs" 2>/dev/null || true)" ]
}

# stop_scope UNIT CGROUP: stop a job's whole systemd scope: `systemctl --user stop` (SIGTERM
# to every process in the cgroup, SIGKILL after the scope's TimeoutStopSec), and if that did
# not empty the cgroup (no user bus, a wedged manager) kill the cgroup itself: cgroup.kill,
# else SIGKILL to each pid in cgroup.procs. CGROUP is the cgroup v2 directory ("" if unknown).
# Bounded; never fails.
stop_scope() {
  local unit=$1 cg=$2 p n
  bounded_for 15 systemctl --user stop "$unit" >/dev/null 2>&1 || true
  [ -n "$cg" ] && [ -d "$cg" ] || return 0
  for n in 1 2 3 4 5; do
    scope_procs "$cg" || return 0
    sleep 0.2 || true
  done
  if [ -w "$cg/cgroup.kill" ]; then
    printf '1' >"$cg/cgroup.kill" 2>/dev/null || true
  else
    for p in $(cat "$cg/cgroup.procs" 2>/dev/null || true); do kill -KILL "$p" 2>/dev/null || true; done
  fi
}

# stop_job WORK PID RUN_ID: end everything a run's job started, however it regrouped:
# its whole scope when it has one (WORK/scope names the unit, WORK/cgroup its directory),
# then its process group PID (the session leader, "" if unknown), then every process still
# tagged with the run (kill_run). Bounded; never fails.
stop_job() {
  local work=$1 pid=$2 run=$3 unit cg runner
  runner=$(cat "$work/runner" 2>/dev/null || true)
  unit=$(cat "$work/scope" 2>/dev/null || true)
  cg=$(cat "$work/cgroup" 2>/dev/null || true)
  if [ -n "$unit" ]; then stop_scope "$unit" "$cg"; fi
  if [ -n "$pid" ]; then stop_group "$pid"; fi
  kill_run "$run" "$runner"
}

# tagged_left RUN_ID: whether a process tagged with the run outlives a short wait (a
# watchdog's sampling `ps` or `sleep` ending with it is not a leftover).
tagged_left() {
  [ -n "$(run_pids "$1" || true)" ] || return 1
  sleep 0.3 || true
  [ -n "$(run_pids "$1" || true)" ]
}

# reap_job WORK PID RUN_ID: after the job's own command ended, stop whatever it left behind
# (a background loop outlives its leader, keeps the run's slot lock through its inherited
# fd, and burns the helper's CPU). Cheap when nothing is left.
reap_job() {
  local work=$1 pid=$2 run=$3 cg
  cg=$(cat "$work/cgroup" 2>/dev/null || true)
  if { [ -n "$cg" ] && [ -d "$cg" ] && scope_procs "$cg"; } ||
    { [ -n "$pid" ] && kill -0 -- "-$pid" 2>/dev/null; } || tagged_left "$run"; then
    printf 'goway-remote: the job of run %s left processes behind; stopping them\n' "$run" >&2 || true
    stop_job "$work" "$pid" "$run"
  fi
  return 0
}

# scope_argv WORK CMD...: leave CMD (argv, NUL separated) and a loader that execs it in WORK.
# systemd-run expands `$` and `%` in the arguments it is given (`$$` becomes `$`), so the
# user's command never goes through its command line.
scope_argv() {
  local work=$1
  shift
  printf '%s\0' "$@" >"$work/argv"
  cat >"$work/scope-exec.sh" <<'LOADER'
args=()
while IFS= read -r -d '' a; do args+=("$a"); done <"$1"
exec "${args[@]}"
LOADER
}

# mem_cgroup PID: the cgroup v2 directory of PID when it runs in goway's own
# scope (see job_scope), else nothing: only then is memory.peak the job's alone.
mem_cgroup() {
  local cg
  cg=$(sed -n 's/^0:://p' "/proc/$1/cgroup" 2>/dev/null | head -1 || true)
  case "$cg" in */goway-*.scope) [ -d "/sys/fs/cgroup$cg" ] && printf '/sys/fs/cgroup%s' "$cg" ;; esac
  return 0
}

# mem_pss SID: the summed proportional set size, in bytes, of the processes of session SID
# (nothing when no process exposes /proc/PID/smaps_rollup).
mem_pss() {
  local pids
  pids=$(ps -A -o sid= -o pid= 2>/dev/null | awk -v s="$1" '$1 == s {printf "%s/smaps_rollup ", $2}' || true)
  [ -n "$pids" ] || return 0
  # shellcheck disable=SC2086 # the list is words by construction
  { cd /proc 2>/dev/null && cat $pids 2>/dev/null || true; } | awk '/^Pss:/ {t += $2} END {if (t > 0) print t * 1024}'
  return 0
}

# mem_sample PID DIR: raise DIR/mempeak to the job's memory now and note in
# DIR/oom when the kernel's OOM killer has killed one of its processes. The
# peak is the scope's own memory.peak when the job has a scope (exact, kernel
# 5.19+), else the resident memory of the job's session, sampled (a short
# spike between two samples is missed, which is why the margin exists).
mem_sample() {
  local pid=$1 dir=$2 cg v="" old
  cg=$(mem_cgroup "$pid" || true)
  if [ -n "$cg" ] && [ -r "$cg/memory.peak" ]; then
    # Only the run's own scope: before the job moves into it, it still sits in the scope of
    # an outer goway job (a nested run), which must never be recorded for killing.
    if [ ! -e "$dir/cgroup" ] && [ "${cg##*/}" = "$(cat "$dir/scope" 2>/dev/null || true)" ]; then
      printf '%s' "$cg" >"$dir/cgroup" 2>/dev/null || true
    fi
    v=$(cat "$cg/memory.peak" 2>/dev/null || true)
    if awk '/^oom_kill / && $2 > 0 {f=1} END {exit !f}' "$cg/memory.events" 2>/dev/null; then : >"$dir/oom"; fi
  else
    # The job leads its session and process group; BSD ps has no session column, so macOS
    # sums the process group.
    local col=sid
    [ "$IS_DARWIN" = 1 ] && col=pgid
    # Linux: the proportional set size (shared pages split between their users), so parallel
    # rustc processes sharing the same libraries are not counted once each; the plain
    # resident size (which does) only where smaps_rollup is unreadable and on macOS.
    [ "$IS_DARWIN" != 1 ] && v=$(mem_pss "$pid" || true)
    case "$v" in "" | 0 | *[!0-9]*)
      v=$(ps -A -o "$col=" -o rss= 2>/dev/null | awk -v s="$pid" '$1 == s {t += $2} END {print t * 1024}' || true) ;;
    esac
  fi
  case "$v" in "" | *[!0-9]*) return 0 ;; esac
  old=$(cat "$dir/mempeak" 2>/dev/null || echo 0)
  case "$old" in "" | *[!0-9]*) old=0 ;; esac
  if [ "$v" -gt "$old" ]; then printf '%s' "$v" >"$dir/mempeak" 2>/dev/null || true; fi
  return 0
}

# job_scope RUN_ID [LIMITS [PRIORITY]]: set SCOPE to a wrapper that puts the job in its own
# transient systemd scope (so its memory is measured exactly and the whole job can be
# stopped), probed once with `true`; empty where there is no user manager (a WSL without
# systemd). LIMITS is TASKS:CPU_PERCENT:MEMORY_BYTES (the run's `limits:` word; an empty
# field is no cap, TASKS 0 lifts the process cap): the scope gets TasksMax, CPUQuota and
# MemoryMax, and a low CPUWeight when PRIORITY is low or owner, so one runaway job cannot
# take the helper down. A manager that refuses the caps still gets a plain scope.
job_scope() {
  local tasks cpu mem props=() id=$1 limits=${2:-} priority=${3:-}
  SCOPE=()
  [ "$IS_DARWIN" != 1 ] && [ -e /sys/fs/cgroup/cgroup.controllers ] && command -v systemd-run >/dev/null 2>&1 || return 0
  IFS=: read -r tasks cpu mem <<<"$limits"
  case "$tasks" in "" | *[!0-9]*) tasks=0 ;; esac
  case "$cpu" in *[!0-9]*) cpu="" ;; esac
  case "$mem" in *[!0-9]*) mem="" ;; esac
  if [ "$tasks" -gt 0 ]; then props+=(--property="TasksMax=$tasks"); fi
  if [ -n "$cpu" ] && [ "$cpu" -gt 0 ]; then props+=(--property="CPUQuota=${cpu}%"); fi
  if [ -n "$mem" ] && [ "$mem" -gt 0 ]; then props+=(--property="MemoryMax=$mem"); fi
  case "$priority" in low | owner) props+=(--property=CPUWeight=20) ;; esac
  local base=(systemd-run --user --scope --quiet --collect --property=TimeoutStopSec=5)
  if bounded_for 3 "${base[@]}" ${props[@]+"${props[@]}"} --unit="goway-probe-$id" true >/dev/null 2>&1; then
    SCOPE=("${base[@]}" ${props[@]+"${props[@]}"} --unit="goway-$id")
  elif [ ${#props[@]} -gt 0 ] && bounded_for 3 "${base[@]}" --unit="goway-probe-$id" true >/dev/null 2>&1; then
    printf 'goway-remote: the user manager refused the job limits; running the job in a plain scope\n' >&2 || true
    SCOPE=("${base[@]}" --unit="goway-$id")
  fi
  return 0
}

# job_tasks LIMITS: the process cap of the run's `limits:` word (0 = none), for the
# `ulimit -u` fallback of a job that has no scope.
job_tasks() {
  local tasks
  tasks=${1%%:*}
  case "$tasks" in "" | *[!0-9]*) tasks=0 ;; esac
  printf '%s' "$tasks"
}

# Kill the job's process group when the ssh session that started it dies
# (sshd does not signal commands without a pty, it orphans them). The job
# writes its pid (= its process group, it is a session leader) to PIDFILE.
# A client that vanished without the connection closing is lifeline's job.
watchdog() {
  local session=$1 pidfile=$2 pid="" memdir=${3:-}
  trap '' HUP PIPE
  while [ -z "$pid" ]; do
    kill -0 "$session" 2>/dev/null || return 0
    sleep 0.2
    pid=$(cat "$pidfile" 2>/dev/null || true)
  done
  while kill -0 "$session" 2>/dev/null && kill -0 "$pid" 2>/dev/null; do
    if [ -n "$memdir" ]; then mem_sample "$pid" "$memdir"; fi
    sleep 0.5
  done
  # The leader may be gone while its background processes run on: stop the whole job.
  if ! kill -0 "$session" 2>/dev/null && [ -n "$memdir" ]; then stop_job "$memdir" "$pid" "${memdir##*/}"; fi
}

# How long (seconds) lifeline waits for the client's next heartbeat byte. Long on
# purpose: an overloaded laptop sends its beats late, and silence alone is never
# proof that the client is gone (end of input is, and stops the job at once).
LIFELINE_TIMEOUT=${GOWAY_LIFELINE_TIMEOUT:-120}

# lifeline ROOT RUN_ID: the run's lifeline. The client keeps this call open
# and writes a byte to its stdin every few seconds. When stdin ends (the
# client died, even by SIGKILL, so its end of the pipe closed) or no byte
# arrives for LIFELINE_TIMEOUT seconds (laptop asleep, network gone), the
# run is stopped: its job's process group when the job started (bounded by
# stop_group), else the run's own shell. A run that already finished (its
# work dir is gone or marked done) is left alone. Stops within
# LIFELINE_TIMEOUT plus 5 seconds at the worst, at once on end of stdin.
lifeline() {
  local root work c="" pid runner rc=0 why began
  root=$(root_dir "$1"); work="$root/work/$2"
  case "$2" in *[!A-Za-z0-9-]* | "") die "lifeline: bad run id" ;; esac
  while :; do
    began=$SECONDS
    IFS= read -r -n 1 -t "$LIFELINE_TIMEOUT" c || rc=$?
    [ "$rc" = 0 ] || break
    [ -d "$work" ] && [ ! -e "$work/done" ] || return 0
  done
  [ -d "$work" ] && [ ! -e "$work/done" ] || return 0
  # A timeout means silence; anything else is end of input: the client's end
  # of the pipe closed, so it is certainly gone. bash 3.2 (macOS) returns 1
  # for both, so a failed read that waited the whole window counts as silence.
  if [ "$rc" -gt 128 ] || [ $((SECONDS - began)) -ge "$LIFELINE_TIMEOUT" ]; then
    why="its heartbeat was silent for ${LIFELINE_TIMEOUT}s (laptop asleep or the network gone)"
  else
    why="its lifeline connection closed (the client exited or lost its network)"
  fi
  printf '%s\n' "$why" >"$work/lost" 2>/dev/null || return 0
  pid=$(cat "$work/pid" 2>/dev/null || true)
  case "$pid" in "" | *[!0-9]*) pid="" ;; esac
  if [ -n "$pid" ]; then
    printf 'goway-remote: client of run %s is gone (%s); stopping its job\n' "$2" "$why" >&2
    stop_job "$work" "$pid" "$2"
    return 0
  fi
  runner=$(cat "$work/runner" 2>/dev/null || true)
  case "$runner" in "" | *[!0-9]*) return 0 ;; esac
  kill -TERM "$runner" 2>/dev/null || true
}

# path_find NAME ROOT: the absolute path of the executable NAME on a PATH directory, else
# fail. Only absolute directories outside ROOT (goway's own state, where the synced work
# tree lives) are searched: never the current directory, never a relative entry, so a
# repository cannot ship a look-alike. A symlink that leads into ROOT does not count.
path_find() {
  local d full real hit="" IFS=:
  set -f
  for d in $PATH; do
    case "$d" in /*) ;; *) continue ;; esac
    full=$(cd "$d" 2>/dev/null && pwd -P) || continue
    case "$full/" in "$2"/*) continue ;; esac
    [ -f "$full/$1" ] && [ -x "$full/$1" ] || continue
    real=$(readlink -f "$full/$1" 2>/dev/null || true)
    case "$real/" in "$2"/*) continue ;; esac
    hit="$full/$1"
    break
  done
  set +f
  [ -n "$hit" ] || return 1
  printf '%s' "$hit"
}

# resolve ROOT RUN_ID: for portable command translation. stdin holds candidate lines
# `tier;kind;name;args;check` (kind same|bare|tree; args and check comma-joined). The first
# tier with a usable candidate wins; two in one tier, or none, is doubt. Prints one line:
# `same` (the program as given is there), `ok;PROGRAM;ARGS` (PROGRAM absolute) or
# `none;WHY`. A candidate's check (arguments) must exit 0 within 15 seconds. The `tree`
# kind (a Windows convention) never matches here. Total: any trouble is `none`.
resolve() {
  local root rootc line tier kind name args check tiers t hits=0 prog="" pargs="" a n=0
  root=$(root_dir "$1")
  case "$2" in *[!A-Za-z0-9-]* | "") die "resolve: bad run id" ;; esac
  rootc=$(cd "$root" 2>/dev/null && pwd -P || printf '%s' "$root")
  local lines=()
  while IFS= read -r line && [ "$n" -lt 16 ]; do lines+=("$line"); n=$((n + 1)); done
  tiers=$(for line in ${lines[@]+"${lines[@]}"}; do printf '%s\n' "${line%%;*}"; done | { grep -E '^[0-9]+$' || true; } | sort -un)
  for t in $tiers; do
    hits=0; prog=""; pargs=""
    for line in ${lines[@]+"${lines[@]}"}; do
      IFS=';' read -r tier kind name args check <<<"$line"
      [ "$tier" = "$t" ] || continue
      case "$name" in "" | *[!A-Za-z0-9._+-]*) continue ;; esac
      case "$kind" in same | bare) ;; *) continue ;; esac
      a=$(path_find "$name" "$rootc") || continue
      if [ -n "$check" ]; then
        local words=()
        IFS=',' read -r -a words <<<"$check"
        bounded_for 15 "$a" "${words[@]}" >/dev/null 2>&1 </dev/null || continue
      fi
      hits=$((hits + 1))
      if [ "$kind" = same ]; then prog=same; else prog=$a; pargs=$args; fi
    done
    if [ "$hits" -gt 1 ]; then printf 'none;several candidates match\n'; return 0; fi
    if [ "$hits" -eq 1 ]; then
      case "$prog" in
        same) printf 'same\n' ;;
        *';'* | *','*) printf 'none;unsafe path\n' ;;
        *) printf 'ok;%s;%s\n' "$prog" "$pargs" ;;
      esac
      return 0
    fi
  done
  printf 'none;nothing found on PATH\n'
}

# discard ROOT RUN_ID: remove the synced work dir of a run that will not start (goway
# found the command has no certain equivalent on this host and re-picked another one).
# Best effort like the end of a run: it never fails; gc collects what it cannot remove.
discard() {
  local root
  root=$(root_dir "$1")
  case "$2" in *[!A-Za-z0-9-]* | "") die "discard: bad run id" ;; esac
  remove_work "$root/work/$2"
}

# envfile ROOT RUN_ID: store the run's --env values (NUL-separated on
# stdin, never in argv) in its work dir, readable by the owner only.
envfile() {
  local root work
  root=$(root_dir "$1"); work="$root/work/$2"
  case "$2" in *[!A-Za-z0-9-]* | "") die "envfile: bad run id" ;; esac
  [ -d "$work" ] || die "envfile: no work dir $work"
  cat >"$work/env"
  chmod 600 "$work/env"
}

# ---- GPU slots (goway run --needs gpu...) -----------------------------
#
# A run that needs a GPU holds a flock on one GPU "slot" while it runs, so
# concurrent GPU runs get different GPUs instead of sharing one by accident.
# Each GPU has PER slot files (gpu_jobs, default 1): $root/gpu/<vendor>-<index>.<n>.lock.
# The lock is on fd 5 of the run's shell: it is released when that shell ends,
# however it ends (exit, kill, ssh drop), and background helpers close fd 5.

# gpu_list: "vendor index" lines for the GPUs the driver tools list.
gpu_list() {
  local i card _rest
  if command -v nvidia-smi >/dev/null 2>&1; then
    while read -r i; do
      case "$i" in '' | *[!0-9]*) ;; *) printf 'nvidia %s\n' "$i" ;; esac
    done < <(bounded nvidia-smi --query-gpu=index --format=csv,noheader 2>/dev/null || true)
  fi
  if command -v rocm-smi >/dev/null 2>&1; then
    while IFS=, read -r card _rest; do
      case "$card" in card[0-9]*) printf 'amd %s\n' "${card#card}" ;; esac
    done < <(bounded rocm-smi --showproductname --csv 2>/dev/null || true)
  fi
}

# gpu_acquire ROOT PER: take one free GPU slot (waiting, with a note, while
# all are busy), then export CUDA_VISIBLE_DEVICES / ROCR_VISIBLE_DEVICES for
# it unless the user already set them. Holds the lock on fd 5.
gpu_acquire() {
  local dir="$1/gpu" per=$2 gpus=() line n k vendor idx f noted=0
  while read -r line; do [ -n "$line" ] && gpus+=("$line"); done < <(gpu_list)
  n=${#gpus[@]}
  if [ "$n" -eq 0 ]; then
    printf 'goway: warning: this run needs a GPU but no GPU tool lists one here; running without a GPU slot\n' >&2
    return 0
  fi
  mkdir -p "$dir"
  while :; do
    # Spread first: slot 0 of every GPU, then slot 1 of every GPU, ...
    for ((k = 0; k < per; k++)); do
      for line in "${gpus[@]}"; do
        vendor=${line% *}; idx=${line#* }
        f="$dir/$vendor-$idx.$k.lock"
        exec 5>"$f"
        if flock -n 5; then
          gpu_export "$vendor" "$idx" "$n"
          printf 'goway: using GPU %s (%s), slot %s\n' "$idx" "$vendor" "$k" >&2
          return 0
        fi
        exec 5>&-
      done
    done
    if [ "$noted" = 0 ]; then
      printf 'goway: all %s GPU(s) are busy (up to %s run(s) each); waiting for one\n' "$n" "$per" >&2
      noted=1
    fi
    sleep 1
  done
}

# gpu_export VENDOR INDEX COUNT: name the GPU for the framework environment
# variables that are not already set (a variable the user set stays theirs).
# With only one vendor on the host, both variables name it.
gpu_export() {
  local vendors
  vendors=$(gpu_list | cut -d' ' -f1 | sort -u | wc -l)
  if { [ "$1" = nvidia ] || [ "$vendors" -le 1 ]; } && [ -z "${CUDA_VISIBLE_DEVICES+x}" ]; then
    export CUDA_VISIBLE_DEVICES=$2
  fi
  if { [ "$1" = amd ] || [ "$vendors" -le 1 ]; } && [ -z "${ROCR_VISIBLE_DEVICES+x}" ]; then
    export ROCR_VISIBLE_DEVICES=$2
  fi
}

# ---- shard framework detection (goway run --shard) -------------------
#
# Before a shard's command runs, the program it names may turn out to be a
# GoogleTest or Catch2 v3 test binary (usually one built on this host during
# the run). The file is only ever READ, never executed: it must be a regular
# ELF, PE or Mach-O executable (magic bytes, so scripts never count) and contain
# EVERY marker string of a framework, found with a fixed-string search over
# a bounded prefix. Markers are flag and variable names the frameworks need
# to parse their own command line; unlike symbols they survive stripping.
SNIFF_CAP=$((256 * 1024 * 1024))
GTEST_MARKERS=(GTEST_SHARD_INDEX GTEST_TOTAL_SHARDS --gtest_list_tests --gtest_filter)
CATCH2_MARKERS=(--shard-count --shard-index --list-tests catch2-version)

# resolve_program NAME: the file the shell will execute for NAME (a path
# containing "/" is relative to the current directory, otherwise PATH is
# searched, files only), or nothing.
resolve_program() {
  case "$1" in
    */*) if [ -f "$1" ]; then printf '%s' "$1"; fi ;;
    *) type -P -- "$1" 2>/dev/null || true ;;
  esac
}

# sniff_binary FILE: print gtest, catch2 or none. Reads FILE, never runs it.
sniff_binary() {
  local f=$1 magic hits m all
  if [ -z "$f" ] || [ ! -f "$f" ] || [ ! -r "$f" ]; then printf 'none'; return 0; fi
  magic=$(head -c 4 <"$f" 2>/dev/null | od -An -tx1 | tr -d ' \n' || true)
  case "$magic" in
    # ELF, PE, and Mach-O (64- and 32-bit, either byte order, and fat binaries).
    7f454c46 | 4d5a* | cffaedfe | cefaedfe | feedfacf | feedface | cafebabe) ;;
    *) printf 'none'; return 0 ;;
  esac
  # One pass for all markers; the output is capped so a hostile file full
  # of markers cannot grow it.
  hits=$(
    head -c "$SNIFF_CAP" <"$f" 2>/dev/null \
      | LC_ALL=C grep -a -o -F -e "${GTEST_MARKERS[0]}" -e "${GTEST_MARKERS[1]}" -e "${GTEST_MARKERS[2]}" \
        -e "${GTEST_MARKERS[3]}" -e "${CATCH2_MARKERS[0]}" -e "${CATCH2_MARKERS[1]}" \
        -e "${CATCH2_MARKERS[2]}" -e "${CATCH2_MARKERS[3]}" 2>/dev/null \
      | head -c 65536 | sort -u || true
  )
  hits=$'\n'$hits$'\n'
  local found_gtest=1 found_catch2=1
  for m in "${GTEST_MARKERS[@]}"; do
    case "$hits" in *$'\n'"$m"$'\n'*) ;; *) found_gtest=0 ;; esac
  done
  for m in "${CATCH2_MARKERS[@]}"; do
    case "$hits" in *$'\n'"$m"$'\n'*) ;; *) found_catch2=0 ;; esac
  done
  all=$((found_gtest + found_catch2))
  # Both complete would be a binary that links both: ambiguous, so neither.
  if [ "$all" = 1 ] && [ "$found_gtest" = 1 ]; then
    printf 'gtest'
  elif [ "$all" = 1 ]; then
    printf 'catch2'
  else
    printf 'none'
  fi
}

# cap_parallelism: while the owner uses this host, builds and tests get half
# the cores (at least 1) through the usual variables, each only when the user
# has not set it (the job's own environment decides, MAKEFLAGS included).
cap_parallelism() {
  local half=$(($(cores) / 2))
  [ "$half" -ge 1 ] || half=1
  [ -n "${CARGO_BUILD_JOBS+x}" ] || export CARGO_BUILD_JOBS=$half
  [ -n "${MAKEFLAGS+x}" ] || export MAKEFLAGS="-j$half"
  [ -n "${CMAKE_BUILD_PARALLEL_LEVEL+x}" ] || export CMAKE_BUILD_PARALLEL_LEVEL=$half
  [ -n "${NEXTEST_TEST_THREADS+x}" ] || export NEXTEST_TEST_THREADS=$half
}

# ---- CMake's own interfaces (File API replies, a traced configure) ------
#
# doctor asks CMake what a project needs instead of reading CMakeLists.txt as
# text. Every run leaves a stateful File API query (client-goway) in the
# build directories of its slot tree, so a normal configure there writes
# replies. doctor reads the newest ones (cmake-replies), and doctor
# --configure runs one traced configure of a snapshot in the run's own work
# dir (cmake-configure), which gc covers like any work dir. Output is framed
# as "@@build LABEL", "@@file NAME SIZE" + SIZE bytes + newline, "@@end".
CMAKE_QUERY='{"requests":[{"kind":"codemodel","version":2},{"kind":"cache","version":2},{"kind":"cmakeFiles","version":1},{"kind":"toolchains","version":1}]}'
CMAKE_MAX_FILE=4194304
CMAKE_MAX_FILES=400
CMAKE_MAX_TOTAL=16777216

# cmake_query_dir DIR: write the File API query into the build directory DIR.
cmake_query_dir() {
  local q="$1/.cmake/api/v1/query/client-goway"
  [ -f "$q/query.json" ] && return 0
  mkdir -p "$q" 2>/dev/null && printf '%s\n' "$CMAKE_QUERY" >"$q/query.json" 2>/dev/null || true
  return 0
}

# cmake_queries TREE: leave the query in the build directories of a slot
# tree: build/ (made when absent) and cmake-build-*, each only when it holds
# a CMake cache or nothing (a directory with the project's own files is not
# a build directory).
cmake_queries() {
  local tree=$1 d own
  [ -f "$tree/CMakeLists.txt" ] || return 0
  [ -e "$tree/build" ] || mkdir "$tree/build" 2>/dev/null || true
  for d in "$tree/build" "$tree"/cmake-build-*; do
    if [ ! -d "$d" ] || [ -L "$d" ]; then continue; fi
    if [ ! -f "$d/CMakeCache.txt" ]; then
      own=$(ls -A "$d" 2>/dev/null | grep -v '^\.cmake$' | head -1 || true)
      [ -z "$own" ] || continue
    fi
    cmake_query_dir "$d"
  done
  return 0
}

# cmake_emit REPLY_DIR LABEL: the replies in REPLY_DIR, size-limited.
cmake_emit() {
  local f name size n=0 total=0
  printf '@@build %s\n' "$2"
  for f in "$1"/*.json; do
    if [ ! -f "$f" ] || [ -L "$f" ]; then continue; fi
    name=${f##*/}
    case "$name" in *[!A-Za-z0-9._-]*) continue ;; esac
    size=$(stat -c %s "$f" 2>/dev/null || echo 0)
    if [ "$size" -le 0 ] || [ "$size" -gt "$CMAKE_MAX_FILE" ]; then continue; fi
    n=$((n + 1)); total=$((total + size))
    if [ "$n" -gt "$CMAKE_MAX_FILES" ] || [ "$total" -gt "$CMAKE_MAX_TOTAL" ]; then break; fi
    printf '@@file %s %s\n' "$name" "$size"
    cat "$f"; printf '\n'
  done
}

# cmake_replies ROOT REPO_ID: the newest File API replies of the repository's slot trees.
cmake_replies() {
  local root cache tree d reply idx best="" best_t=0 t
  root=$(root_dir "$1")
  case "$2" in "" | *[!A-Za-z0-9._-]*) die "cmake-replies: bad repository id" ;; esac
  cache="$root/cache/$2"
  printf 'goway-cmake1\n'
  for tree in "$cache"/tree-*; do
    [ -d "$tree" ] || continue
    for d in "$tree/build" "$tree"/cmake-build-*; do
      reply="$d/.cmake/api/v1/reply"
      [ -d "$reply" ] || continue
      idx=$(ls -1 "$reply"/index-*.json 2>/dev/null | sort | tail -1 || true)
      [ -n "$idx" ] || continue
      t=$(stat -c %Y "$idx" 2>/dev/null || echo 0)
      if [ -z "$best" ] || [ "$t" -gt "$best_t" ]; then best=$d; best_t=$t; fi
    done
  done
  if [ -n "$best" ]; then
    cmake_emit "$best/.cmake/api/v1/reply" "${best#"$cache"/}"
    printf '@@end\n'
  fi
  return 0
}

# cmake_configure ROOT RUN_ID SECONDS: one traced configure of the snapshot
# synced into work/RUN_ID/tree, in that work dir (labelled, locked, removed
# afterwards, collected by gc if this dies). Prints the exit code, the tail of
# stderr, the filtered json-v1 trace and the File API replies.
cmake_configure() {
  local root work secs=${3:-300} rc=0 src build
  root=$(root_dir "$1"); work="$root/work/$2"
  case "$2" in *[!A-Za-z0-9-]* | "") die "cmake-configure: bad run id" ;; esac
  case "$secs" in "" | *[!0-9]*) die "cmake-configure: bad seconds" ;; esac
  [ "$secs" -le 900 ] || secs=900
  [ -d "$work/tree" ] || die "cmake-configure: no work dir at $work (was it synced?)"
  printf 'goway-cmake1\n'
  if ! command -v cmake >/dev/null 2>&1; then printf '@@rc 127\n@@end\n'; return 0; fi
  mark_root "$root"
  exec 9>"$work/lock"
  flock -x 9
  src="$work/cfg-src"; build="$work/cfg-build"
  rm -rf "$src" "$build"
  cp -a --reflink=auto "$work/tree" "$src"
  cmake_query_dir "$build"
  # A configure may download (FetchContent, CPM): into this scratch dir only.
  (cd "$src" && bounded_for "$secs" nice -n 19 cmake -S . -B "$build" \
    --trace-expand --trace-format=json-v1 --trace-redirect="$work/trace.json" \
    >"$work/cfg.out" 2>"$work/cfg.err" </dev/null) || rc=$?
  printf '@@rc %s\n' "$rc"
  for f in stderr:cfg.err trace.jsonl:trace.json; do
    local name=${f%%:*} file="$work/${f#*:}" body
    body=$(mktemp "$work/body.XXXXXX")
    case "$name" in
      stderr) tail -c 65536 "$file" >"$body" 2>/dev/null || true ;;
      *) grep -E '^\{"version"|"cmd":"(cmake_minimum_required|project|find_package|FetchContent_Declare|FetchContent_MakeAvailable|FetchContent_Populate|pkg_check_modules|pkg_search_module|CPMAddPackage|CPMFindPackage|CPMDeclarePackage|add_subdirectory)"' "$file" 2>/dev/null | head -c "$CMAKE_MAX_FILE" >"$body" || true ;;
    esac
    printf '@@file %s %s\n' "$name" "$(stat -c %s "$body")"
    cat "$body"; printf '\n'
    rm -f "$body"
  done
  if [ -d "$build/.cmake/api/v1/reply" ]; then cmake_emit "$build/.cmake/api/v1/reply" configure; fi
  printf '@@end\n'
  remove_work "$work"
  return 0
}

# keep_awake: set AWAKE to the words that wrap a job in a sleep inhibitor,
# empty when the host has none that works. The inhibitor is held by the
# wrapper process, so it lasts exactly as long as the job and is released
# however the job ends (exit, signal, the watchdog's kill of the group).
# Linux asks logind (probed once with `true`: a WSL without systemd, or a
# session logind refuses, simply gets none); macOS uses caffeinate.
keep_awake() {
  AWAKE=()
  case "$(uname -s 2>/dev/null)" in
    Darwin)
      if command -v caffeinate >/dev/null 2>&1; then AWAKE=(caffeinate -i -m -s); fi ;;
    Linux)
      if command -v systemd-inhibit >/dev/null 2>&1 &&
        bounded systemd-inhibit --what=sleep:idle --who=goway --why="goway job" --mode=block true >/dev/null 2>&1; then
        AWAKE=(systemd-inhibit --what=sleep:idle --who=goway --why="goway job" --mode=block)
      fi ;;
  esac
}

# What a job without a scope runs under `setsid bash -c` (dash has no `ulimit -u`): record its pid ($0), cap the
# user's processes at the count now plus $1 (`ulimit -u` is per user, not per job; 0 = no cap) as the fallback for TasksMax, then exec it.
JOB_LAUNCH='echo $$ >"$0"; if [ "$1" -gt 0 ] 2>/dev/null; then ulimit -u $(($1 + $(ps -u "$(id -u)" -o pid= 2>/dev/null | wc -l))) 2>/dev/null || true; fi; shift; exec "$@"'

# launch_job CMD...: start the job as run does (own session, pid recorded
# for the watchdog, polite priority). JOB_PID and JOB_NICER are run's.
launch_job() {
  setsid bash -c "$JOB_LAUNCH" "$JOB_PID" "$JOB_TASKS" ${JOB_NICER[@]+"${JOB_NICER[@]}"} "$@"
}

# catch2_rejected ERRFILE RC: whether Catch2 refused the shard flags before
# running anything: a non-zero exit whose stderr STARTS with Catch2's
# command-line error and names one of the two flags. Anything less certain
# is not a rejection.
catch2_rejected() {
  local first
  [ "$2" -ne 0 ] || return 1
  first=$(grep -m1 -v '^[[:space:]]*$' "$1" 2>/dev/null || true)
  [ "$first" = "Error(s) in input:" ] || return 1
  grep -qE 'Unrecognised token: --shard-(count|index)' "$1" 2>/dev/null
}

# catch2_attempt ATTEMPT IDX CNT ERRFILE CMD...: run CMD once. Attempt 1
# adds Catch2's shard flags, attempt 2 (the one rerun) adds none. Stderr
# still reaches the caller live; a copy of its first 64 KiB or so goes to
# ERRFILE (a fifo and a size-limited tee, so nothing unbounded is written).
# Sets ATTEMPT_REJECTED=1 when the attempt was certainly rejected.
catch2_attempt() {
  local attempt=$1 idx=$2 cnt=$3 errfile=$4 rc=0 extra=() fifo tpid
  shift 4
  if [ "$attempt" = 1 ]; then extra=(--shard-count "$cnt" --shard-index $((idx - 1))); fi
  ATTEMPT_REJECTED=0
  fifo="$errfile.fifo"
  rm -f "$errfile" "$fifo"
  mkfifo "$fifo"
  # Not the run's lock fds (7, 9): a lingering tee must never hold a slot.
  (trap '' XFSZ; ulimit -f 128; exec tee "$errfile" <"$fifo" >&2) 5>&- 7>&- 9>&- &
  tpid=$!
  launch_job "$@" ${extra[@]+"${extra[@]}"} 2>"$fifo" || rc=$?
  wait "$tpid" || true
  rm -f "$fifo"
  if catch2_rejected "$errfile" "$rc"; then ATTEMPT_REJECTED=1; fi
  return "$rc"
}

# shard_run SPEC WORK CMD...: run a shard whose framework goway may detect.
# SPEC is "index:count:nonce". Prints goway's notes on stderr and, last, one
# result line "\001goway-shard-result:NONCE key=value..." that the client
# strips from the stream and records. Returns the command's exit code.
# The rerun after a certain Catch2 rejection happens at most once, by
# construction: attempt 1 and attempt 2 are two explicit calls.
shard_run() {
  local spec=$1 work=$2 idx cnt nonce file kind rc=0 rc2=0 attempts rerun=0 rejected=0
  local flagged=0 dup=0 status="$2/gtest-shard-status" err="$2/shard-stderr" a
  shift 2
  IFS=: read -r idx cnt nonce <<<"$spec"
  file=$(resolve_program "$1")
  kind=$(sniff_binary "$file")
  for a in "$@"; do
    case "$a" in
      --shard-count | --shard-count=* | --shard-index | --shard-index=*) kind=none ;;
    esac
  done
  if [ "$kind" = gtest ] && { [ -n "${GTEST_TOTAL_SHARDS:-}" ] || [ -n "${GTEST_SHARD_INDEX:-}" ]; }; then
    kind=none
  fi
  case "$kind" in
    gtest)
      printf 'goway: shard %s/%s: %s is a GoogleTest binary (detected by reading it); sharding with GTEST_TOTAL_SHARDS and GTEST_SHARD_INDEX\n' "$idx" "$cnt" "$1" >&2
      rm -f "$status"
      export GTEST_TOTAL_SHARDS=$cnt GTEST_SHARD_INDEX=$((idx - 1)) GTEST_SHARD_STATUS_FILE=$status
      launch_job "$@" || rc=$?
      attempts=$rc
      # GoogleTest creates the status file when it applies sharding. List
      # and help runs return before that point, so they are not judged.
      local judged=1
      for a in "$@"; do
        case "$a" in --gtest_list_tests | --help | -h | --gtest_help) judged=0 ;; esac
      done
      if [ "$judged" = 1 ] && [ "$cnt" -gt 1 ] && [ ! -e "$status" ]; then
        dup=1
        printf 'goway: warning: %s: GoogleTest did not apply sharding (no status file), so this shard ran the whole suite; results stand but work was duplicated\n' "$1" >&2
      fi
      ;;
    catch2)
      printf 'goway: shard %s/%s: %s is a Catch2 v3 binary (detected by reading it); sharding with --shard-count and --shard-index\n' "$idx" "$cnt" "$1" >&2
      catch2_attempt 1 "$idx" "$cnt" "$err" "$@" || rc=$?
      attempts=$rc
      if [ "$ATTEMPT_REJECTED" = 1 ]; then
        printf 'goway: warning: %s rejected the shard flags before running any test; rerunning this shard once without them (it runs the whole suite)\n' "$1" >&2
        rerun=1
        catch2_attempt 2 "$idx" "$cnt" "$err" "$@" || rc2=$?
        rc=$rc2
        attempts="$attempts,$rc2"
        if [ "$ATTEMPT_REJECTED" = 1 ]; then
          rejected=1
          printf 'goway: warning: %s was rejected again after the rerun; not trying again\n' "$1" >&2
        fi
      elif [ "$rc" -ne 0 ] && grep -qE 'Unrecognised token|Error\(s\) in input' "$err" 2>/dev/null; then
        flagged=1
        printf 'goway: warning: %s failed with a command-line error that may be about the shard flags; not certain, so it is not rerun\n' "$1" >&2
      fi
      rm -f "$err"
      ;;
    *)
      launch_job "$@" || rc=$?
      attempts=$rc
      ;;
  esac
  printf '\001goway-shard-result:%s detected=%s attempts=%s rerun=%s rejected=%s flagged=%s duplicated=%s\n' \
    "$nonce" "$kind" "$attempts" "$rerun" "$rejected" "$flagged" "$dup" >&2
  return "$rc"
}

# slots_busy_note CACHE SLOTS: how long the oldest holder of a build slot has run (a slot
# lock's mtime is stamped when a run takes it), as `oldest holder has run 12m 3s`.
slots_busy_note() {
  local k m now oldest=0 age
  now=$(date +%s)
  for ((k = 0; k < $2; k++)); do
    m=$(stat -c %Y "$1/target-$k.lock" 2>/dev/null || true)
    case "$m" in "" | *[!0-9]*) continue ;; esac
    age=$((now - m))
    [ "$age" -gt "$oldest" ] && oldest=$age
  done
  printf 'oldest holder has run %sm %ss' $((oldest / 60)) $((oldest % 60))
}

# run ROOT RUN_ID REPO_ID KEEP SLOTS CACHE_META_B64 TTLS PRIORITY KEEP_IGNORED
#     KEEP_B64 -- CMD...
# The work dir was created by receive (a hard-link snapshot of the seed).
# TTLS is "cache:orphan:kept" in seconds, for the automatic gc afterwards.
# Take a free slot (preferring the one this worktree used last), update the
# slot's persistent tree in place from the snapshot (sync_slot), run CMD there
# in its own process group with stdio passed through, clean up, and exit with
# CMD's status (128+N when killed by signal N). With KEEP=1 the finished
# tree is copied to work/<run-id>/tree for inspection; the slot stays usable.
run() {
  local root work cache slot="" k rc=0 wd rundir order=() aff="" seedkey
  root=$(root_dir "$1"); work="$root/work/$2"; cache="$root/cache/$3"
  local root_arg=$1 run_id=$2 repo_id=$3 keep=$4 slots=$5 cache_meta=$6
  local ttls=$7 priority=$8 keepignored=$9 keepb64=${10} nicer=() cc_launcher=""
  shift 10
  # Optional words before "--": shard-detect:INDEX:COUNT:NONCE asks for
  # framework detection of the command's program (see shard_run).
  local limits="" detect="" gpu_per="" slot_wait=300 verify="" fresh=0 attempt=1 level=changed room=""
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    case "$1" in
      shard-detect:[0-9]*:[0-9]*:[A-Za-z0-9]*) detect=${1#shard-detect:} ;;
      gpu-slots:[0-9]*) gpu_per=${1#gpu-slots:} ;;
      slot-wait:[0-9]*) slot_wait=${1#slot-wait:} ;;
      limits:[0-9]*:*:*) limits=${1#limits:} ;;
      room:[0-9]*:[0-9]*) room=${1#room:} ;;
      verify:[12]:changed | verify:[12]:all | verify:[12]:changed:fresh | verify:[12]:all:fresh)
        # The attempt number is goway's explicit argument, never read from
        # the environment or from anything the helper reports.
        verify=1; attempt=${1#verify:}; attempt=${attempt%%:*}
        level=${1#verify:?:}; level=${level%%:*}
        case "$1" in *:fresh) fresh=1 ;; esac ;;
      *) die "run: unknown option $1" ;;
    esac
    shift
  done
  # TTLS is "cache:orphan:kept[:max_disk:min_free:cache_size]" (bytes).
  local t_cache t_orphan t_kept t_max t_minfree t_csize
  IFS=: read -r t_cache t_orphan t_kept t_max t_minfree t_csize <<<"$ttls"
  case "$detect" in *[!A-Za-z0-9:]*) die "run: bad shard-detect" ;; esac
  case "$gpu_per" in *[!0-9]*) die "run: bad gpu-slots" ;; esac
  case "$slot_wait" in *[!0-9]*) die "run: bad slot-wait" ;; esac
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "run: no command"
  [ -d "$work/tree" ] || die "run: no work dir at $work (was it synced?)"

  mark_root "$root"
  mkdir -p "$cache"
  exec 9>"$work/lock"
  flock -x 9
  # The client's lifeline stops this shell with SIGTERM while it is still
  # preparing (before the job exists): clean up and go.
  # The lock protects the dir from here on; the starting-run markers are done.
  rm -f "$work/born" "$work/creator"
  printf '%s\n' "$$" >"$work/runner"
  trap 'remove_work "$work"; exit 143' TERM
  if [ -e "$work/lost" ]; then lost_note "$work"; remove_work "$work"; exit 143; fi
  # What the last automatic disk-budget eviction freed (it ran detached).
  if [ -s "$root/evicted.log" ]; then
    cat "$root/evicted.log" >&2 2>/dev/null || true
    rm -f "$root/evicted.log"
  fi

  [ -f "$cache/meta.json" ] || printf '%s' "$cache_meta" | base64 -d >"$cache/meta.json"
  touch "$cache/meta.json"

  # Settings that already exist win over goway's defaults: first the
  # remote environment and the usual per-user tool directories, then the
  # user's --env values; goway only fills in what is still unset.
  # Scratch files (compilers, test harnesses, build scripts) go under the
  # run's own work dir, never to the helper's /tmp: that may be a small
  # tmpfs, or mounted noexec so that build scripts cannot run. Only an
  # explicit --env TMPDIR=... wins; the helper's own TMPDIR does not.
  unset TMPDIR
  user_tool_path
  if [ -f "$work/env" ]; then
    while IFS= read -r -d '' kv; do export "$kv"; done <"$work/env"
    rm -f "$work/env"
  fi
  if [ -z "${TMPDIR:-}" ]; then
    mkdir -p "$work/tmp"
    export TMPDIR="$work/tmp"
  fi

  # A GPU run holds one GPU slot until this shell ends (fd 5). It waits for
  # the GPU before taking a build slot, so a queue for GPUs never pins the
  # build slots that CPU-only runs need.
  if [ -n "$gpu_per" ] && [ "$gpu_per" -ge 1 ]; then gpu_acquire "$root" "$gpu_per"; fi

  # A slot is a persistent build tree (tree-k) plus its cargo target dir
  # (target-k), held by target-k.lock. Prefer the slot this worktree used
  # last, so its tree is already close to the snapshot.
  seedkey=$(cat "$work/seed" 2>/dev/null || true)
  seedkey=${seedkey##*/}
  case "$seedkey" in "" | *[!A-Za-z0-9._-]*) ;; *) aff="$cache/affinity-$seedkey" ;; esac
  k=$(cat "$aff" 2>/dev/null || true)
  case "$k" in "" | *[!0-9]*) ;; *) [ "$k" -lt "$slots" ] && order+=("$k") ;; esac
  for ((k = 0; k < slots; k++)); do
    [ "${order[0]:-}" = "$k" ] || order+=("$k")
  done
  for k in "${order[@]}"; do
    # gc may have removed an expired cache dir just now: re-create it and
    # accept the lock only if it is the file the dir currently has.
    mkdir -p "$cache"
    exec 7>"$cache/target-$k.lock"
    if flock -n 7 && same_fd "$cache/target-$k.lock" 7; then slot=$k; break; fi
    exec 7>&-
  done
  if [ -z "$slot" ]; then
    # Every slot is busy: wait for the first to free, at most the run's --wait.
    printf 'goway: all %s build slots busy (%s); waiting up to %ss for one\n' "$slots" "$(slots_busy_note "$cache" "$slots")" "$slot_wait" >&2
    wait_until=$((SECONDS + slot_wait))
    while [ -z "$slot" ]; do
      for k in "${order[@]}"; do
        mkdir -p "$cache"
        exec 7>"$cache/target-$k.lock"
        if flock -n 7 && same_fd "$cache/target-$k.lock" 7; then slot=$k; break; fi
        exec 7>&-
      done
      [ -z "$slot" ] || break
      if [ "$SECONDS" -ge "$wait_until" ]; then
        printf 'goway: no build slot freed within %ss: all %s build slots busy (%s); raise --wait or try another host\n' "$slot_wait" "$slots" "$(slots_busy_note "$cache" "$slots")" >&2
        remove_work "$work"
        exit 125
      fi
      sleep 1
    done
  fi
  [ -f "$cache/meta.json" ] || printf '%s' "$cache_meta" | base64 -d >"$cache/meta.json"
  touch "$cache/meta.json"
  # The slot's last use, for least-recently-used eviction (evict_slot).
  touch "$cache/target-$slot.lock"
  [ -z "$aff" ] || printf '%s' "$slot" >"$aff"
  # Builds bake absolute source paths into binaries (CARGO_MANIFEST_DIR,
  # file!()), and cargo reuses them when only the workspace moved. So a
  # slot's binaries always run against a tree at the same path: tree-<slot>.
  rundir="$cache/tree-$slot"
  # A fresh copy (the second attempt, or a distrusted repository): no tree,
  # target dir or seed of an earlier run is trusted.
  if [ $fresh = 1 ] || [ "$attempt" = 2 ]; then
    slot_wipe "$slot" "$cache"
    printf 'goway: building slot %s from scratch (attempt %s)\n' "$slot" "$attempt" >&2
  fi
  # Nothing this run uses may be evicted to make room for it: its work dir,
  # the seed it was synced from, and its cache (see evict).
  GC_PROTECT="|$work|$root/seed/$(cat "$work/seed" 2>/dev/null || true)|$cache|"
  make_room "$root" "$cache" "$slot" "$repo_id" "$room" "$t_max"
  # Only making room protects them; the gc after the run may take what it left.
  GC_PROTECT=""
  sync_slot "$work/tree" "$rundir" "$work" "$keepignored" "$keepb64" "$(cat "$work/seed" 2>/dev/null || true)" "$cache/target-$slot"
  # The snapshot has done its job; its links hold no data of their own.
  rm -rf "$work/tree"
  # Copy integrity, before the command may start: goway compares what this
  # sync wrote (everything, when distrusted) with the laptop's files.
  if [ -n "$verify" ]; then
    if [ "$level" = all ]; then
      verify_gate "$work" 1 "$rundir" "$work/all.reg" "$work/all.lnk" || verify_failed 1
    else
      verify_gate "$work" 1 "$rundir" "$work/written.reg" "$work/written.lnk" || verify_failed 1
    fi
  fi
  # What the tree looked like (ctime, size) when the command started: a file
  # whose record changes was changed by the command, not by the copy.
  cmake_queries "$rundir"
  tree_stamps "$rundir" >"$work/stamps.before"
  if [ -z "${CARGO_TARGET_DIR:-}" ]; then
    export CARGO_TARGET_DIR="$cache/target-$slot"
  fi
  # Compiler caches: sccache (or ccache) with a per-repository cache dir.
  # Rust uses sccache as RUSTC_WRAPPER; C and C++ builds through CMake use
  # whichever is installed as the compiler launcher. Everything is set only
  # when still unset: the user's and the project's choices always win (a
  # RUSTC_WRAPPER of the user's, even an empty one, leaves sccache alone).
  if [ -z "${RUSTC_WRAPPER+set}" ] && command -v sccache >/dev/null 2>&1; then
    export SCCACHE_DIR="${SCCACHE_DIR:-$cache/sccache}"
    # A unix socket in the owner-only cache dir: no TCP port another user
    # on the host could reach or squat (unless the user chose an endpoint).
    if [ -z "${SCCACHE_SERVER_PORT:-}" ] && [ -z "${SCCACHE_SERVER_UDS:-}" ]; then
      export SCCACHE_SERVER_UDS=$(sccache_socket "$cache")
    fi
    # sccache's server is the one process allowed to outlive a run (it
    # keeps the cache warm); make it leave soon after the last build.
    export SCCACHE_IDLE_TIMEOUT="${SCCACHE_IDLE_TIMEOUT:-300}"
    # The server outlives this run, so it must not be started with the run's
    # TMPDIR: that directory is removed when the run ends (the server's later
    # temp files then fail), and a deep one overflows the start-up socket's
    # unix address ("path must be shorter than SUN_LEN"). Start it from a
    # stable directory, a short private one when the cache path is long.
    sc_tmp="$cache/sccache-tmp"
    if [ "${#sc_tmp}" -ge 80 ]; then sc_tmp=$(short_private_dir) || sc_tmp=; fi
    if [ -n "$sc_tmp" ]; then
      mkdir -p "$sc_tmp" 2>/dev/null
      sccache_heal "$cache" "$sc_tmp"
      TMPDIR=$sc_tmp sccache --start-server >/dev/null 2>&1 || true
      printf '%s\n' "$sc_tmp" >"$cache/sccache.tmpdir" 2>/dev/null || true
      export RUSTC_WRAPPER=sccache
      cc_launcher=sccache
    else
      # No stable directory for the server: a build would start it under the
      # run's TMPDIR, which is removed with the run. Build without sccache.
      unset SCCACHE_DIR SCCACHE_SERVER_UDS SCCACHE_IDLE_TIMEOUT
    fi
  elif command -v ccache >/dev/null 2>&1; then
    export CCACHE_DIR="${CCACHE_DIR:-$cache/ccache}"
    cc_launcher=ccache
  fi
  # CMake 3.17+ reads these from the environment; "unset" (not "empty")
  # decides, so CMAKE_CXX_COMPILER_LAUNCHER= switches the launcher off.
  if [ -n "$cc_launcher" ]; then
    export CMAKE_C_COMPILER_LAUNCHER="${CMAKE_C_COMPILER_LAUNCHER-$cc_launcher}"
    export CMAKE_CXX_COMPILER_LAUNCHER="${CMAKE_CXX_COMPILER_LAUNCHER-$cc_launcher}"
  fi
  # CPM.cmake downloads go to one directory shared by every slot of the
  # repository. (FetchContent's FETCHCONTENT_BASE_DIR is a cmake variable,
  # never injected into the user's command; see docs/usage.md.)
  export CPM_SOURCE_CACHE="${CPM_SOURCE_CACHE:-$cache/cpm}"
  # Compiler caches stay under a size cap unless the user chose one. ccache
  # reads bare numbers as GB and sccache needs a suffix, so both get MB.
  if [ -n "$t_csize" ] && [ "$t_csize" -gt 0 ] 2>/dev/null; then
    export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-$((t_csize / 1048576))M}"
    export CCACHE_MAXSIZE="${CCACHE_MAXSIZE:-$((t_csize / 1048576))M}"
  fi
  export GOWAY=1 GOWAY_RUN_ID="$run_id" GOWAY_HOST
  GOWAY_HOST=$(uname -n)

  # sshd hangs up the session's shell when the client goes away; survive
  # it (a handler, not an ignore, so the job keeps default dispositions)
  # long enough for the watchdog to stop the job and for cleanup to run.
  trap 'hangup=1' HUP PIPE
  trap - TERM
  if [ -e "$work/lost" ]; then lost_note "$work"; remove_work "$work"; exit 143; fi
  cd "$rundir"
  : >"$work/pid"
  # The watchdog must not inherit the lock fds, or a lingering `sleep`
  # would keep this run's slot and work dir locked after it ends.
  watchdog "$PPID" "$work/pid" "$work" </dev/null >/dev/null 2>&1 5>&- 7>&- 9>&- &
  wd=$!
  # Out of the job table, so bash 3.2 (macOS) never prints "Terminated" for it.
  disown "$wd" 2>/dev/null || true
  # Foreground (not `&`): background jobs of a non-interactive shell start
  # with SIGINT and SIGQUIT ignored, and the command must not inherit that.
  # A polite guest on someone's laptop: low CPU and idle-class I/O; extra
  # polite (nice 19, half the cores for builds) while the owner is using it.
  case "$priority" in
    low | owner)
      if command -v nice >/dev/null 2>&1; then nicer+=(nice -n "$([ "$priority" = owner ] && echo 19 || echo 10)"); fi
      if command -v ionice >/dev/null 2>&1; then nicer+=(ionice -c 3); fi ;;
  esac
  if [ "$priority" = owner ]; then cap_parallelism; fi
  # Outermost, so the inhibitor wraps the niceness wrappers and the job.
  keep_awake
  nicer=(${AWAKE[@]+"${AWAKE[@]}"} ${nicer[@]+"${nicer[@]}"})
  SCOPE=()
  if [ -z "$detect" ]; then job_scope "$run_id" "$limits" "$priority"; fi
  JOB_TASKS=$(job_tasks "$limits")
  if [ -n "$detect" ]; then
    JOB_PID="$work/pid"
    JOB_NICER=(${nicer[@]+"${nicer[@]}"})
    shard_run "$detect" "$work" "$@" || rc=$?
  else
    if [ ${#SCOPE[@]} -gt 0 ]; then
      scope_argv "$work" ${nicer[@]+"${nicer[@]}"} "$@"
      printf 'goway-%s.scope' "$run_id" >"$work/scope"
      setsid sh -c 'echo $$ >"$0"; exec "$@"' "$work/pid" "${SCOPE[@]}" bash "$work/scope-exec.sh" "$work/argv" || rc=$?
    else
      setsid bash -c "$JOB_LAUNCH" "$work/pid" "$JOB_TASKS" ${nicer[@]+"${nicer[@]}"} "$@" || rc=$?
    fi
  fi
  # The watchdog's own children (its sleep) carry the run's tag: end them with it.
  pkill -P "$wd" 2>/dev/null || true
  kill "$wd" 2>/dev/null || true
  reap_job "$work" "$(cat "$work/pid" 2>/dev/null || true)" "$run_id"
  : >"$work/done"
  cd "$root"
  memory_report "$root" "$repo_id" "$work" "$rc"
  lost_note "$work"
  # A failed command: before blaming the code, goway compares every synced
  # file the command did not itself change with the laptop's.
  if [ -n "$verify" ] && [ "$rc" -ne 0 ] && [ ! -e "$work/lost" ] && kill -0 "$PPID" 2>/dev/null; then
    tree_stamps "$rundir" >"$work/stamps.after"
    comm -z -12 "$work/stamps.before" "$work/stamps.after" | sed -z "s/^\\([^$SOH]*$SOH\\)\\{2\\}//" |
      sort -z >"$work/untouched"
    comm -z -12 "$work/all.reg" "$work/untouched" >"$work/after.reg"
    comm -z -12 "$work/all.lnk" "$work/untouched" >"$work/after.lnk"
    verify_gate "$work" 2 "$rundir" "$work/after.reg" "$work/after.lnk" || verify_failed 2
  fi
  if [ "$rc" -ne 0 ] && [ -n "$ttls" ]; then disk_full_note "$root" "$cache" "$slot" "$repo_id" "$room" "$t_max" "$t_minfree"; fi
  if [ "$keep" = 1 ]; then cp -a --reflink=auto "$rundir" "$work/tree"; fi
  if [ "$keep" != 1 ]; then remove_work "$work"; fi
  # Cheap automatic gc of expired entries, detached so it never delays
  # the exit (and never holds the ssh session open).
  if [ -n "$ttls" ]; then
    # At most one automatic gc per root (gc.lock); it only ever removes
    # files and never starts a goway run.
    (trap '' HUP; footprint_record "$root" "$repo_id" "$(footprint_measure "$cache" "$slot")"
     flock -n 8 || exit 0
     gc "$root_arg" "$(date +%s)" "$t_cache" "$t_orphan" "$t_kept" apply "" "" "$t_max" "$t_minfree" log) \
      8>"$root/gc.lock" </dev/null >/dev/null 2>&1 5>&- 7>&- 9>&- &
  fi
  exit "$rc"
}

# Run "$@" for at most 10 seconds when timeout exists (a hung driver tool
# must never hang a probe).
bounded() {
  bounded_for 10 "$@"
}

# bounded_for SECONDS cmd...: like bounded with its own limit. A Windows program that WSL
# interop cannot run (interop disabled) hangs for about 10 seconds, so those calls use 3.
bounded_for() {
  local secs=$1
  shift
  if command -v timeout >/dev/null 2>&1; then timeout "$secs" "$@"; else "$@"; fi
}

# static_facts: facts that change rarely (GPUs, CPU features, KVM, Docker,
# WSL), as key=value lines; "static=1" marks that they were probed. GPU
# lines are "gpu.N=vendor|name|mem_mib|driver|cuda". Nothing here executes
# anything but the vendor query tools, docker info and powershell.exe (WSL
# only, to list the video adapters Windows has).
static_facts() {
  local n=0 pat cuda="" flags="" f kvm=0 docker=0 wsl=0 win="" interop="" adm="" winhw="" nvcc=0 swap=""
  printf 'static=1\n'
  if command -v nvidia-smi >/dev/null 2>&1; then
    cuda=$(bounded nvidia-smi 2>/dev/null | grep -o 'CUDA Version: [0-9.]*' | head -1 | cut -d' ' -f3 || true)
    while IFS=, read -r name mem drv; do
      [ -n "$name" ] || continue
      name=$(printf '%s' "$name" | tr -d '|' | sed 's/^ *//;s/ *$//')
      mem=$(printf '%s' "$mem" | tr -dc '0-9')
      drv=$(printf '%s' "$drv" | tr -d ' ')
      printf 'gpu.%s=nvidia|%s|%s|%s|%s\n' "$n" "$name" "$mem" "$drv" "$cuda"
      n=$((n + 1))
    done < <(bounded nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits 2>/dev/null || true)
  fi
  if command -v rocm-smi >/dev/null 2>&1; then
    while IFS=, read -r card series _; do
      case "$card" in card[0-9]*) ;; *) continue ;; esac
      series=$(printf '%s' "$series" | tr -d '|' | sed 's/^ *//;s/ *$//')
      printf 'gpu.%s=amd|%s|||\n' "$n" "${series:-AMD GPU}"
      n=$((n + 1))
    done < <(bounded rocm-smi --showproductname --csv 2>/dev/null || true)
  fi
  if [ -r /proc/cpuinfo ]; then
    # aarch64 reports NEON as asimd.
    for f in avx2 avx512f neon; do
      pat=$f; [ "$f" = neon ] && pat='(neon|asimd)'
      if grep -m1 -E "^(flags|Features)[[:space:]]*:.*[[:space:]]$pat([[:space:]]|\$)" /proc/cpuinfo >/dev/null 2>&1; then
        flags="$flags${flags:+,}$f"
      fi
    done
  fi
  printf 'cpu_flags=%s\n' "$flags"
  if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then kvm=1; fi
  printf 'kvm=%s\n' "$kvm"
  if command -v docker >/dev/null 2>&1 && bounded docker info >/dev/null 2>&1; then docker=1; fi
  printf 'docker=%s\n' "$docker"
  if grep -qi microsoft /proc/version 2>/dev/null; then
    wsl=1
    # interop: whether Windows programs run from this distro, and with which token. They run
    # with the token of whatever started WSL, so "elevated" means every WSL user is a Windows
    # administrator. A program that neither answers nor fails within 3s means interop is off.
    interop=off
    if command -v powershell.exe >/dev/null 2>&1; then
      adm=$(bounded_for 3 powershell.exe -NoProfile -NonInteractive -Command '([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)' 2>/dev/null | tr -d '\r' | head -1 || true)
      case "$adm" in
        True) interop=elevated ;;
        False) interop=limited ;;
      esac
      if [ "$interop" != off ]; then
        win=$(bounded_for 3 powershell.exe -NoProfile -NonInteractive -Command '(Get-CimInstance Win32_VideoController).Name -join ";"' 2>/dev/null | tr -d '\r' | head -1 || true)
        # What the whole laptop has, to compare with what WSL got: "<RAM bytes>;<logical cores>".
        winhw=$(bounded_for 3 powershell.exe -NoProfile -NonInteractive -Command '$c = Get-CimInstance Win32_ComputerSystem; "$($c.TotalPhysicalMemory);$($c.NumberOfLogicalProcessors)"' 2>/dev/null | tr -d '\r' | head -1 || true)
      fi
    fi
  fi
  printf 'wsl=%s\nwinvideo=%s\ninterop=%s\nwinhw=%s\n' "$wsl" "$win" "$interop" "$winhw"
  if [ "$IS_DARWIN" != 1 ]; then
    swap=$(awk '/^SwapTotal:/ {printf "%.0f", $2*1024}' /proc/meminfo 2>/dev/null || true)
    printf 'swap_total=%s\n' "$swap"
    if command -v nvcc >/dev/null 2>&1 || [ -x /usr/local/cuda/bin/nvcc ]; then nvcc=1; fi
    printf 'nvcc=%s\n' "$nvcc"
  fi
}

# cores: this host's logical CPU count (macOS asks sysctl, which coreutils' nproc need not be installed for).
cores() {
  if [ "$IS_DARWIN" = 1 ]; then sysctl -n hw.ncpu; else nproc; fi
}

# machine: this host's CPU architecture, spelled as Linux does (arm64 is aarch64).
machine() {
  case "$(uname -m)" in
    arm64) printf 'aarch64\n' ;;
    *) uname -m ;;
  esac
}

# mem_darwin: mem_total and mem_avail (free + inactive + speculative pages) from sysctl and vm_stat.
mem_darwin() {
  local total page free inactive spec
  total=$(sysctl -n hw.memsize 2>/dev/null) || return 0
  printf 'mem_total=%s\n' "$total"
  page=$(sysctl -n hw.pagesize 2>/dev/null || echo 4096)
  free=$(vm_stat 2>/dev/null | awk '/^Pages free/ {gsub(/\./, "", $3); print $3}')
  inactive=$(vm_stat 2>/dev/null | awk '/^Pages inactive/ {gsub(/\./, "", $3); print $3}')
  spec=$(vm_stat 2>/dev/null | awk '/^Pages speculative/ {gsub(/\./, "", $3); print $3}')
  printf 'mem_avail=%s\n' $(((${free:-0} + ${inactive:-0} + ${spec:-0}) * page))
}

# power_state: "ac" or "battery" from the kernel's power supplies (WSL2 shows
# the Windows laptop's), pmset on a Mac; nothing when it cannot be told. The
# directory is overridable for tests (GOWAY_POWER_SUPPLY_DIR).
power_state() {
  local dir=${GOWAY_POWER_SUPPLY_DIR:-/sys/class/power_supply} d type status online ac=0 disch=0
  if [ "$IS_DARWIN" = 1 ] && [ -z "${GOWAY_POWER_SUPPLY_DIR:-}" ]; then
    case "$(pmset -g batt 2>/dev/null || true)" in
      *"'Battery Power'"*) printf 'power=battery\n' ;;
      *"'AC Power'"*) printf 'power=ac\n' ;;
    esac
    return 0
  fi
  [ -d "$dir" ] || return 0
  for d in "$dir"/*; do
    [ -r "$d/type" ] || continue
    type=$(cat "$d/type" 2>/dev/null || true)
    case "$type" in
      Mains | USB*)
        online=$(cat "$d/online" 2>/dev/null || true)
        [ "$online" = 1 ] && ac=1 ;;
      Battery)
        status=$(cat "$d/status" 2>/dev/null || true)
        case "$status" in
          Discharging) disch=1 ;;
          Charging | Full | "Not charging") ac=1 ;;
        esac ;;
    esac
  done
  if [ "$ac" = 1 ]; then printf 'power=ac\n'; elif [ "$disch" = 1 ]; then printf 'power=battery\n'; fi
  return 0
}

# idle_secs: seconds since the owner last touched the keyboard or mouse, when
# it can be told: from Windows through interop on WSL (nothing when interop is
# off or slow), from the HID idle time on a Mac. Never fails.
idle_secs() {
  local v=""
  if [ "$IS_DARWIN" = 1 ]; then
    v=$(ioreg -c IOHIDSystem 2>/dev/null | awk '/HIDIdleTime/ {print int($NF / 1000000000); exit}' || true)
  elif grep -qi microsoft /proc/version 2>/dev/null && command -v powershell.exe >/dev/null 2>&1; then
    v=$(bounded_for 3 powershell.exe -NoProfile -NonInteractive -Command 'Add-Type -Name L -Namespace G -MemberDefinition @"
[StructLayout(LayoutKind.Sequential)] public struct I { public uint s; public uint t; }
[DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref I p);
"@; $i = New-Object G.L+I; $i.s = 8; if ([G.L]::GetLastInputInfo([ref]$i)) { [int64](((([int64][Environment]::TickCount -band 0xFFFFFFFF) - $i.t) -band 0xFFFFFFFF) / 1000) }' 2>/dev/null | tr -d '\r' | head -1 || true)
  fi
  case "$v" in "" | *[!0-9]*) ;; *) printf 'idle_secs=%s\n' "$v" ;; esac
  return 0
}

# probe ROOT [disk] [budget:MAX:MIN_FREE] [static] [owner] [tools:A,B]: key=value facts for scheduling and status.
# "tools:A,B" adds want.TOOL=<version line> for each tool (a run refreshing its version cache).
# RAM is always reported; "static" adds the rarely changing hardware facts;
# "owner" adds power= and idle_secs= when they can be read (absent: unknown).
probe() {
  local root jobs=0 l a want_disk=0 want_static=0 want_owner=0 budget="" tools="" room=1073741824:10
  root=$(root_dir "$1")
  shift
  for a in "$@"; do
    case "$a" in room:[0-9]*:[0-9]*) room=${a#room:} ;; disk) want_disk=1 ;; static) want_static=1 ;; owner) want_owner=1 ;; budget:[0-9]*:[0-9]*) budget=${a#budget:} ;; tools:*) tools=${a#tools:} ;; esac
  done
  if [ "$IS_DARWIN" = 1 ]; then
    mem_darwin
  else
  awk '/^MemTotal:/ {t=$2} /^MemAvailable:/ {a=$2} END {if (t) printf "mem_total=%.0f\n", t*1024; if (a) printf "mem_avail=%.0f\n", a*1024}' /proc/meminfo 2>/dev/null || true
  fi
  if [ "$want_static" = 1 ]; then static_facts; fi
  printf 'arch=%s\nhostname=%s\ncores=%s\n' "$(machine)" "$(uname -n)" "$(cores)"
  printf 'os=%s\n' "$(uname -s | tr '[:upper:]' '[:lower:]')"
  # The host's wall clock in whole seconds; goway computes the clock offset from it.
  printf 'epoch=%s\n' "$(date +%s)"
  if [ "$IS_DARWIN" = 1 ]; then
    # "{ 1.23 1.45 1.67 }"
    read -r _ l1 l5 l15 _ < <(sysctl -n vm.loadavg)
  else
    read -r l1 l5 l15 _ </proc/loadavg
  fi
  printf 'load1=%s\nload5=%s\nload15=%s\n' "$l1" "$l5" "$l15"
  if [ -d "$root/work" ]; then
    for l in "$root"/work/*/lock; do
      [ -e "$l" ] || continue
      flock -n "$l" true || jobs=$((jobs + 1))
    done
  fi
  printf 'jobs=%s\n' "$jobs"
  if [ "$want_owner" = 1 ]; then power_state; idle_secs; fi
  probe_footprints "$root" "$room" "$want_disk"
  probe_mempeaks "$root"
  if [ -n "$tools" ]; then
    # shellcheck disable=SC2086 # the comma-separated names are split on purpose
    (IFS=,; want_facts $tools)
  fi
  if [ "$want_disk" = 1 ]; then
    printf 'disk_used=%s\n' "$(du -sb "$root" 2>/dev/null | cut -f1 || true)"
    printf 'disk_free=%s\n' "$(df -B1 --output=avail "$HOME" | tail -1 | tr -d ' ')"
    if [ -n "$budget" ]; then
      printf 'disk_max=%s\ndisk_min_free=%s\n' "$(budget_max "$HOME" "${budget%%:*}")" "${budget#*:}"
    fi
  fi
}

# probe_mempeaks ROOT: the recorded memory peaks as mempeak.ID=BYTES.
probe_mempeaks() {
  local f v
  for f in "$1"/mempeaks/*; do
    [ -f "$f" ] || continue
    v=$(mempeak_of "$f")
    [ "$v" -gt 0 ] || continue
    printf 'mempeak.%s=%s\n' "${f##*/}" "$v"
  done
}

# probe_footprints ROOT ROOM WANT_DISK: the recorded footprints as
# footprint.ID=BYTES. When any exist, also the free space (unless the probe
# prints it anyway) and, only when the biggest footprint plus margin does not
# fit in it, the bytes goway holds (the most eviction could free).
probe_footprints() {
  local f v max=0 free
  for f in "$1"/footprints/*; do
    [ -f "$f" ] || continue
    v=$({ cat "$f" 2>/dev/null || true; } | head -1)
    case "$v" in "" | *[!0-9]*) continue ;; esac
    printf 'footprint.%s=%s\n' "${f##*/}" "$v"
    if [ "$v" -gt "$max" ]; then max=$v; fi
  done
  [ "$max" -gt 0 ] || return 0
  free=$(df -B1 --output=avail "$(nearest_dir "$1")" 2>/dev/null | tail -1 | tr -d ' ')
  [ -n "$free" ] || return 0
  if [ "$3" != 1 ]; then
    printf 'disk_free=%s\n' "$free"
    if [ "$free" -lt $((max + $(room_margin "$2" "$max"))) ]; then
      printf 'disk_used=%s\n' "$(du -sb "$1" 2>/dev/null | cut -f1 || true)"
    fi
  fi
}

# A work dir with no lock file yet and younger than this (seconds) is never removed by gc.
WORK_GRACE=120

# Seconds since boot: monotonic, so a step of the wall clock never changes
# it. Empty where the host does not say (/proc/uptime).
uptime_secs() {
  local up
  read -r up _ </proc/uptime 2>/dev/null || return 0
  printf '%s' "${up%%.*}"
}

# proc_start PID: the start time (clock ticks since boot) of process PID, so
# a recycled pid is not mistaken for the process that wrote it. Empty if unknown.
proc_start() {
  { sed 's/^.*) //' "/proc/$1/stat" 2>/dev/null || true; } | awk '{ print $20 }'
}

# work_young DIR: whether the run that owns work dir DIR is still starting.
# True while its creator process is alive, or for WORK_GRACE seconds after
# the dir was born by the monotonic clock. Never judged by wall-clock age,
# which a clock jump can make huge. A dir without the marker (an older
# goway made it) is not young by this test.
work_young() {
  local pid start born now
  if read -r pid start <"$1/creator" 2>/dev/null && [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    if [ -z "$start" ] || [ "$(proc_start "$pid")" = "$start" ]; then return 0; fi
  fi
  born=$(cat "$1/born" 2>/dev/null || true)
  now=$(uptime_secs)
  case "$born" in "" | *[!0-9]*) return 1 ;; esac
  case "$now" in "" | *[!0-9]*) return 1 ;; esac
  [ "$now" -ge "$born" ] && [ $((now - born)) -lt "$WORK_GRACE" ]
}

# work_alive DIR: whether the run that owns the work dir DIR still has a live shell
# or job (its recorded runner or job pid): liveness by process, never by age, as a
# second guard behind the dir's lock.
work_alive() {
  local f pid
  for f in runner pid; do
    pid=$(cat "$1/$f" 2>/dev/null || true)
    case "$pid" in "" | *[!0-9]*) continue ;; esac
    if kill -0 "$pid" 2>/dev/null; then return 0; fi
  done
  return 1
}

# Seconds since the last use of DIR (its meta.json mtime).
age_of() {
  local m
  # An entry another gc removed meanwhile counts as brand new (kept).
  m=$(stat -c %Y "$1/meta.json" 2>/dev/null || stat -c %Y "$1" 2>/dev/null || echo "$2")
  printf '%s' $(($2 - m))
}

# The repo name and id recorded in DIR/meta.json, tab separated.
repo_of() {
  local meta name id
  meta=$(cat "$1/meta.json" 2>/dev/null || true)
  name=$(printf '%s' "$meta" | sed -n 's/.*"repo":"\([^"]*\)".*/\1/p')
  id=$(printf '%s' "$meta" | sed -n 's/.*"repo_id":"\([^"]*\)".*/\1/p')
  printf '%s\t%s' "${name:--}" "${id:--}"
}

# What the last gc_entry/evict_slot decided (action and bytes), the bytes
# gc has removed or would remove so far, and the paths a dry run lists as
# gone ("|path|" each), so a dry-run eviction never counts them twice.
GC_ACTION=""
GC_BYTES=0
GC_FREED=0
GC_GONE=""
# Paths evict must never remove (the current run's own entries), as |path|| items.
GC_PROTECT=""

# Decide one entry: print "action TAB kind TAB age TAB bytes TAB repo TAB id TAB path"
# and remove it when the action is "remove" and MODE is apply. The entry's
# locks are taken exclusively (non-blocking) while it is removed. VERB is
# the word printed for a removal ("remove", or "evict" for the disk budget).
gc_entry() {
  local kind=$1 dir=$2 ttl=$3 now=$4 mode=$5 repo_filter=$6 verb=${7:-remove} action age bytes repo locks=() l fd
  GC_ACTION=skip; GC_BYTES=0
  [ -d "$dir" ] || return 0
  repo=$(repo_of "$dir")
  if [ -n "$repo_filter" ] && [ "${repo%%$'\t'*}" != "$repo_filter" ] && [ "${repo##*$'\t'}" != "$repo_filter" ]; then
    return 0
  fi
  case "$kind" in
    cache) for l in "$dir"/target-*.lock; do [ -e "$l" ] && locks+=("$l"); done ;;
    *) locks=("$dir/lock") ;;
  esac
  # Only entries goway labelled as this kind are ever removed.
  if ! grep -q "\"kind\":\"$kind\"" "$dir/meta.json" 2>/dev/null; then
    printf 'unlabelled\t%s\t0\t0\t-\t-\t%s\n' "$kind" "$dir"
    return 0
  fi
  # Take every lock of the entry first, then read its age: a sync or run
  # that refreshed the entry just before cannot be raced.
  action=keep
  fd=20
  for l in ${locks[@]+"${locks[@]}"}; do
    [ -e "$l" ] || continue
    # Open read-only: opening for write would refresh a slot lock's mtime
    # (the slot's last-use stamp, evict_slot) and, worse, would re-create a
    # lock file a finishing run has just removed, leaving a stray file that
    # makes the run's own rm -rf fail with "Directory not empty". A lock that
    # vanished since the check above belongs to an entry being removed: busy.
    if eval "exec $fd<\"\$l\"" 2>/dev/null; then
      if ! flock -n "$fd"; then action=busy; fi
    else
      action=busy
      continue
    fi
    fd=$((fd + 1))
  done
  age=$(age_of "$dir" "$now")
  if [ "$action" = keep ] && [ "$age" -ge "$ttl" ]; then action=$verb; fi
  # A run creates its work dir in one ssh call and its lock in the next, so
  # for a moment the dir is unlocked and has no lock file yet. Never remove
  # such a young dir, not even with --all (a finished run always has the file).
  if [ "$kind" = work ] && [ "$action" = "$verb" ] && [ ! -e "$dir/lock" ] && [ "$age" -lt "$WORK_GRACE" ]; then action=keep; fi
  # A run that is starting is protected by liveness (its creator process, or
  # the monotonic clock), not by wall-clock age: if the host's clock jumps
  # forward every age is huge, and this dir must still survive.
  if [ "$kind" = work ] && [ "$action" = "$verb" ] && work_young "$dir"; then action=keep; fi
  # A work dir whose run is alive is never taken, whatever its lock or age say.
  if [ "$kind" = work ] && [ "$action" = "$verb" ] && work_alive "$dir"; then action=busy; fi
  bytes=$(du -sb "$dir" 2>/dev/null | cut -f1 || echo 0)
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$action" "$kind" "$age" "${bytes:-0}" "$repo" "$dir"
  GC_ACTION=$action; GC_BYTES=${bytes:-0}
  if [ "$action" = "$verb" ]; then
    GC_FREED=$((GC_FREED + GC_BYTES))
    GC_GONE="$GC_GONE|$dir|"
    if [ "$mode" = apply ]; then rm -rf "$dir"; fi
  fi
  while [ "$fd" -gt 20 ]; do fd=$((fd - 1)); eval "exec $fd>&-"; done
}

# human BYTES: binary units, one decimal ("3.1 GiB").
human() {
  awk -v b="$1" 'BEGIN { split("B KiB MiB GiB TiB", u, " "); i = 1; while (b >= 1024 && i < 5) { b /= 1024; i++ }
    if (i == 1) printf "%d B", b; else printf "%.1f %s", b, u[i] }'
}

# The disk budget in bytes: MAX when set (> 0), else the smaller of 20% of
# the filesystem holding ROOT and 50 GiB.
budget_max() {
  local total
  if [ "${2:-0}" -gt 0 ] 2>/dev/null; then printf '%s' "$2"; return 0; fi
  total=$(df -B1 --output=size "$1" 2>/dev/null | tail -1 | tr -d ' ')
  total=${total:-0}
  if [ $((total / 5)) -lt 53687091200 ]; then printf '%s' $((total / 5)); else printf '%s' 53687091200; fi
}

# Footprints: the peak bytes a repository has occupied on this host (slot
# tree, target dir and the shared compiler caches), kept in one small file per
# repository id under footprints/ (neither gc nor eviction lists it). The probe reports them; goway's
# scheduler skips a host that cannot hold the next run, and `run` makes room
# first. ROOM is "FLOOR:PERCENT" (goway's margin rule: the larger of FLOOR
# bytes and PERCENT of the footprint).
footprint_file() { printf '%s/footprints/%s' "$1" "$2"; }

# footprint_of ROOT REPO_ID: the recorded peak, 0 when none.
footprint_of() {
  local v
  v=$({ cat "$(footprint_file "$1" "$2")" 2>/dev/null || true; } | head -1)
  case "$v" in "" | *[!0-9]*) v=0 ;; esac
  printf '%s' "$v"
}

# footprint_measure CACHE SLOT: bytes of the slot's tree and target dir and of
# the repository's shared compiler caches (0 when none exist).
footprint_measure() {
  local d list=()
  for d in "$1/tree-$2" "${CARGO_TARGET_DIR:-$1/target-$2}" "$1/sccache" "$1/ccache" "$1/cpm"; do
    if [ -e "$d" ]; then list+=("$d"); fi
  done
  if [ ${#list[@]} -eq 0 ]; then printf 0; return 0; fi
  { du -sbc "${list[@]}" 2>/dev/null || true; } | tail -1 | cut -f1
}

# footprint_record ROOT REPO_ID BYTES: keep the larger of BYTES and the recorded peak.
footprint_record() {
  local f old
  case "$2" in "" | *[!A-Za-z0-9._-]*) return 0 ;; esac
  case "$3" in "" | *[!0-9]*) return 0 ;; esac
  [ "$3" -gt 0 ] || return 0
  old=$(footprint_of "$1" "$2")
  [ "$3" -gt "$old" ] || return 0
  f=$(footprint_file "$1" "$2")
  mkdir -p "${f%/*}" 2>/dev/null || return 0
  printf '%s\n' "$3" >"$f.tmp.$$" 2>/dev/null && mv -f "$f.tmp.$$" "$f" 2>/dev/null || rm -f "$f.tmp.$$"
  return 0
}

# Memory peaks: the most memory a repository's job tree has used on this
# host, one small file per repository id under mempeaks/ (reported by the
# probe as mempeak.ID=BYTES; goway never places the repository on a host whose
# memory is below it plus a margin, and waits while less is available).
mempeak_file() { printf '%s/mempeaks/%s' "$1" "$2"; }

# mempeak_record ROOT REPO_ID BYTES: append BYTES to the repository's recent runs, keeping the
# last MEMPEAK_RUNS lines (the file is that history, newest last; the probe reports the largest, so one
# inflated run, such as an OOM-killed run's quarter more, ages out after a few runs).
MEMPEAK_RUNS=5
mempeak_record() {
  local f
  case "$2" in "" | *[!A-Za-z0-9._-]*) return 0 ;; esac
  case "$3" in "" | *[!0-9]*) return 0 ;; esac
  [ "$3" -gt 0 ] || return 0
  f=$(mempeak_file "$1" "$2")
  mkdir -p "${f%/*}" 2>/dev/null || return 0
  { { cat "$f" 2>/dev/null || true; printf '%s\n' "$3"; } | grep -E '^[0-9]+$' | tail -n "$MEMPEAK_RUNS" >"$f.tmp.$$" 2>/dev/null && mv -f "$f.tmp.$$" "$f" 2>/dev/null; } || rm -f "$f.tmp.$$"
  return 0
}

# mempeak_of FILE: the largest recorded peak in FILE, 0 when none.
mempeak_of() {
  local v
  v=$({ grep -E '^[0-9]+$' "$1" 2>/dev/null || true; } | sort -n | tail -1)
  printf '%s' "${v:-0}"
}

# oom_in_dmesg: whether the kernel log's recent lines show an OOM kill
# (often unreadable without privilege: then it simply says no).
oom_in_dmesg() {
  local log
  log=$({ bounded_for 3 dmesg 2>/dev/null || true; } | tail -n 200 || true)
  printf '%s\n' "$log" | grep -qiE 'out of memory: kill|oom-kill|killed process'
}

# memory_report ROOT REPO_ID WORK RC: after the job, record its measured peak
# memory; when the job was OOM-killed (the scope's oom_kill count, else exit 137 with
# the kernel log or nothing else to explain it) say so in plain words, with the
# peak and the host's total and what to do. A killed job's true peak is higher
# than measured, so a quarter more is recorded.
memory_report() {
  local root=$1 repo_id=$2 work=$3 rc=$4 peak total oom=0 gib=1073741824
  peak=$(cat "$work/mempeak" 2>/dev/null || echo 0)
  case "$peak" in "" | *[!0-9]*) peak=0 ;; esac
  if [ -e "$work/oom" ]; then oom=1; fi
  if [ "$oom" = 0 ] && [ "$rc" = 137 ] && [ ! -e "$work/lost" ] && oom_in_dmesg; then oom=1; fi
  if [ "$oom" = 1 ]; then
    mempeak_record "$root" "$repo_id" $((peak + peak / 4))
    total=$(awk '/^MemTotal:/ {printf "%.0f", $2 * 1024}' /proc/meminfo 2>/dev/null || true)
    printf 'goway: this host ran out of memory: the kernel killed a process of the job (measured peak %s of %s total). Run it on another host (--host), ask for more memory (--needs mem>=%sG), run fewer jobs at once (CARGO_BUILD_JOBS=2, nextest -j 2), or give the helper more memory (goway-setup tune on WSL helpers).\n' \
      "$(human "$peak")" "$(human "${total:-0}")" "$(((peak + peak / 4 + gib - 1) / gib))" >&2
  else
    mempeak_record "$root" "$repo_id" "$peak"
  fi
  return 0
}

# room_margin ROOM BYTES: the margin to keep free on top of a footprint of BYTES.
room_margin() {
  local floor=${1%%:*} pct=${1#*:} m
  m=$(($2 * pct / 100))
  if [ "$m" -lt "$floor" ]; then m=$floor; fi
  printf '%s' "$m"
}

# make_room ROOT CACHE SLOT REPO_ID ROOM MAX_DISK: before the command starts,
# when the disk has less free than this repository's footprint plus margin
# (less what its slot already holds), evict least recently used unlocked
# entries to make it, and say so.
make_room() {
  local root=$1 cache=$2 slot=$3 repo_id=$4 room=$5 fp own need free before
  [ -n "$room" ] || return 0
  fp=$(footprint_of "$root" "$repo_id")
  [ "$fp" -gt 0 ] || return 0
  own=$(footprint_measure "$cache" "$slot")
  need=$((fp + $(room_margin "$room" "$fp") - own))
  [ "$need" -gt 0 ] || return 0
  free=$(df -B1 --output=avail "$root" 2>/dev/null | tail -1 | tr -d ' ')
  [ "${free:-0}" -lt "$need" ] || return 0
  before=$GC_FREED
  evict "$root" "$(date +%s)" apply "" "${6:-0}" "$need" "" exact
  printf 'goway: this repository needs about %s on this host and %s was free; the disk budget freed %s first\n' \
    "$(human "$need")" "$(human "${free:-0}")" "$(human $((GC_FREED - before)))" >&2
}

# disk_full_note ROOT CACHE SLOT REPO_ID ROOM MAX_DISK MIN_FREE: after a failed
# command, when the disk is (nearly) full, say so in plain words: what the
# repository needs and what is free, what the disk budget freed, and what to
# ask for instead of the compiler's or linker's own error.
disk_full_note() {
  local root=$1 cache=$2 slot=$3 repo_id=$4 room=$5 free total fp need before freed gib=1073741824
  free=$(df -B1 --output=avail "$root" 2>/dev/null | tail -1 | tr -d ' ')
  total=$(df -B1 --output=size "$root" 2>/dev/null | tail -1 | tr -d ' ')
  [ -n "$free" ] && [ -n "$total" ] || return 0
  if [ "$free" -ge "$gib" ] && [ $((free * 50)) -ge "$total" ]; then return 0; fi
  footprint_record "$root" "$repo_id" "$(footprint_measure "$cache" "$slot")"
  fp=$(footprint_of "$root" "$repo_id")
  need=$((fp + $(room_margin "${room:-1073741824:10}" "$fp")))
  before=$GC_FREED
  evict "$root" "$(date +%s)" apply "" "${6:-0}" "${7:-0}" ""
  freed=$((GC_FREED - before))
  printf 'goway: this host ran out of disk (%s free of %s). This repository needs at least %s here; the disk budget freed %s afterwards. Run it again with --needs disk>=%sG, or on another host.\n' \
    "$(human "$free")" "$(human "$total")" "$(human "$need")" "$(human "$freed")" "$(((need + gib - 1) / gib))" >&2
}

# evict_slot CACHE_DIR K NOW MODE REPO: evict one build slot (its tree-K and
# target-K) of a per-repository cache when its lock is free. The slot's age
# is its lock file's mtime, read after the lock is held.
evict_slot() {
  local dir=$1 k=$2 now=$3 mode=$4 repo_filter=$5 lock repo age bytes m
  lock="$dir/target-$k.lock"
  GC_ACTION=skip; GC_BYTES=0
  [ -e "$lock" ] || return 0
  [ -d "$dir/tree-$k" ] || [ -d "$dir/target-$k" ] || return 0
  grep -q '"kind":"cache"' "$dir/meta.json" 2>/dev/null || return 0
  repo=$(repo_of "$dir")
  if [ -n "$repo_filter" ] && [ "${repo%%$'\t'*}" != "$repo_filter" ] && [ "${repo##*$'\t'}" != "$repo_filter" ]; then
    return 0
  fi
  exec 20>>"$lock"
  if ! flock -n 20 || ! same_fd "$lock" 20; then
    exec 20>&-
    printf 'busy\tslot\t0\t0\t%s\t%s\n' "$repo" "$dir/tree-$k"
    GC_ACTION=busy
    return 0
  fi
  m=$(stat -c %Y "$lock" 2>/dev/null || echo "$now")
  age=$((now - m))
  bytes=$({ du -sbc "$dir/tree-$k" "$dir/target-$k" 2>/dev/null || true; } | tail -1 | cut -f1)
  printf 'evict\tslot\t%s\t%s\t%s\t%s\n' "$age" "${bytes:-0}" "$repo" "$dir/tree-$k"
  GC_ACTION=evict; GC_BYTES=${bytes:-0}
  GC_FREED=$((GC_FREED + GC_BYTES))
  if [ "$mode" = apply ]; then rm -rf "$dir/tree-$k" "$dir/target-$k"; fi
  exec 20>&-
}

# evict ROOT NOW MODE REPO MAX_DISK MIN_FREE [log [exact]]: when goway's root is over
# its budget (MAX_DISK bytes, 0 = auto) or the disk has less than MIN_FREE
# bytes free, evict unlocked entries, least recently used first (build slots,
# then work dirs, seeds, whole repository caches at the same age), until both
# hold. Entries in use are skipped; the same lock rules as gc_entry apply.
# With "log" a summary is left for the next run to print.
evict() {
  local root=$1 now=$2 mode=$3 repo=$4 total minfree max used free need freed=0 count=0 m rank kind path k d l slot_log="" sub list
  local before=$GC_FREED
  max=$(budget_max "$root" "$5")
  used=$(du -sb "$root" 2>/dev/null | cut -f1 || echo 0)
  free=$(df -B1 --output=avail "$root" 2>/dev/null | tail -1 | tr -d ' ')
  used=${used:-0}; free=${free:-0}
  # A dry run has not removed what gc listed before this; pretend it did.
  if [ "$mode" != apply ]; then
    used=$((used - before)); free=$((free + before))
    [ "$used" -ge 0 ] || used=0
  fi
  # On a small disk (a tmpfs, a tiny VM) a fixed MIN_FREE could never be met
  # and would empty goway's root after every run: cap it at a quarter of the disk.
  total=$(df -B1 --output=size "$root" 2>/dev/null | tail -1 | tr -d ' ')
  minfree=$6
  # "exact" (a run making room for itself) keeps the whole figure.
  if [ "${8:-}" != exact ] && [ $((${total:-0} / 4)) -lt "$minfree" ]; then minfree=$((${total:-0} / 4)); fi
  need=$((used - max))
  if [ $((minfree - free)) -gt "$need" ]; then need=$((minfree - free)); fi
  [ "$need" -gt 0 ] || return 0
  list=$(
    for d in "$root"/work/*/; do
      [ -d "$d" ] || continue
      d=${d%/}
      m=$(stat -c %Y "$d/meta.json" 2>/dev/null || stat -c %Y "$d" 2>/dev/null || echo "$now")
      printf '%s\t1\twork\t%s\t-\n' "$m" "$d"
    done
    for d in "$root"/seed/*/*/; do
      [ -d "$d" ] || continue
      d=${d%/}
      m=$(stat -c %Y "$d/meta.json" 2>/dev/null || stat -c %Y "$d" 2>/dev/null || echo "$now")
      printf '%s\t1\tseed\t%s\t-\n' "$m" "$d"
    done
    for d in "$root"/cache/*/; do
      [ -d "$d" ] || continue
      d=${d%/}
      m=$(stat -c %Y "$d/meta.json" 2>/dev/null || stat -c %Y "$d" 2>/dev/null || echo "$now")
      printf '%s\t2\tcache\t%s\t-\n' "$m" "$d"
      for l in "$d"/target-*.lock; do
        [ -e "$l" ] || continue
        k=${l##*/target-}
        k=${k%.lock}
        if [ -z "$k" ] || [ -n "${k//[0-9]/}" ]; then continue; fi
        printf '%s\t0\tslot\t%s\t%s\n' "$(stat -c %Y "$l" 2>/dev/null || echo "$now")" "$d" "$k"
      done
    done | sort -n -k1,1 -k2,2
  )
  while IFS=$'\t' read -r m rank kind path k; do
    [ -n "$kind" ] || continue
    [ "$freed" -lt "$need" ] || break
    case "$GC_GONE" in *"|$path|"*) continue ;; esac
    case "$GC_PROTECT" in *"|$path|"*) continue ;; esac
    case "$kind" in
      slot) evict_slot "$path" "$k" "$now" "$mode" "$repo" ;;
      *) gc_entry "$kind" "$path" 0 "$now" "$mode" "$repo" evict ;;
    esac
    if [ "$GC_ACTION" = evict ]; then
      sub=0
      # A dry run has not removed the slots it listed; do not count them twice.
      if [ "$kind" = cache ] && [ "$mode" != apply ]; then
        sub=$(printf '%s' "$slot_log" | awk -F'\t' -v d="$path" '$1 == d { s += $2 } END { print s + 0 }')
      fi
      [ "$kind" != slot ] || slot_log="$slot_log$path"$'\t'"$GC_BYTES"$'\n'
      freed=$((freed + GC_BYTES - sub))
      count=$((count + 1))
    fi
  done <<<"$list"
  GC_FREED=$((before + freed))
  if [ "$mode" = apply ] && [ "$count" -gt 0 ] && [ "${7:-}" = log ]; then
    printf 'goway: disk budget: evicted %s entries, freed %s (goway used %s of %s, %s free)\n' \
      "$count" "$(human "$freed")" "$(human "$used")" "$(human "$max")" "$(human "$free")" >>"$root/evicted.log" 2>/dev/null || true
  fi
}

# gc ROOT NOW CACHE_TTL ORPHAN_TTL KEPT_TTL MODE REPO OLDER_THAN [MAX_DISK MIN_FREE [log]]
# TTLs in seconds; OLDER_THAN (seconds, or empty) replaces every TTL. With
# MAX_DISK and MIN_FREE (bytes) it then evicts to the disk budget (evict).
gc() {
  local root now cache_ttl orphan_ttl kept_ttl mode repo older d ttl max_disk=${9:-} min_free=${10:-}
  root=$(root_dir "$1"); now=$2; cache_ttl=$3; orphan_ttl=$4; kept_ttl=$5
  mode=$6; repo=$7; older=$8
  [ -d "$root" ] || return 0
  if [ ! -e "$root/.goway-root" ]; then
    printf 'goway-remote: %s is not marked as goway state; gc removes nothing there\n' "$root" >&2
    return 0
  fi
  for d in "$root"/work/*/; do
    [ -d "$d" ] || continue
    d=${d%/}
    if [ -e "$d/keep" ]; then ttl=$kept_ttl; else ttl=$orphan_ttl; fi
    gc_entry work "$d" "${older:-$ttl}" "$now" "$mode" "$repo"
  done
  for d in "$root"/seed/*/*/; do
    [ -d "$d" ] || continue
    gc_entry seed "${d%/}" "${older:-$cache_ttl}" "$now" "$mode" "$repo"
  done
  for d in "$root"/cache/*/; do
    [ -d "$d" ] || continue
    gc_entry cache "${d%/}" "${older:-$cache_ttl}" "$now" "$mode" "$repo"
  done
  if [ "$mode" = apply ]; then
    find "$root/seed" -mindepth 1 -maxdepth 1 -type d -empty -delete 2>/dev/null || true
  fi
  if [ -n "$min_free" ]; then evict "$root" "$now" "$mode" "$repo" "${max_disk:-0}" "$min_free" "${11:-}"; fi
  mempeaks_reset "$root" "$mode" "$repo" "$older"
}

# mempeaks_reset ROOT MODE REPO OLDER_THAN: forget the recorded memory peaks of repository id REPO
# (gc --repo ID), or of every repository with gc --all (OLDER_THAN 0), so a bad record cannot hold
# a repository back; nothing on a dry run.
mempeaks_reset() {
  [ "$2" = apply ] || return 0
  if [ -n "$3" ]; then
    case "$3" in *[!A-Za-z0-9._-]*) return 0 ;; esac
    rm -f "$1/mempeaks/$3" 2>/dev/null || true
  elif [ "$4" = 0 ]; then
    rm -f "$1"/mempeaks/* 2>/dev/null || true
  fi
  return 0
}

# doctor ROOT: key=value facts about the toolchain and host for `goway doctor`.
# want_version NAME: the first line of NAME's version report ("go version",
# "java -version" and the usual "--version"), bounded; empty when missing.
want_version() {
  case "$1" in
    go) { bounded go version 2>&1 || true; } | head -1 ;;
    java) { bounded java -version 2>&1 || true; } | head -1 ;;
    *) { bounded "$1" --version 2>&1 || true; } | head -1 ;;
  esac
}

# doctor ROOT [TOOL...]: facts about this host; every TOOL (a project's
# needs, names checked here as well as by the client) is reported as
# want.TOOL=<version line>, empty when missing. User-level installs that
# goway's fixes make (~/.local/bin) count.
# fs_facts PREFIX DIR: print PREFIX_fs (file system type), PREFIX_free and
# PREFIX_size (bytes) and PREFIX_noexec (1 when a script placed there will
# not run) for DIR, or its nearest existing ancestor.
fs_facts() {
  local d probe type free size noexec=0
  d=$(nearest_dir "$2")
  type=$(df -T "$d" 2>/dev/null | tail -1 | awk '{print $2}' || true)
  free=$(df -B1 --output=avail "$d" 2>/dev/null | tail -1 | tr -d ' ' || true)
  size=$(df -B1 --output=size "$d" 2>/dev/null | tail -1 | tr -d ' ' || true)
  probe="$d/.goway-exec-$$"
  if printf '#!/bin/sh\nexit 0\n' >"$probe" 2>/dev/null; then
    chmod +x "$probe" 2>/dev/null || true
    "$probe" >/dev/null 2>&1 || noexec=1
    rm -f "$probe"
  fi
  printf '%s_fs=%s\n%s_free=%s\n%s_size=%s\n%s_noexec=%s\n' "$1" "${type:-unknown}" "$1" "${free:-}" "$1" "${size:-}" "$1" "$noexec"
}

# want_facts TOOL...: want.TOOL=<version line> for each safe tool name, empty when missing.
want_facts() {
  local t
  for t in "$@"; do
    case "$t" in '' | *[!A-Za-z0-9._+-]*) continue ;; esac
    if command -v "$t" >/dev/null 2>&1; then
      printf 'want.%s=%s\n' "$t" "$(want_version "$t")"
    else
      printf 'want.%s=\n' "$t"
    fi
  done
}

doctor() {
  local t v pa out root
  root=$(root_dir "$1")
  shift
  user_tool_path
  if [ -f "$HOME/.cargo/env" ]; then printf 'cargo_env=yes\n'; else printf 'cargo_env=no\n'; fi
  # Names only: a proxy URL may carry credentials, so values never leave the host.
  printf 'proxy_vars=%s\n' "$({ env | sed -n 's/=.*//p' | grep -iE '^(https?|all|no)_proxy$' | sort -u | paste -sd, - ; } 2>/dev/null || true)"
  printf 'kernel=%s\n' "$(uname -s)"
  printf 'epoch=%s\n' "$(date +%s)"
  if [ "$IS_DARWIN" = 1 ]; then
    # Which tools still resolve to the BSD versions (no --version, or not GNU).
    v=""
    for t in find sed tar grep stat date sort comm xargs cp du df; do
      out=$("$t" --version 2>&1 || true)
      case "$out" in *GNU*) ;; *) v="$v${v:+,}$t" ;; esac
    done
    printf 'gnu_missing=%s\n' "$v"
    printf 'os=macOS %s\n' "$(sw_vers -productVersion 2>/dev/null || echo unknown)"
  fi
  for t in bash git tar flock setsid cc curl rustup cargo cargo-nextest sccache brew apt-get dnf pacman; do
    if command -v "$t" >/dev/null 2>&1; then
      case "$t" in
        cargo-nextest) v=$({ cargo-nextest nextest --version 2>/dev/null || true; } | head -1) ;;
        *) v=$({ "$t" --version 2>/dev/null || true; } | head -1) ;;
      esac
      printf 'tool.%s=%s\n' "$t" "${v:-present}"
    else
      printf 'tool.%s=\n' "$t"
    fi
  done
  want_facts "$@"
  if [ "$IS_DARWIN" != 1 ]; then
    printf 'os=%s\n' "$(. /etc/os-release 2>/dev/null; printf '%s' "${PRETTY_NAME:-unknown}")"
  fi
  printf 'arch=%s\n' "$(machine)"
  printf 'disk_free=%s\n' "$(df -B1 --output=avail "$HOME" | tail -1 | tr -d ' ')"
  # sshd's effective value when we may ask (root), else its first-match order:
  # drop-ins in lexical order, then the main file.
  pa=$( { sshd -T 2>/dev/null || true; } | awk 'tolower($1) == "passwordauthentication" { print tolower($2); exit }')
  if [ -z "$pa" ]; then
    # shellcheck disable=SC2046
    pa=$(cat $(ls /etc/ssh/sshd_config.d/*.conf 2>/dev/null | sort) /etc/ssh/sshd_config 2>/dev/null | grep -iE '^\s*PasswordAuthentication\s' | head -1 | awk '{print tolower($2)}' || true)
  fi
  printf 'password_auth=%s\n' "${pa:-default-yes}"
  printf 'home=%s\n' "$HOME"
  fs_facts root "$root"
  fs_facts tmp "${TMPDIR:-/tmp}"
  if case_insensitive "$root"; then printf 'root_case_insensitive=1\n'; else printf 'root_case_insensitive=0\n'; fi
  static_facts
  # Whether a cargo home existed before any goway fix (so uninstall never removes it).
  if [ -e "${CARGO_HOME:-$HOME/.cargo}" ]; then printf 'cargo_home=1\n'; else printf 'cargo_home=0\n'; fi
}

# purge ROOT: remove all of goway's state on this host (goway uninstall).
# An owner-only directory with a short path, for things that must fit a unix
# socket address (108 bytes): $XDG_RUNTIME_DIR, else /tmp/goway-<uid>.
short_private_dir() {
  local d="${XDG_RUNTIME_DIR:-/tmp/goway-$(id -u)}"
  mkdir -p -m 700 "$d" 2>/dev/null
  [ -O "$d" ] || return 1
  printf '%s\n' "$d"
}

# sccache_server_tmpdir SOCK: the TMPDIR the running sccache server for the unix socket SOCK
# was started under (a client that finds no server starts one with its own
# environment, so a job whose server idled out mid-run can start one under the
# run's TMPDIR); empty when unknown (no /proc, no such server).
sccache_server_tmpdir() {
  local p env
  for p in /proc/[0-9]*; do
    [ -O "$p" ] && [ -r "$p/environ" ] || continue
    env=$(tr '\0' '\n' <"$p/environ" 2>/dev/null || true)
    case "$env" in *$'\n'"SCCACHE_START_SERVER=1"$'\n'* | "SCCACHE_START_SERVER=1"$'\n'*) ;; *) continue ;; esac
    case "$env" in *$'\n'"SCCACHE_SERVER_UDS=$1"$'\n'* | "SCCACHE_SERVER_UDS=$1"$'\n'*) ;; *) continue ;; esac
    printf '%s\n' "$env" | sed -n 's/^TMPDIR=//p' | head -1
    return 0
  done
  return 0
}

# sccache_heal CACHE TMP: stop this repository's shared sccache server when it
# is broken, so the caller starts a fresh one under the stable TMP. Broken:
# it does not answer, or it was not started by this goway under a TMPDIR that
# still exists (a server of an older goway runs under a deleted per-run
# TMPDIR and fails every build, yet constant use keeps its idle timeout from
# ever firing). Silent when it is healthy; one note when it restarts it.
sccache_heal() {
  local cache=$1 tmp=$2 why= recorded= actual=
  # Only the unix-socket server goway starts; with no socket there is no server
  # to heal (and asking one would start it under the run's TMPDIR).
  [ -S "${SCCACHE_SERVER_UDS:-}" ] || return 0
  TMPDIR=$tmp bounded_for 5 sccache --show-stats >/dev/null 2>&1 || why="it does not answer"
  if [ -z "$why" ]; then
    actual=$(sccache_server_tmpdir "$SCCACHE_SERVER_UDS")
    if [ -n "$actual" ] && [ "$actual" != "$tmp" ]; then why="it runs under the temp directory $actual of a build, which is removed with its run"; fi
  fi
  if [ -z "$why" ]; then
    read -r recorded <"$cache/sccache.tmpdir" 2>/dev/null || recorded=
    if [ -z "$recorded" ]; then
      why="it was started by an older goway, under a temp directory that may be gone"
    elif [ ! -d "$recorded" ]; then
      why="its temp directory $recorded is gone"
    fi
  fi
  [ -n "$why" ] || return 0
  printf 'goway-remote: restarting the shared sccache server (%s)\n' "$why" >&2
  bounded_for 10 sccache --stop-server >/dev/null 2>&1 || true
  rm -f "$cache/sccache.tmpdir" 2>/dev/null || true
  return 0
}

# The sccache server socket for a cache dir: inside it when the path fits a
# unix socket address, else in the short private dir (sccache fails every
# build with "path must be shorter than SUN_LEN" otherwise).
sccache_socket() {
  local s="$1/sccache.sock" d
  if [ "${#s}" -lt 100 ]; then printf '%s\n' "$s"; return 0; fi
  d=$(short_private_dir) || { printf '%s\n' "$s"; return 0; }
  printf '%s/sccache-%s.sock\n' "$d" "$(printf '%s' "$1" | cksum | cut -d' ' -f1)"
}

# Refuses an unmarked root and a root with a run in progress.
purge() {
  local root l s
  root=$(root_dir "$1")
  if [ ! -d "$root" ]; then printf 'absent\n'; return 0; fi
  [ -e "$root/.goway-root" ] || die "$root is not marked as goway state; not removing it"
  for l in "$root"/work/*/lock "$root"/cache/*/target-*.lock; do
    [ -e "$l" ] || continue
    # gc and the end of a run hold locks for moments; a real run for longer.
    flock -w 10 "$l" true || die "a goway run is in progress on this host; try again when it ends"
  done
  for s in "$root"/cache/*; do
    s=$(sccache_socket "$s")
    [ -S "$s" ] || continue
    SCCACHE_SERVER_UDS="$s" sccache --stop-server >/dev/null 2>&1 || true
  done
  # Only goway's own entries: a root that also holds foreign files keeps them.
  rm -rf "$root/work" "$root/seed" "$root/cache" "$root/gpu" "$root/footprints" "$root/mempeaks" "$root/gc.lock" "$root/evicted.log"
  rm -f "$root/.goway-root"
  rmdir "$root" 2>/dev/null || true
  printf 'removed\n'
}

verb=${1:-}
[ -n "$verb" ] || die "no verb"
shift
case "$verb" in
  manifest) manifest "$@" ;;
  receive) receive "$@" ;;
  hashes) hashes "$@" ;;
  deletions) deletions "$@" ;;
  changes) changes "$@" ;;
  verify-wait) verify_wait "$@" ;;
  verify-verdict) verify_verdict "$@" ;;
  run) run "$@" ;;
  envfile) envfile "$@" ;;
  probe) probe "$@" ;;
  cmake-replies) cmake_replies "$@" ;;
  cmake-configure) cmake_configure "$@" ;;
  gc) gc "$@" ;;
  doctor) doctor "$@" ;;
  purge) purge "$@" ;;
  lifeline) lifeline "$@" ;;
  resolve) resolve "$@" ;;
  discard) discard "$@" ;;
  ping) printf 'goway-remote ok\n' ;;
  *) die "unknown verb: $verb" ;;
esac
