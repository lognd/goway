#!/bin/sh
# Background watcher for CI test runs: every 30s, when some process has run
# for 45s or more (a test that is stuck, or its goway and helper children),
# print the process table and the open pipes of those processes, so a hang
# leaves its evidence in the log before nextest's timeout kills it. Stops
# after eight dumps; the job step kills it when the tests end.
n=0
while [ "$n" -lt 8 ]; do
  sleep 30
  old=$(ps -axo pid=,etime= | awk '
    { t = $2; d = 0; sub(/^[0-9]+-/, "", t); m = split(t, p, ":")
      s = (m == 3) ? p[1] * 3600 + p[2] * 60 + p[3] : p[1] * 60 + p[2]
      if (s >= 45) print $1 }' | tr '\n' ' ')
  [ -n "$old" ] || continue
  n=$((n + 1))
  echo "::group::process dump $n ($(date +%T))"
  ps -axo pid,ppid,pgid,etime,stat,command | grep -v "ps-watch\|ps -axo" | grep -E "goway|nextest|sh -c|bash|ssh|sleep|caffeinate|sccache|cargo|PID" | cut -c1-300
  for p in $old; do
    case "$(ps -o command= -p "$p" 2>/dev/null)" in
      *goway* | *nextest* | *caffeinate* | *bash*)
        echo "--- open files of $p"
        lsof -nP -p "$p" 2>/dev/null | grep -E "PIPE|FIFO|CHR|REG" | cut -c1-200 ;;
    esac
  done
  echo "::endgroup::"
done
