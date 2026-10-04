+++
id = "01M4256TBME68EPWC1PWXVMJKT"
title = "Remote watchdog stops a job whose client vanished without a hangup (heartbeat)"
type = "bug"
category = "in-progress"
priority = "high"
reporter = "lognd"
created = "2026-10-04T00:32:17Z"
updated = "2026-10-04T05:33:30Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/tests/client_loss.rs", "docs/design.md"]

[[acceptance]]
text = "Given a running job whose client disappears without the connection signalling a hangup (the client process is killed, the laptop sleeps, or the network drops), when the helper's watchdog notices the client is gone (EOF on a heartbeat stream from the client, or no heartbeat for a bounded time), then the job's process group is stopped, its slot and locks are released and its work directory is cleaned as for an interrupted run; a test kills the client with SIGKILL and asserts the job is gone within the bound"
bound = true
+++

Split from ~DFXCR9H (its criterion 1) so the test drop guards could land without holding remote.sh. Evidence: 96 orphaned pollers kept a helper at 3 cores for 20 minutes on 2026-10-03.
