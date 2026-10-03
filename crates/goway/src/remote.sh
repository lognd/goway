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

# manifest ROOT SEED: print the seed tree as NUL-terminated records
#   type TAB size TAB mtime TAB mode TAB linktarget TAB path
manifest() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  [ -d "$seed/tree" ] || return 0
  exec 8>"$seed/lock"
  flock -s 8
  find "$seed/tree" -mindepth 1 \( -type f -o -type l \) \
    -printf '%y\t%s\t%T@\t%m\t%l\t%P\0'
}

# receive ROOT SEED META_B64 DELETES_B64: apply deletions, then extract
# the tar on stdin into the seed tree. Files are replaced by unlink and
# recreate, so hard-linked work directories keep their snapshot.
receive() {
  local root seed
  root=$(root_dir "$1"); seed="$root/seed/$2"
  mkdir -p "$seed/tree"
  exec 8>"$seed/lock"
  flock -x 8
  printf '%s' "$3" | base64 -d >"$seed/meta.json"
  if [ -n "$4" ]; then
    (cd "$seed/tree" && printf '%s' "$4" | base64 -d | xargs -0 -r rm -f --)
  fi
  tar -x --unlink-first --recursive-unlink --no-same-owner -C "$seed/tree" -f -
  find "$seed/tree" -mindepth 1 -depth -type d -empty -delete
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

# run ROOT SEED RUN_ID REPO_ID KEEP SLOTS META_B64 CACHE_META_B64 ENV_B64 TTLS PRIORITY -- CMD...
# TTLS is "cache:orphan:kept" in seconds, for the automatic gc afterwards.
# Snapshot the seed into a fresh work dir, pick a free cargo target slot,
# run CMD in its own process group with stdio passed through, clean up,
# and exit with CMD's status (128+N when killed by signal N).
run() {
  local root seed work cache slot="" k rc=0 wd
  root=$(root_dir "$1"); seed="$root/seed/$2"; work="$root/work/$3"
  cache="$root/cache/$4"
  local root_arg=$1 run_id=$3 repo_id=$4 keep=$5 slots=$6 meta=$7 cache_meta=$8 envb=$9
  local ttls=${10} priority=${11} nicer=()
  shift 11
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "run: no command"
  [ -d "$seed/tree" ] || die "run: no synced tree at $seed"

  mkdir -p "$work" "$cache"
  exec 9>"$work/lock"
  flock -x 9
  printf '%s' "$meta" | base64 -d >"$work/meta.json"
  if [ "$keep" = 1 ]; then : >"$work/keep"; fi

  exec 8>"$seed/lock"
  flock -s 8
  cp -al "$seed/tree" "$work/tree"
  touch "$seed/meta.json"
  exec 8>&-

  [ -f "$cache/meta.json" ] || printf '%s' "$cache_meta" | base64 -d >"$cache/meta.json"
  touch "$cache/meta.json"

  # Settings that already exist win over goway's defaults: first the
  # remote environment and ~/.cargo/env, then the user's --env values;
  # goway only fills in what is still unset.
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
  if [ -n "$envb" ]; then
    while IFS= read -r -d '' kv; do export "$kv"; done < <(printf '%s' "$envb" | base64 -d)
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
  fi
  if [ -z "${RUSTC_WRAPPER+set}" ] && command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER=sccache
    export SCCACHE_DIR="${SCCACHE_DIR:-$cache/sccache}"
    export SCCACHE_SERVER_PORT="${SCCACHE_SERVER_PORT:-$((4300 + 16#${repo_id:0:4} % 1000))}"
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
  cd "$work/tree"
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
  age=$(age_of "$dir" "$now")
  case "$kind" in
    cache) for l in "$dir"/target-*.lock; do [ -e "$l" ] && locks+=("$l"); done ;;
    *) locks=("$dir/lock") ;;
  esac
  action=keep
  if [ "$age" -ge "$ttl" ]; then action=remove; fi
  # Hold every lock of the entry while deciding and removing.
  fd=20
  for l in "${locks[@]}"; do
    [ -e "$l" ] || continue
    eval "exec $fd>\"\$l\""
    if ! flock -n "$fd"; then action=busy; fi
    fd=$((fd + 1))
  done
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

verb=${1:-}
[ -n "$verb" ] || die "no verb"
shift
case "$verb" in
  manifest) manifest "$@" ;;
  receive) receive "$@" ;;
  run) run "$@" ;;
  probe) probe "$@" ;;
  gc) gc "$@" ;;
  doctor) doctor "$@" ;;
  ping) printf 'goway-remote ok\n' ;;
  *) die "unknown verb: $verb" ;;
esac
