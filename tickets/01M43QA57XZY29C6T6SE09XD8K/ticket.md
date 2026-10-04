+++
id = "01M43QA57XZY29C6T6SE09XD8K"
title = "macOS CI: slot_trees written_files_are_stamped_after_outputs_dated_in_the_future hangs for hours on a runner"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T15:07:56Z"
updated = "2026-10-04T17:12:58Z"
scope = ["crates/goway/src/spawn.rs", "crates/goway/src/run.rs", "crates/goway/src/local.rs", "crates/goway/src/session.rs", "crates/goway/src/remotesys.rs", "crates/goway/src/ecotools.rs", "crates/goway/src/gitmeta.rs", "crates/goway/src/hosts.rs", "crates/goway/src/sshenv.rs", "crates/goway/src/lib.rs", "crates/goway/src/repo.rs", "crates/goway/src/runners.rs", "crates/goway/src/interop.rs", "crates/goway/src/add.rs", "crates/goway/src/resolve.rs", "crates/goway/tests/nesting.rs", "crates/goway/tests/slot_trees.rs", "docs/macos.md", "crates/goway/src/doctor.rs", "crates/goway/src/sshsetup.rs", "crates/goway/src/uninstall.rs", "crates/goway/src/winadmin.rs"]

[[acceptance]]
text = "Given a macOS runner, When slot_trees runs, Then no test runs past its timeout"
bound = true

[[acceptance]]
text = "Given goway runs jobs from several threads on macOS, When any child is spawned, Then it is spawned under the process-wide spawn lock so no long-lived child inherits another call's pipe"
bound = true
+++

Seen hung (SLOW >2900s, no output) in three macOS jobs on different branches (two started before the memory-sampling fix, so not caused by it); it passed on main fc83349. Runs hogged the macOS runner pool for 2h+. Needs a reproduction on a Mac or a per-test timeout in .config/nextest.toml. Found while working D6024D0.
