+++
id = "01M4A8H1DWYN24DXSPMK60F05J"
title = "CPU-aware placement sends frob-v2 cargo dev ci to the small helper, where rustdoc and the linker are OOM-killed"
type = "bug"
category = "in-progress"
priority = "high"
points = 5
reporter = "lognd"
created = "2026-10-07T04:04:13Z"
updated = "2026-10-09T05:31:54Z"
scope = ["crates/goway/src/**", "crates/goway/tests/**"]

[[acceptance]]
text = "a run whose recorded peak does not fit a small idle helper goes to the bigger busy one (test)"
bound = true

[[acceptance]]
text = "after an OOM kill on a host, the next identical run is not placed there (test)"
bound = false

[[acceptance]]
text = "a helper whose total memory is below the repository's recorded peak gets the largest job count whose modelled peak (fixed part plus per-job part, learned from the recorded levels) fits, and is refused only when one job cannot fit (test)"
bound = false

[[acceptance]]
text = "a run cut short because its client left records no memory peak (test)"
bound = false
+++

Measured 2026-10-06 (frob-v2 tickets ~0WQ67AJ, ~PBJ6GPZ): after ~EW8738V made placement prefer an idle smaller helper over a busy big one, frob-v2 cargo dev ci runs went to the 3.8 GiB helper and were killed by signal 9 in rustdoc and the linker; agents had to pin --host to the 12-core helper. The memory estimate either does not cover rustdoc and link peaks of a cargo alias run (cargo dev ci spawns several cargo commands in sequence) or the per-level records are too low after the gc. Placement must never prefer a host whose estimated peak plus margin does not fit, and an OOM kill on a host must raise that host's estimate for the repository so the next run goes elsewhere.
