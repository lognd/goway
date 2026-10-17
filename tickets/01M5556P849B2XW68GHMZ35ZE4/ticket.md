+++
id = "01M5556P849B2XW68GHMZ35ZE4"
title = "render_misses lacks the Windows hint"
type = "task"
category = "done"
outcome = "done"
priority = "low"
points = 1
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T14:45:38Z"
updated = "2026-10-17T20:22:27Z"
idempotency_key = "gap-misses"
labels = ["circles"]
scope = ["crates/goway/src/error.rs"]

[[acceptance]]
text = "Given render_misses on Windows, when a tool is missing, then the Windows hint is shown"
bound = true
+++
