# goway remote side. Sent inline with every ssh call and run as
#   bash -c "<this script>" goway VERB ARGS...
# so the remote needs nothing installed beyond bash, GNU findutils, tar,
# coreutils and util-linux (flock). Every directory goway owns carries a
# meta.json label and a lock file that is flock-held while in use.
set -euo pipefail
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
      work | seed | cache | .goway-root) ;;
      *) die "$1 exists, is not empty and is not goway state; pick a dedicated remote_root" ;;
    esac
  done
  printf 'goway state; safe to delete with goway gc --all\n' >"$1/.goway-root"
}

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
  exec 6>"$sib/lock"
  flock -s 6
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
  mkdir -p "$seed"
  exec 8>"$seed/lock"
  if [ ! -d "$seed/tree" ]; then
    flock -x 8
    [ -d "$seed/tree" ] || seed_from_sibling "$seed"
  fi
  flock -s 8
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
  exec 8>"$seed/lock"
  flock -s 8
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
  mkdir -p "$seed"
  exec 8>"$seed/lock"
  flock -x 8
  cat >"$seed/deletions.$3"
}

# receive ROOT SEED META_B64 GENERATION RUN_ID WORK_META_B64 KEEP ATTEMPT:
# under the seed's exclusive lock, check that the seed is still the one the
# manifest described (GENERATION, empty for "no tree yet"), apply pending
# deletions, extract the tar on stdin, and (when RUN_ID is given) snapshot
# the tree into the run's fresh work dir in the same critical section.
# Files are replaced by unlink and recreate, so hard-linked snapshots keep
# their content. Exit 75 with "seed changed" when the generation differs.
receive() {
  local root seed gen work
  root=$(root_dir "$1"); seed="$root/seed/$2"
  mark_root "$root"
  mkdir -p "$seed"
  exec 8>"$seed/lock"
  flock -x 8
  gen=$(cat "$seed/generation" 2>/dev/null || true)
  if [ ! -d "$seed/tree" ]; then gen=""; fi
  local attempt=${8:-}
  case "$attempt" in *[!A-Za-z0-9-]*) die "receive: bad attempt id" ;; esac
  if [ "$gen" != "$4" ]; then
    rm -f "$seed"/deletions.*
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
  tar -x --unlink-first --recursive-unlink --no-same-owner -C "$seed/tree" -f -
  find "$seed/tree" -mindepth 1 -depth -type d -empty -delete
  if [ -n "${5:-}" ]; then
    work="$root/work/$5"
    mkdir -p "$work"
    printf '%s' "$6" | base64 -d >"$work/meta.json"
    if [ "${7:-0}" = 1 ]; then : >"$work/keep"; fi
    # A real copy (reflinked where the filesystem can): a job that
    # writes a file in place must never change the seed or other runs.
    cp -a --reflink=auto "$seed/tree" "$work/tree"
  fi
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

# run ROOT RUN_ID REPO_ID KEEP SLOTS CACHE_META_B64 TTLS PRIORITY -- CMD...
# The work dir was created by receive (snapshot of the seed).
# TTLS is "cache:orphan:kept" in seconds, for the automatic gc afterwards.
# Snapshot the seed into a fresh work dir, pick a free cargo target slot,
# run CMD in its own process group with stdio passed through, clean up,
# and exit with CMD's status (128+N when killed by signal N).
run() {
  local root work cache slot="" k rc=0 wd rundir
  root=$(root_dir "$1"); work="$root/work/$2"; cache="$root/cache/$3"
  local root_arg=$1 run_id=$2 repo_id=$3 keep=$4 slots=$5 cache_meta=$6
  local ttls=$7 priority=$8 nicer=()
  shift 8
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "run: no command"
  [ -d "$work/tree" ] || die "run: no work dir at $work (was it synced?)"
  rundir="$work/tree"

  mark_root "$root"
  mkdir -p "$cache"
  exec 9>"$work/lock"
  flock -x 9

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
  if [ -z "${CARGO_TARGET_DIR:-}" ]; then
    for ((k = 0; k < slots; k++)); do
      exec 7>"$cache/target-$k.lock"
      if flock -n 7; then slot=$k; break; fi
      exec 7>&-
    done
    if [ -z "$slot" ]; then
      slot=$((RANDOM % slots))
      printf 'goway: all %s cargo target slots busy; waiting for slot %s\n' "$slots" "$slot" >&2
      exec 7>"$cache/target-$slot.lock"
      flock 7
    fi
    export CARGO_TARGET_DIR="$cache/target-$slot"
    # Builds bake absolute source paths into binaries (CARGO_MANIFEST_DIR,
    # file!()), and cargo reuses them when only the workspace moved. So a
    # slot's binaries always run against a tree at the same path: the
    # snapshot moves to tree-<slot> (a rename) for the length of the run.
    rundir="$cache/tree-$slot"
    rm -rf "$rundir"
    mv "$work/tree" "$rundir"
  fi
  if [ -z "${RUSTC_WRAPPER+set}" ] && command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER=sccache
    export SCCACHE_DIR="${SCCACHE_DIR:-$cache/sccache}"
    # A unix socket in the owner-only cache dir: no TCP port another user
    # on the host could reach or squat (unless the user chose an endpoint).
    if [ -z "${SCCACHE_SERVER_PORT:-}" ] && [ -z "${SCCACHE_SERVER_UDS:-}" ]; then
      export SCCACHE_SERVER_UDS="$cache/sccache.sock"
    fi
    # sccache's server is the one process allowed to outlive a run (it
    # keeps the cache warm); make it leave soon after the last build.
    export SCCACHE_IDLE_TIMEOUT="${SCCACHE_IDLE_TIMEOUT:-300}"
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
  watchdog "$PPID" "$work/pid" </dev/null >/dev/null 2>&1 7>&- 9>&- &
  wd=$!
  # Foreground (not `&`): background jobs of a non-interactive shell start
  # with SIGINT and SIGQUIT ignored, and the command must not inherit that.
  # A polite guest on someone's laptop: low CPU and idle-class I/O.
  if [ "$priority" = low ]; then
    if command -v nice >/dev/null 2>&1; then nicer+=(nice -n 10); fi
    if command -v ionice >/dev/null 2>&1; then nicer+=(ionice -c 3); fi
  fi
  setsid sh -c 'echo $$ >"$0"; exec "$@"' "$work/pid" "${nicer[@]}" "$@" || rc=$?
  kill "$wd" 2>/dev/null || true
  cd "$root"
  if [ "$rundir" != "$work/tree" ]; then
    if [ "$keep" = 1 ]; then mv "$rundir" "$work/tree"; else rm -rf "$rundir"; fi
  fi
  if [ "$keep" != 1 ]; then rm -rf "$work"; fi
  # Cheap automatic gc of expired entries, detached so it never delays
  # the exit (and never holds the ssh session open).
  if [ -n "$ttls" ]; then
    IFS=: read -r t_cache t_orphan t_kept <<<"$ttls"
    (trap '' HUP; gc "$root_arg" "$(date +%s)" "$t_cache" "$t_orphan" "$t_kept" apply "" "") \
      </dev/null >/dev/null 2>&1 7>&- 9>&- &
  fi
  exit "$rc"
}

# probe ROOT [disk]: key=value facts for scheduling and status.
probe() {
  local root jobs=0 l
  root=$(root_dir "$1")
  printf 'arch=%s\nhostname=%s\ncores=%s\n' "$(uname -m)" "$(uname -n)" "$(nproc)"
  read -r l1 l5 l15 _ </proc/loadavg
  printf 'load1=%s\nload5=%s\nload15=%s\n' "$l1" "$l5" "$l15"
  if [ -d "$root/work" ]; then
    for l in "$root"/work/*/lock; do
      [ -e "$l" ] || continue
      flock -n "$l" true || jobs=$((jobs + 1))
    done
  fi
  printf 'jobs=%s\n' "$jobs"
  if [ "${2:-}" = disk ]; then
    printf 'disk_used=%s\n' "$(du -sb "$root" 2>/dev/null | cut -f1 || true)"
    printf 'disk_free=%s\n' "$(df -B1 --output=avail "$HOME" | tail -1 | tr -d ' ')"
  fi
}

# Seconds since the last use of DIR (its meta.json mtime).
age_of() {
  local m
  m=$(stat -c %Y "$1/meta.json" 2>/dev/null || stat -c %Y "$1")
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

# Decide one entry: print "action TAB kind TAB age TAB bytes TAB repo TAB id TAB path"
# and remove it when the action is "remove" and MODE is apply. The entry's
# locks are taken exclusively (non-blocking) while it is removed.
gc_entry() {
  local kind=$1 dir=$2 ttl=$3 now=$4 mode=$5 repo_filter=$6 action age bytes repo locks=() l fd
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
    eval "exec $fd>\"\$l\""
    if ! flock -n "$fd"; then action=busy; fi
    fd=$((fd + 1))
  done
  age=$(age_of "$dir" "$now")
  if [ "$action" = keep ] && [ "$age" -ge "$ttl" ]; then action=remove; fi
  bytes=$(du -sb "$dir" 2>/dev/null | cut -f1 || echo 0)
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$action" "$kind" "$age" "${bytes:-0}" "$repo" "$dir"
  if [ "$action" = remove ] && [ "$mode" = apply ]; then
    rm -rf "$dir"
  fi
  while [ "$fd" -gt 20 ]; do fd=$((fd - 1)); eval "exec $fd>&-"; done
}

# gc ROOT NOW CACHE_TTL ORPHAN_TTL KEPT_TTL MODE REPO OLDER_THAN
# TTLs in seconds; OLDER_THAN (seconds, or empty) replaces every TTL.
gc() {
  local root now cache_ttl orphan_ttl kept_ttl mode repo older d ttl
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
  pa=$(grep -rhiE '^\s*PasswordAuthentication\s' /etc/ssh/sshd_config.d/ /etc/ssh/sshd_config 2>/dev/null | head -1 | awk '{print tolower($2)}' || true)
  printf 'password_auth=%s\n' "${pa:-default-yes}"
  printf 'home=%s\n' "$HOME"
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
  rm -rf "$root/work" "$root/seed" "$root/cache"
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
  run) run "$@" ;;
  envfile) envfile "$@" ;;
  probe) probe "$@" ;;
  gc) gc "$@" ;;
  doctor) doctor "$@" ;;
  purge) purge "$@" ;;
  ping) printf 'goway-remote ok\n' ;;
  *) die "unknown verb: $verb" ;;
esac
