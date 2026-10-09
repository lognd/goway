+++
id = "01M4GMWH31KWNZ5VHK9D4HTVG7"
title = "A helper's own recorded job levels are not overridden by other helpers' levels when fitting a cargo run"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-09T15:35:39Z"
updated = "2026-10-09T15:38:30Z"
scope = ["crates/goway/src/**", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "a helper with its own recorded levels for a repository is fitted from them alone, other helpers' levels fill in only when it has none (test)"
bound = true
+++

found while proving K60F05J: the big helper's levels for the repository include whole-alias ci runs (up to 7 GiB at one job) while the small helper measured 2.7 GiB at 4 jobs and 3.2 GiB at 7 for nextest; the shared levels forced a 7 GiB fixed part onto the small helper and refused it.
