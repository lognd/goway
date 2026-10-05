+++
id = "01M4521XH97V8EXXK8RV630TRA"
title = "The end-of-run sweep prints environ permission errors and stops the shared sccache server as a leftover on every run"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-05T03:34:54Z"
updated = "2026-10-05T04:12:10Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/client_loss.rs", "docs/design.md"]

[[acceptance]]
text = "Given a run whose only surviving process is the sccache server goway configured, When the run ends, Then no leftover message is printed and the server keeps serving the next run"
bound = true

[[acceptance]]
text = "Given a run that leaves a real stray process, When the run ends, Then it is still stopped and the message names it (pid and command)"
bound = true

[[acceptance]]
text = "Given other users' processes on the helper, When the sweep scans, Then no permission error reaches the run's output"
bound = false
+++

On helpers every run ends with 'goway-remote: the job of run ... left processes behind; stopping them', even passing ones, and 'line 2540: /proc/N/environ: Permission denied' from reading other users' processes. The usual leftover is the sccache server the job started (it daemonizes and inherits GOWAY_RUN_ID); stopping it after every run throws away the warm server and makes the message meaningless. Decide deliberately: either sccache's server is expected and exempt (it is goway's per-repository cache server, with its own idle timeout), or it is started outside the job's scope.
