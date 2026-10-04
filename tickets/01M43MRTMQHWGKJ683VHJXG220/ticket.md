+++
id = "01M43MRTMQHWGKJ683VHJXG220"
title = "macOS CI: the queue arrival-order test is out of order by up to five places on a loaded runner"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T14:23:30Z"
updated = "2026-10-04T15:29:19Z"
scope = ["crates/goway/tests/queue.rs"]

[[acceptance]]
text = "Given a loaded 3-core runner, When the 20-run wave is queued, Then arrival order holds within the tolerance on macOS and Linux"
bound = true
+++

The 20 ms stagger is within macOS scheduler jitter (observed order [0, 1, 7, 8, 6, 2, ...]). Widen the stagger to 60 ms; jobs still outlast the arrival span (500 ms hold, still waiters). Found while working D6024D0.
