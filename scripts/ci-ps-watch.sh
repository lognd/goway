#!/bin/sh
# Background watcher for CI test runs: every 30s, when a test binary, a
# goway it started or a caffeinate has run for 45s or more (a stuck test),
# print the process table and the open pipes of those processes, so a hang
# leaves its evidence in the log before nextest's timeout kills it. Stops
# after six dumps; the job step kills it when the tests end.
n=0
while [ "$n" -lt 6 ]; do
  sleep 30
  old=$(ps -axo pid=,etime=,command= | awk '
    /target\/debug|caffeinate/ && !/ps-watch/ {
      t = $2; sub(/^[0-9]+-/, "", t); m = split(t, p, ":")
      s = (m == 3) ? p[1] * 3600 + p[2] * 60 + p[3] : p[1] * 60 + p[2]
      if (s >= 45) print $1 }' | tr '\n' ' ')
  [ -n "$old" ] || continue
  n=$((n + 1))
  echo "::group::process dump $n ($(date +%T)): stuck pids $old"
  ps -axo pid,ppid,pgid,etime,stat,command | grep -E "target/debug|nextest|caffeinate|sccache|ssh|sleep|bash|sh -c|PID" | grep -v "ps-watch\|grep -E" | cut -c1-260
  for p in $old; do
    echo "--- open files of $p"
    lsof -nP -p "$p" 2>/dev/null | grep -E "PIPE|FIFO|CHR|REG|IPv|unix" | cut -c1-200
  done
  echo "::endgroup::"
done
