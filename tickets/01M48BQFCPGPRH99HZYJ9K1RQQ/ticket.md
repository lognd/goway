+++
id = "01M48BQFCPGPRH99HZYJ9K1RQQ"
title = "With no memory record, a cargo run starts at full parallelism and can OOM a small helper"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-06T10:21:41Z"
updated = "2026-10-17T10:54:38Z"
scope = ["crates/goway/src/fit.rs", "crates/goway/tests/**"]

[[acceptance]]
text = "a cargo run with no recorded peak on a small helper starts with lowered jobs and says why"
bound = true

[[acceptance]]
text = "once a record exists, the recorded estimate replaces the cold-start one"
bound = true
+++

Measured 2026-10-06 from frob-v2: after goway gc --repo cleared an inflated peak, the first cargo dev ci on a 3.8 GiB helper ran at full jobs and its linker was OOM-killed (clang Killed). fit::estimate returns None without a record, so Mode::Adaptive lowers nothing. Use a conservative cold-start estimate when nothing is recorded (for example a per-job default derived from host memory, or a fixed floor per job for Rust links) so the first run fits; real records then take over.
