+++
id = "01M43AFK9N84ZE45CY5T8B9HQV"
title = "A run refreshes a stale tool-version cache from its own probe"
type = "story"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-04T11:23:42Z"
updated = "2026-10-04T11:23:42Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/pool.rs", "crates/goway/src/run.rs", "crates/goway/src/facts.rs", "crates/goway/src/state.rs", "crates/goway/src/doctor.rs", "crates/goway/src/drift.rs", "docs/usage.md", "crates/goway/tests/drift_refresh.rs"]

[[acceptance]]
text = "Given a run in a repository whose cached tool versions for the chosen host are missing or older than a day, when the run probes the host, then the probe asks for the project tools, the cache is refreshed with what it reports, and the drift note and report use it; given a fresh cache the probe asks for nothing extra"
bound = false
+++

Follow-up of ~EX16KQS: drift notes only worked after goway doctor. When the cache for the host and repository is missing or over a day old, the probe a run already makes asks for the project tool versions (probe word tools:A,B) and the run records them.
