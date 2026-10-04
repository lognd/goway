+++
id = "01M43QA57XZY29C6T6SE09XD8K"
title = "macOS CI: slot_trees written_files_are_stamped_after_outputs_dated_in_the_future hangs for hours on a runner"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T15:07:56Z"
updated = "2026-10-04T15:07:56Z"
scope = ["crates/goway/tests/slot_trees.rs"]

[[acceptance]]
text = "Given a macOS runner, When slot_trees runs, Then no test runs past its timeout"
bound = false
+++

Seen hung (SLOW >2900s, no output) in three macOS jobs on different branches (two started before the memory-sampling fix, so not caused by it); it passed on main fc83349. Runs hogged the macOS runner pool for 2h+. Needs a reproduction on a Mac or a per-test timeout in .config/nextest.toml. Found while working D6024D0.
