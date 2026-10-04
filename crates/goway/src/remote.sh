# goway remote side. Sent inline with every ssh call and run as
#   bash -c "<this script>" goway VERB ARGS...
# so the remote needs nothing installed beyond bash, GNU findutils, tar,
# coreutils and util-linux (flock). Every directory goway owns carries a
# meta.json label and a lock file that is flock-held while in use.
set -Eeuo pipefail
umask 077

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
      work | seed | cache | gpu | gc.lock | evicted.log | .goway-root) ;;
      *) die "$1 exists, is not empty and is not goway state; pick a dedicated remote_root" ;;
    esac
  done
  printf 'goway state; safe to delete with goway gc --all\n' >"$1/.goway-root"
}

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
      if [ "$dir/lock" -ef "/proc/self/fd/$fd" ]; then return 0; fi
    fi
    sleep 0.05
  done
  die "cannot lock $dir (kept vanishing)"
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
  tar -x --unlink-first --recursive-unlink --no-same-owner -C "$seed/tree" -f -
  find "$seed/tree" -mindepth 1 -depth -type d -empty -delete
  if [ -n "${5:-}" ]; then
    work="$root/work/$5"
    mkdir -p "$work"
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
  local tree=$1 name rel dir pre
  while IFS=$SOH read -r -d '' name rel; do
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
    -printf "%f\\001%P\\0")
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
  find "$slot" -mindepth 1 "${prune[@]}" \( -type f -o -type l \) -printf "$REC" | sort -z >"$tmp/slot"
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
    find "$slot" -mindepth 1 "${prune[@]}" -type d -empty -print0 >"$tmp/emptydirs"
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
  find "$slot" -mindepth 1 "${prune[@]}" \( -type f -o -type l \) -printf "$REC" | sort -z >"$slot.slot"
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

# verify_failed PHASE: goway judged the copy bad (or never answered): wipe the
# slot and the seed, say so, and stop. Uses run's variables (dynamic scope).
verify_failed() {
  printf 'goway-remote: the copy of the tree on this host did not verify (phase %s); slot %s is discarded\n' \
    "$1" "$slot" >&2
  slot_wipe "$slot" "$cache" "$(cat "$work/seed" 2>/dev/null || true)" "$root"
  rm -rf "$work"
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

# Kill the job's process group when the ssh session that started it dies
# (sshd does not signal commands without a pty, it orphans them). The job
# writes its pid (= its process group, it is a session leader) to PIDFILE.
watchdog() {
  local session=$1 pidfile=$2 pid=""
  trap '' HUP PIPE
  while [ -z "$pid" ]; do
    kill -0 "$session" 2>/dev/null || return 0
    sleep 0.2
    pid=$(cat "$pidfile" 2>/dev/null || true)
  done
  while kill -0 "$session" 2>/dev/null && kill -0 "$pid" 2>/dev/null; do
    sleep 1
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM -- "-$pid" 2>/dev/null || true
    sleep 5
    kill -KILL -- "-$pid" 2>/dev/null || true
  fi
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
# ELF or PE executable (magic bytes, so scripts never count) and contain
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
    7f454c46 | 4d5a*) ;;
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

# launch_job CMD...: start the job as run does (own session, pid recorded
# for the watchdog, polite priority). JOB_PID and JOB_NICER are run's.
launch_job() {
  setsid sh -c 'echo $$ >"$0"; exec "$@"' "$JOB_PID" "${JOB_NICER[@]}" "$@"
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
  launch_job "$@" "${extra[@]}" 2>"$fifo" || rc=$?
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
  local detect="" gpu_per="" verify="" fresh=0 attempt=1 level=changed
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    case "$1" in
      shard-detect:[0-9]*:[0-9]*:[A-Za-z0-9]*) detect=${1#shard-detect:} ;;
      gpu-slots:[0-9]*) gpu_per=${1#gpu-slots:} ;;
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
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "run: no command"
  [ -d "$work/tree" ] || die "run: no work dir at $work (was it synced?)"

  mark_root "$root"
  mkdir -p "$cache"
  exec 9>"$work/lock"
  flock -x 9
  # What the last automatic disk-budget eviction freed (it ran detached).
  if [ -s "$root/evicted.log" ]; then
    cat "$root/evicted.log" >&2 2>/dev/null || true
    rm -f "$root/evicted.log"
  fi

  [ -f "$cache/meta.json" ] || printf '%s' "$cache_meta" | base64 -d >"$cache/meta.json"
  touch "$cache/meta.json"

  # Settings that already exist win over goway's defaults: first the
  # remote environment and ~/.cargo/env, then the user's --env values;
  # goway only fills in what is still unset.
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
  if [ -f "$work/env" ]; then
    while IFS= read -r -d '' kv; do export "$kv"; done <"$work/env"
    rm -f "$work/env"
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
    if flock -n 7 && [ "$cache/target-$k.lock" -ef /proc/self/fd/7 ]; then slot=$k; break; fi
    exec 7>&-
  done
  if [ -z "$slot" ]; then
    slot=$((RANDOM % slots))
    printf 'goway: all %s build slots busy; waiting for slot %s\n' "$slots" "$slot" >&2
    while :; do
      mkdir -p "$cache"
      exec 7>"$cache/target-$slot.lock"
      flock 7
      [ "$cache/target-$slot.lock" -ef /proc/self/fd/7 ] && break
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
      export SCCACHE_SERVER_UDS="$cache/sccache.sock"
    fi
    # sccache's server is the one process allowed to outlive a run (it
    # keeps the cache warm); make it leave soon after the last build.
    export SCCACHE_IDLE_TIMEOUT="${SCCACHE_IDLE_TIMEOUT:-300}"
    export RUSTC_WRAPPER=sccache
    cc_launcher=sccache
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
  cd "$rundir"
  : >"$work/pid"
  # The watchdog must not inherit the lock fds, or a lingering `sleep`
  # would keep this run's slot and work dir locked after it ends.
  watchdog "$PPID" "$work/pid" </dev/null >/dev/null 2>&1 5>&- 7>&- 9>&- &
  wd=$!
  # Foreground (not `&`): background jobs of a non-interactive shell start
  # with SIGINT and SIGQUIT ignored, and the command must not inherit that.
  # A polite guest on someone's laptop: low CPU and idle-class I/O.
  if [ "$priority" = low ]; then
    if command -v nice >/dev/null 2>&1; then nicer+=(nice -n 10); fi
    if command -v ionice >/dev/null 2>&1; then nicer+=(ionice -c 3); fi
  fi
  if [ -n "$detect" ]; then
    JOB_PID="$work/pid"
    JOB_NICER=("${nicer[@]}")
    shard_run "$detect" "$work" "$@" || rc=$?
  else
    setsid sh -c 'echo $$ >"$0"; exec "$@"' "$work/pid" "${nicer[@]}" "$@" || rc=$?
  fi
  kill "$wd" 2>/dev/null || true
  cd "$root"
  # A failed command: before blaming the code, goway compares every synced
  # file the command did not itself change with the laptop's.
  if [ -n "$verify" ] && [ "$rc" -ne 0 ] && kill -0 "$PPID" 2>/dev/null; then
    tree_stamps "$rundir" >"$work/stamps.after"
    comm -z -12 "$work/stamps.before" "$work/stamps.after" | sed -z "s/^\\([^$SOH]*$SOH\\)\\{2\\}//" |
      sort -z >"$work/untouched"
    comm -z -12 "$work/all.reg" "$work/untouched" >"$work/after.reg"
    comm -z -12 "$work/all.lnk" "$work/untouched" >"$work/after.lnk"
    verify_gate "$work" 2 "$rundir" "$work/after.reg" "$work/after.lnk" || verify_failed 2
  fi
  if [ "$keep" = 1 ]; then cp -a --reflink=auto "$rundir" "$work/tree"; fi
  if [ "$keep" != 1 ]; then rm -rf "$work"; fi
  # Cheap automatic gc of expired entries, detached so it never delays
  # the exit (and never holds the ssh session open).
  if [ -n "$ttls" ]; then
    # At most one automatic gc per root (gc.lock); it only ever removes
    # files and never starts a goway run.
    (trap '' HUP; flock -n 8 || exit 0
     gc "$root_arg" "$(date +%s)" "$t_cache" "$t_orphan" "$t_kept" apply "" "" "$t_max" "$t_minfree" log) \
      8>"$root/gc.lock" </dev/null >/dev/null 2>&1 5>&- 7>&- 9>&- &
  fi
  exit "$rc"
}

# Run "$@" for at most 10 seconds when timeout exists (a hung driver tool
# must never hang a probe).
bounded() {
  if command -v timeout >/dev/null 2>&1; then timeout 10 "$@"; else "$@"; fi
}

# static_facts: facts that change rarely (GPUs, CPU features, KVM, Docker,
# WSL), as key=value lines; "static=1" marks that they were probed. GPU
# lines are "gpu.N=vendor|name|mem_mib|driver|cuda". Nothing here executes
# anything but the vendor query tools, docker info and powershell.exe (WSL
# only, to list the video adapters Windows has).
static_facts() {
  local n=0 pat cuda="" flags="" f kvm=0 docker=0 wsl=0 win=""
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
    if command -v powershell.exe >/dev/null 2>&1; then
      win=$(bounded powershell.exe -NoProfile -NonInteractive -Command '(Get-CimInstance Win32_VideoController).Name -join ";"' 2>/dev/null | tr -d '\r' | head -1 || true)
    fi
  fi
  printf 'wsl=%s\nwinvideo=%s\n' "$wsl" "$win"
}

# probe ROOT [disk] [budget:MAX:MIN_FREE] [static]: key=value facts for scheduling and status.
# RAM is always reported; "static" adds the rarely changing hardware facts.
probe() {
  local root jobs=0 l a want_disk=0 want_static=0 budget=""
  root=$(root_dir "$1")
  shift
  for a in "$@"; do
    case "$a" in disk) want_disk=1 ;; static) want_static=1 ;; budget:[0-9]*:[0-9]*) budget=${a#budget:} ;; esac
  done
  awk '/^MemTotal:/ {t=$2} /^MemAvailable:/ {a=$2} END {if (t) printf "mem_total=%.0f\n", t*1024; if (a) printf "mem_avail=%.0f\n", a*1024}' /proc/meminfo 2>/dev/null || true
  if [ "$want_static" = 1 ]; then static_facts; fi
  printf 'arch=%s\nhostname=%s\ncores=%s\n' "$(uname -m)" "$(uname -n)" "$(nproc)"
  printf 'os=%s\n' "$(uname -s | tr '[:upper:]' '[:lower:]')"
  read -r l1 l5 l15 _ </proc/loadavg
  printf 'load1=%s\nload5=%s\nload15=%s\n' "$l1" "$l5" "$l15"
  if [ -d "$root/work" ]; then
    for l in "$root"/work/*/lock; do
      [ -e "$l" ] || continue
      flock -n "$l" true || jobs=$((jobs + 1))
    done
  fi
  printf 'jobs=%s\n' "$jobs"
  if [ "$want_disk" = 1 ]; then
    printf 'disk_used=%s\n' "$(du -sb "$root" 2>/dev/null | cut -f1 || true)"
    printf 'disk_free=%s\n' "$(df -B1 --output=avail "$HOME" | tail -1 | tr -d ' ')"
    if [ -n "$budget" ]; then
      printf 'disk_max=%s\ndisk_min_free=%s\n' "$(budget_max "$HOME" "${budget%%:*}")" "${budget#*:}"
    fi
  fi
}

# A work dir with no lock file yet and younger than this (seconds) is never removed by gc.
WORK_GRACE=120

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
  for l in "${locks[@]}"; do
    [ -e "$l" ] || continue
    # Append, never truncate: opening for write would refresh a slot lock's
    # mtime, which is the slot's last-use stamp (evict_slot).
    eval "exec $fd>>\"\$l\""
    if ! flock -n "$fd"; then action=busy; fi
    fd=$((fd + 1))
  done
  age=$(age_of "$dir" "$now")
  if [ "$action" = keep ] && [ "$age" -ge "$ttl" ]; then action=$verb; fi
  # A run creates its work dir in one ssh call and its lock in the next, so
  # for a moment the dir is unlocked and has no lock file yet. Never remove
  # such a young dir, not even with --all (a finished run always has the file).
  if [ "$kind" = work ] && [ "$action" = "$verb" ] && [ ! -e "$dir/lock" ] && [ "$age" -lt "$WORK_GRACE" ]; then action=keep; fi
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
  if ! flock -n 20 || [ ! "$lock" -ef /proc/self/fd/20 ]; then
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

# evict ROOT NOW MODE REPO MAX_DISK MIN_FREE [log]: when goway's root is over
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
  if [ $((${total:-0} / 4)) -lt "$minfree" ]; then minfree=$((${total:-0} / 4)); fi
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
        case "$k" in "" | *[!0-9]*) continue ;; esac
        printf '%s\t0\tslot\t%s\t%s\n' "$(stat -c %Y "$l" 2>/dev/null || echo "$now")" "$d" "$k"
      done
    done | sort -n -k1,1 -k2,2
  )
  while IFS=$'\t' read -r m rank kind path k; do
    [ -n "$kind" ] || continue
    [ "$freed" -lt "$need" ] || break
    case "$GC_GONE" in *"|$path|"*) continue ;; esac
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
}

# doctor ROOT: key=value facts about the toolchain and host for `goway doctor`.
doctor() {
  local t v pa
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; printf 'cargo_env=yes\n'; else printf 'cargo_env=no\n'; fi
  for t in bash git tar flock setsid cc curl rustup cargo cargo-nextest sccache apt-get dnf pacman; do
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
  printf 'os=%s\n' "$(. /etc/os-release 2>/dev/null; printf '%s' "${PRETTY_NAME:-unknown}")"
  printf 'arch=%s\n' "$(uname -m)"
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
  static_facts
  # Whether a cargo home existed before any goway fix (so uninstall never removes it).
  if [ -e "${CARGO_HOME:-$HOME/.cargo}" ]; then printf 'cargo_home=1\n'; else printf 'cargo_home=0\n'; fi
}

# purge ROOT: remove all of goway's state on this host (goway uninstall).
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
  for s in "$root"/cache/*/sccache.sock; do
    [ -S "$s" ] || continue
    SCCACHE_SERVER_UDS="$s" sccache --stop-server >/dev/null 2>&1 || true
  done
  # Only goway's own entries: a root that also holds foreign files keeps them.
  rm -rf "$root/work" "$root/seed" "$root/cache" "$root/gpu" "$root/gc.lock" "$root/evicted.log"
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
  gc) gc "$@" ;;
  doctor) doctor "$@" ;;
  purge) purge "$@" ;;
  ping) printf 'goway-remote ok\n' ;;
  *) die "unknown verb: $verb" ;;
esac
