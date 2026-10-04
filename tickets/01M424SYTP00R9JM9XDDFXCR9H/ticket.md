+++
id = "01M424SYTP00R9JM9XDDFXCR9H"
title = "A remote job outlives a client that vanished without a hangup; GPU-slot tests leak busy pollers when they fail"
type = "bug"
category = "todo"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:25:16Z"
updated = "2026-10-04T00:25:16Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/remote.ps1", "crates/goway/src/run.rs", "crates/goway/tests/gpu_slots.rs", "crates/goway/tests/common/mod.rs", "crates/goway/tests/client_loss.rs", "docs/design.md"]

[[acceptance]]
text = "Given a running job whose client disappears without the connection signalling a hangup (the client process is killed, the laptop sleeps, or the network drops), when the helper's watchdog notices the client is gone (EOF on a heartbeat stream from the client, or no heartbeat for a bounded time), then the job's process group is stopped, its slot and locks are released and its work directory is cleaned as for an interrupted run; a test kills the client with SIGKILL and asserts the job is gone within the bound"
bound = false

[[acceptance]]
text = "Given the GPU-slot and other tests that hold a run open until a release file appears, when such a test fails, panics or is cancelled, then a drop guard releases or kills what it started, so no poller survives the test (on 2026-10-03 96 orphaned pollers kept a helper at 3 cores for 20 minutes)"
bound = false
+++
