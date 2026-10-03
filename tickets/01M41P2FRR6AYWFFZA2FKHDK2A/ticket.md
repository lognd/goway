+++
id = "01M41P2FRR6AYWFFZA2FKHDK2A"
title = "Portable helper side: macOS (and other non-GNU systems) as helpers"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T20:07:47Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/doctor.rs", "crates/goway/src/pool.rs", "crates/goway/tests/**", ".github/workflows/ci.yml"]

[[acceptance]]
text = "Given a macOS helper with Homebrew coreutils, findutils, flock and util-linux, when goway syncs and runs a command there, then it works like on Linux (CI runs the full suite on macOS)"
bound = false

[[acceptance]]
text = "Given a macOS helper missing those tools, when goway doctor --fix runs, then it installs them with Homebrew without sudo, and doctor explains what is missing otherwise"
bound = false
+++
