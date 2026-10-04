+++
id = "01M43Z0NPW4WH9YNZGXAW0DHW0"
title = "Memory footprint can lock a repository out of every helper: inflated peaks, no override, pinned hosts refused"
type = "bug"
category = "in-progress"
priority = "critical"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T17:22:33Z"
updated = "2026-10-04T17:34:50Z"
scope = ["crates/goway/src/footprint.rs", "crates/goway/src/pool.rs", "crates/goway/src/remote.sh", "crates/goway/src/cli.rs", "crates/goway/tests/mem_footprint.rs", "docs/usage.md", "crates/goway/src/needs.rs", "crates/goway/src/lib.rs", "crates/goway/src/run.rs", "crates/goway/src/project.rs", "crates/goway/src/status.rs", "crates/goway/src/doctor.rs"]

[[acceptance]]
text = "Given a recorded memory peak above every helper's total (found live: goway's own suite recorded about 7.3 GiB, refused on a 7.6 GiB helper with margin, and no host could run it), when goway picks hosts, then the run is never locked out: --host always runs there (with a warning naming the recorded peak), a new --ignore-footprint (or --needs mem>=0) skips the check for one run, and when no helper could ever fit the peak goway runs on the largest eligible helper with a warning instead of refusing"
bound = true

[[acceptance]]
text = "Given peak measurement, when it records, then it does not over-count: cgroup memory.peak is preferred; the sampling fallback uses PSS (smaps_rollup) or the job tree's unique RSS, not a sum that double-counts shared pages; a killed run's inflated record decays; and the stored peak is the maximum of the last few runs, not all time, so one bad run cannot exclude a repository forever"
bound = false

[[acceptance]]
text = "Given goway status or doctor, when a repository has recorded peaks, then they are shown per helper with how to reset them (goway gc --repo or a documented command)"
bound = false
+++
