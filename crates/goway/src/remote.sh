# goway remote side. Sent inline with every ssh call and run as
#   bash -c "<this script>" goway VERB ARGS...
# so the remote needs nothing installed beyond bash, GNU findutils, tar,
# coreutils and util-linux (flock). Every directory goway owns carries a
# meta.json label and a lock file that is flock-held while in use.
set -euo pipefail
umask 077

die() { printf 'goway-remote: %s\n' "$*" >&2; exit 125; }

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

verb=${1:-}
[ -n "$verb" ] || die "no verb"
shift
case "$verb" in
  manifest) manifest "$@" ;;
  receive) receive "$@" ;;
  ping) printf 'goway-remote ok\n' ;;
  *) die "unknown verb: $verb" ;;
esac
