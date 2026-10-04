+++
id = "01M42EM74CSKZX1MHWBZW327DY"
title = "Waves of runs queue instead of failing: wait for a job slot or enough free memory, fairly, with a bounded wait"
type = "story"
category = "in-progress"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T03:16:54Z"
updated = "2026-10-04T06:36:54Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/run.rs", "crates/goway/src/cli.rs", "crates/goway/src/state.rs", "crates/goway/tests/queue.rs", "docs/queue.md", "crates/goway/src/queue.rs", "crates/goway/src/lib.rs", "crates/goway/src/config.rs"]

[[acceptance]]
text = "Given many concurrent goway runs from different worktrees on one laptop (a wave from an agent), when they start together, then they spread over the helpers by score, no helper is given more concurrent jobs than its free memory allows at the default reserve, and the queue never starves an early waiter; a test launches 20 runs against 3 fake hosts and checks order and bounds"
bound = false

[[acceptance]]
text = "Given every eligible host at its job limit or below the run's memory need (--needs mem>=X, or a default per-job memory reserve, configurable), when goway run starts, then it waits in a local first-come-first-served queue (a note says why and its place), re-probing at a bounded interval, up to --wait (default 5 minutes, as the owner asked; --wait 0 keeps today's immediate exit 125), and runs as soon as a host qualifies; when the wait runs out it exits 125 saying how long it waited and what it waited for"
bound = false

[[acceptance]]
text = "Given a helper whose available memory is below the per-job reserve (default 1.5 GiB for any job, configurable per host), when goway picks hosts, then that helper is not given another job until memory frees up (found live on 2026-10-04: a 3.6 GiB helper carried 6 jobs with 0.7 GiB available)"
bound = false
+++
