+++
id = "01M42HH9DKKMRPPWNY1EX16KQS"
title = "A run notes fleet drift for the project tools and records the tool versions of the host that ran it"
type = "story"
category = "in-progress"
priority = "high"
points = 3
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-04T04:07:43Z"
updated = "2026-10-04T05:44:03Z"
scope = ["crates/goway/src/run.rs", "crates/goway/src/drift.rs", "crates/goway/src/render.rs", "crates/goway/tests/drift.rs", "docs/usage.md"]

[[links]]
kind = "blocked-by"
target = "01M41Y9H462DH2ZJRC43T7FA5K"

[[acceptance]]
text = "Given a run, when the chosen host's cached tool versions (written by goway doctor) differ from the rest of the fleet for a tool the project uses, then goway prints a one-line drift note, and the run report (and --report JSON) records the versions of the project's tools on that host with the time they were captured, so frob evidence says what built and tested it"
bound = false
+++

Criterion 3 of ~3T7FA5K, split because run.rs was leased. Versions come from the cache doctor writes into goway state; a run never probes for them.
