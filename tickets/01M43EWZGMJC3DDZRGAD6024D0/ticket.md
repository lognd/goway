+++
id = "01M43EWZGMJC3DDZRGAD6024D0"
title = "macOS CI: scratch_fs tests assume a case-sensitive file system"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T12:40:55Z"
updated = "2026-10-04T15:29:28Z"
scope = ["crates/goway/tests/scratch_fs.rs"]

[[acceptance]]
text = "Given a case-insensitive temp file system, When the scratch_fs tests run, Then they pass on macOS and Linux"
bound = true
+++

The case-clash test runs a receive that must succeed without the assume hook, and the doctor test expects root_case_insensitive=0; APFS is case-insensitive so the product (correctly) refuses and reports 1. Fix the test to ask the temp dir what it is.
