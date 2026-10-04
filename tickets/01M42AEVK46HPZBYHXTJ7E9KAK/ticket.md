+++
id = "01M42AEVK46HPZBYHXTJ7E9KAK"
title = "macOS CI: remote::tests::invocation_runs_locally_through_a_shell fails (empty output)"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T02:04:04Z"
updated = "2026-10-04T02:04:04Z"
scope = ["crates/goway/src/remote.rs"]

[[acceptance]]
text = "Given macOS CI, when the lib tests run, then invocation_runs_locally_through_a_shell passes"
bound = false
+++

found while working YRM1N6N: CI macos job (runs 37167653172, 37169409262) fails at remote.rs:144 left empty, right 'goway-remote ok'.
