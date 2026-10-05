+++
id = "01M44TCANTN1ZSR2RGN8RVD8CV"
title = "The probe should report running jobs per repository so a repository with no peak is held only behind its own runs"
type = "task"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-05T01:20:47Z"
updated = "2026-10-05T01:20:47Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/pool.rs", "crates/goway/src/footprint.rs"]
+++

Found while working ~9SKK7DR: with no peak on any host a repository is admitted only to a host running no goway jobs, because the probe counts jobs without saying which repository they belong to. A per-repository running count would hold it only behind its own runs.
