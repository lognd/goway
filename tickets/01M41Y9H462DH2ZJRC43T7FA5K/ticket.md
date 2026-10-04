+++
id = "01M41Y9H462DH2ZJRC43T7FA5K"
title = "Fleet drift: doctor shows a host-by-tool version table, flags missing or too-old tools as errors and disagreements as drift, and runs record the versions they used"
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T22:31:26Z"
updated = "2026-10-04T04:17:56Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/drift.rs", "docs/usage.md", "crates/goway/src/state.rs", "crates/goway/src/cli.rs", "crates/goway/tests/doctor_output.rs", "crates/goway/tests/drift.rs", "crates/goway/src/lib.rs", "crates/goway/src/doctor/projneeds.rs", "crates/goway/src/add.rs"]

[[links]]
kind = "blocked-by"
target = "01M41P2G171VZJW77Z1XP4RZVS"

[[acceptance]]
text = "Given a project and several hosts, when goway doctor runs, then it prints one table (also in --plain) with a row per required tool and a column per host plus the laptop, showing versions; a missing tool or one below the project minimum is an error with the exact fix; hosts that disagree (different major version, or different minor where the tool is a compiler or a pinned [toolchain] entry) are marked drift"
bound = true

[[acceptance]]
text = "Given goway doctor --fix --all-hosts, when it runs, then it brings every host to the required (or pinned) versions with user-level installs where possible and --rsudo for system packages, each recorded for uninstall"
bound = true
+++
