+++
id = "01M48MPBRZBVGBP1JF5EW8738V"
title = "Fit runs to host CPU as well as memory: concurrent cargo runs plus multithreaded mold oversubscribe a helper"
type = "bug"
category = "in-progress"
priority = "high"
reporter = "lognd"
created = "2026-10-06T12:58:22Z"
updated = "2026-10-06T13:01:34Z"
scope = ["crates/goway/src/**", "crates/goway/tests/**", "docs/**"]

[[acceptance]]
text = "two concurrent fitted cargo runs on one host get jobs that sum to about the core count (test)"
bound = false

[[acceptance]]
text = "MOLD_JOBS and the link thread cap are exported unless set by the user (test)"
bound = false

[[acceptance]]
text = "with one helper saturated and another idle and big enough, a new run goes to the idle one (test)"
bound = false
+++

Measured 2026-10-06 on the 12-core helper: 3 concurrent frob-v2 runs (admitted under max_jobs = 12) gave load 107: 19 rustc and 31 linker processes, mostly ld.mold, which threads across every core per link. Memory fitting (~41CG5RF) bounds CARGO_BUILD_JOBS per run but not the sum across runs, and nothing bounds mold. Want: admission and fitting account for CPU across goway's in-flight runs on a host (jobs per run scaled so the total stays near the core count), MOLD_JOBS (concurrent mold processes) and a per-link thread cap exported with the fitted jobs unless the user set them, and a placement score that prefers an idle helper over a saturated one even when it is smaller (while still respecting memory). Report the chosen split on stderr like the memory fit does.
