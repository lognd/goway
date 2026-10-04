+++
id = "01M423RSW9ETZ357QYKM6ZS9DZ"
title = "Clippy fails on the README download link test (manual char comparison)"
type = "bug"
category = "in-progress"
priority = "critical"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:07:09Z"
updated = "2026-10-04T00:11:41Z"
scope = ["crates/goway/tests/publishing.rs"]

[[acceptance]]
text = "Given cargo clippy --workspace --all-targets -- -D warnings, when it runs, then publishing.rs passes"
bound = true
+++
