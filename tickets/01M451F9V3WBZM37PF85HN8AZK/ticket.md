+++
id = "01M451F9V3WBZM37PF85HN8AZK"
title = "A whole-workspace test run under the default job cap exhausts the task limit: every goway client starts one worker thread per core"
type = "bug"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-05T03:24:44Z"
updated = "2026-10-05T03:24:44Z"
scope = ["crates/goway/src/main.rs"]

[[acceptance]]
text = "Given a many-core helper, when goway clients run concurrently, then each client's thread count does not grow with the machine's core count"
bound = false
+++

Found while landing the CI fixes: cargo nextest run --workspace inside a goway job on a 32-core helper reaches the 4096 task cap (ulimit -u fallback and TasksMax count threads): about 80 concurrent goway test clients show 32 tokio-rt-worker threads each, then fork and thread spawns fail with EAGAIN (45 to 190 failures, fork: retry: Resource temporarily unavailable). The client is I/O bound: bound its worker threads, or document the cap. Owner decision on the default job_tasks may be needed.
