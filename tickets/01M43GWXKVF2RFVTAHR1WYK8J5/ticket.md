+++
id = "01M43GWXKVF2RFVTAHR1WYK8J5"
title = "macOS: measure a job's memory peak (BSD ps has no session id column, so nothing was sampled)"
type = "bug"
category = "todo"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T13:15:50Z"
updated = "2026-10-04T13:15:50Z"
scope = ["crates/goway/src/remote.sh", "docs/usage.md"]

[[acceptance]]
text = "Given a macOS helper, When a job runs, Then its sampled peak resident memory is recorded like on Linux"
bound = false
+++

mem_sample skips sampling on Darwin; mem_footprint tests fail on macOS CI (no mempeak record). The job is a session and process-group leader, so ps pgid works on BSD. Found while working D6024D0.
