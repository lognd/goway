+++
id = "01M42BHS18MF66ZQZ69PSR953W"
title = "Keep a helper awake while a job runs (systemd-inhibit on Linux, a keep-awake request on Windows, caffeinate on macOS)"
type = "story"
category = "todo"
priority = "high"
points = 3
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:08Z"
updated = "2026-10-04T02:36:27Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/inhibit.rs", "docs/usage.md"]

[[acceptance]]
text = "Given a laptop helper that would suspend on idle, when a goway job runs there, then the job holds a sleep inhibitor for exactly its lifetime (systemd-inhibit --what=sleep:idle where available, SetThreadExecutionState on Windows, caffeinate on macOS), released however the run ends, and a helper that suspends anyway is reported as asleep, not as a failed command"
bound = false
+++
