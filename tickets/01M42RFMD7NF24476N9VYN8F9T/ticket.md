+++
id = "01M42RFMD7NF24476N9VYN8F9T"
title = "Learn each repository's build footprint and never send a run to a helper without room for it; explain a disk-full failure"
type = "story"
category = "todo"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T06:09:09Z"
updated = "2026-10-04T11:18:34Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/state.rs", "crates/goway/src/run.rs", "crates/goway/tests/footprint.rs", "docs/usage.md", "crates/goway/src/needs.rs", "crates/goway/src/project.rs", "crates/goway/src/footprint.rs", "crates/goway/src/lib.rs", "crates/goway/src/queue.rs", "crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given a repository's previous runs, when goway picks a host, then it knows the repository's peak footprint on a helper (slot tree plus target and build dirs, recorded per repository after each run) and skips a helper whose free space plus what eviction could free is below that footprint plus a margin, saying so in one note; an explicit --needs disk>=X still applies (found live on 2026-10-04: a full build of a large Rust workspace filled a 23 GiB helper, the linker failed with 'Disk full?' and the run failed)"
bound = true

[[acceptance]]
text = "Given a run that fails while the helper's disk is (nearly) full, when goway reports it, then it says the helper ran out of disk, how much the repository needs and has free there, what the disk budget freed afterwards, and suggests --needs disk>=SIZE or another host, instead of only the tool's error"
bound = true
+++
