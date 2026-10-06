+++
id = "01M47SYH8F5YD6P5G2741CG5RF"
title = "Fit cargo and nextest runs to the host's memory by lowering parallelism instead of refusing or OOMing"
type = "story"
category = "in-progress"
priority = "high"
points = 8
reporter = "lognd"
created = "2026-10-06T05:10:58Z"
updated = "2026-10-06T05:23:39Z"
scope = ["crates/goway/**", "docs/**", "changelog.d/**"]

[[acceptance]]
text = "Given a host whose free memory is below the recorded peak with margin, when a cargo run is placed, then goway lowers CARGO_BUILD_JOBS and NEXTEST_TEST_THREADS to the largest level whose estimated peak fits and says so on stderr"
bound = false

[[acceptance]]
text = "Given even one job cannot fit the host's total memory, when a cargo run is placed, then it is refused"
bound = false

[[acceptance]]
text = "Given an explicit CARGO_BUILD_JOBS or -j, when the run is placed, then it is never overridden"
bound = false

[[acceptance]]
text = "Given a run in flight on a host whose memory has not yet ramped, when another run is admitted, then the first run's unused reservation is subtracted from the host's available memory"
bound = false

[[acceptance]]
text = "Given a run finished at a parallelism level, then its peak is recorded for that level and later estimates use it"
bound = false
+++

Measured 2026-10-06 from a large Rust workspace: every run refused (7.6 GiB host, recorded peak 7.0 GiB with margin 7.7) and an admitted run was OOM-killed in clang. Lower CARGO_BUILD_JOBS / NEXTEST_TEST_THREADS to fit, record peaks per level, reserve memory for in-flight jobs, never override an explicit CARGO_BUILD_JOBS.
