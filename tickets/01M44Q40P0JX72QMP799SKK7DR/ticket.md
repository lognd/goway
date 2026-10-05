+++
id = "01M44Q40P0JX72QMP799SKK7DR"
title = "The wave queue starves behind a head that cannot fit, and a host with no recorded peak admits runs that exhaust its memory"
type = "bug"
category = "in-progress"
priority = "critical"
points = 5
reporter = "lognd"
created = "2026-10-05T00:23:49Z"
updated = "2026-10-05T00:47:57Z"
scope = ["crates/goway/src/queue.rs", "crates/goway/src/footprint.rs", "crates/goway/src/pool.rs", "crates/goway/tests/queue.rs", "docs/troubleshooting.md", "crates/goway/tests/gpu_slots.rs", "crates/goway/tests/slot_trees.rs", "crates/goway/tests/run_local.rs", "crates/goway/tests/nesting.rs", "crates/goway/tests/mem_footprint.rs"]

[[acceptance]]
text = "Given a queue head that does not fit on any host, When a later entry fits (or skips the footprint check), Then the later entry is dispatched, and the head keeps its place so it is not starved forever (bounded overtaking)"
bound = false

[[acceptance]]
text = "Given a host running no goway jobs and a repository whose recorded peak exceeds the host's free memory but not its total, When that repository's run is at the head, Then it runs alone on that host with a warning instead of waiting for memory that will never free"
bound = false

[[acceptance]]
text = "Given a repository whose peak exceeds a host's total memory, When it is queued, Then it fails fast with a message naming the peak and the host's size, instead of waiting out --wait"
bound = false

[[acceptance]]
text = "Given a host with no recorded peak for a repository but a peak recorded on another host, When admission is decided, Then the other host's peak is used as the estimate (so a fresh or restarted host is not flooded)"
bound = false

[[acceptance]]
text = "Given a host with no recorded peak anywhere for a repository, When several runs of it are queued, Then only one is admitted to that host until its peak is known"
bound = false
+++

Observed 2026-10-04: six runs queued for up to 58 minutes while the only usable helper sat idle at 0/12 jobs. The head's repository had a recorded 6.8 GiB peak (7.0 GiB with margin) against 6.9 GiB free, so it never fit; FIFO kept every entry behind it waiting, including two with --ignore-footprint. After forgetting the peak, three runs of that repository started at once on the 7.6 GiB host. The same missing-record admission probably hung another helper outright: after its WSL restart it had no peaks on record, took several concurrent agent runs in 5.4 GB and Windows itself stopped answering ssh.
