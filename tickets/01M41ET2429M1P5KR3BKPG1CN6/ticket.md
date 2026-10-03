+++
id = "01M41ET2429M1P5KR3BKPG1CN6"
title = "Test suite must pass when run on a goway host (no WSL interop over ssh, inherited CARGO_TARGET_DIR)"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:00:51Z"
updated = "2026-10-03T18:00:52Z"
scope = ["crates/goway/tests/**", "crates/goway/src/resolve.rs"]

[[acceptance]]
text = "Given goway run --host H -- cargo nextest run --workspace on each pool host, when it finishes, then every test passes"
bound = false
+++
