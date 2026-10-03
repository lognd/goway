+++
id = "01M41Y9H462DH2ZJRC43T7FA5K"
title = "Fleet drift: doctor shows a host-by-tool version table, flags missing or too-old tools as errors and disagreements as drift, and runs record the versions they used"
type = "story"
category = "todo"
priority = "high"
points = 5
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T22:31:26Z"
updated = "2026-10-03T22:31:26Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/drift.rs", "crates/goway/src/facts.rs", "crates/goway/src/run.rs", "crates/goway/src/render.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/usage.md"]

[[links]]
kind = "blocked-by"
target = "01M41P2G171VZJW77Z1XP4RZVS"

[[acceptance]]
text = "Given a project and several hosts, when goway doctor runs, then it prints one table (also in --plain) with a row per required tool and a column per host plus the laptop, showing versions; a missing tool or one below the project minimum is an error with the exact fix; hosts that disagree (different major version, or different minor where the tool is a compiler or a pinned [toolchain] entry) are marked drift"
bound = false

[[acceptance]]
text = "Given goway doctor --fix --all-hosts, when it runs, then it brings every host to the required (or pinned) versions with user-level installs where possible and --rsudo for system packages, each recorded for uninstall"
bound = false

[[acceptance]]
text = "Given a run, when the chosen host's cached tool versions differ from the rest of the fleet for a tool the project uses, then goway prints a one-line drift note, and the run report (and --report JSON) records the versions of the project's tools on that host so frob evidence says what built and tested it"
bound = false
+++
