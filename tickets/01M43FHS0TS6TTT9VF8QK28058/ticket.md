+++
id = "01M43FHS0TS6TTT9VF8QK28058"
title = "queue test: waiters never wait when the stagger is wider than the jobs, so the wait-note assertion fails"
type = "bug"
category = "in-progress"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T12:52:17Z"
updated = "2026-10-04T12:52:21Z"
scope = ["crates/goway/tests/queue.rs"]

[[acceptance]]
text = "Given 20 runs arriving 20ms apart and jobs that last 500ms, when the queue test runs 20 times under load, then it passes every time"
bound = false
+++

found in release verification on a helper: noted was 0. The product notes at once whenever a run waits; with 60ms starts and 150ms jobs on 6 slots nobody waits. Hold jobs longer than the arrival span.
