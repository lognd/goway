+++
id = "01M42TD5V6H043JYBGK591BBA2"
title = "Wire the measured clock offset into pool probe_one"
type = "task"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T06:42:46Z"
updated = "2026-10-04T11:23:21Z"
scope = ["crates/goway/src/pool.rs"]

[[acceptance]]
text = "Given a probe_one call, when the host reports epoch, then Probe.facts.clock_offset_ms is the measured offset"
bound = true
+++

Split from ~TGZTRVF because pool.rs was leased. probe_one times the probe and calls clock::measure.
