+++
id = "01M41TY572561EPG8TRX6SY2NM"
title = "Nested goway is bounded: GOWAY_DEPTH stops goway from recursively spawning itself through the commands it runs"
type = "task"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:32:48Z"
updated = "2026-10-03T22:56:13Z"
scope = ["crates/goway/src/run.rs", "crates/goway/src/remote.sh", "crates/goway/src/lib.rs", "crates/goway/src/error.rs", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a command run by goway (on a helper or locally) that itself calls goway, when the nested goway starts, then it sees GOWAY_DEPTH (set by every goway run to its own depth plus one, also forwarded to the remote job), and at depth 4 it refuses with exit 125 and a message naming the chain, so a command that recursively invokes goway always terminates"
bound = false

[[acceptance]]
text = "Given goway's own background work (auto-gc, the watchdog), when it runs, then it never starts another goway run and holds a lock so at most one auto-gc per host root runs at a time"
bound = false
+++
