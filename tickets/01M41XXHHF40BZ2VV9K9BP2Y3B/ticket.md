+++
id = "01M41XXHHF40BZ2VV9K9BP2Y3B"
title = "Slot updates keep old mtimes on changed files, so make, ninja and cargo can skip rebuilding and run stale code"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T22:24:53Z"
updated = "2026-10-03T22:26:38Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/sync.rs", "crates/goway/tests/**", "docs/design.md"]

[[acceptance]]
text = "Given a slot whose build outputs are newer than the incoming files (another worktree or an older branch built there last), when a run reconciles the slot, then every file written because its content changed gets a modification time later than any existing output (the current time, as git checkout does), unchanged files keep theirs, and cargo, make and ninja rebuild exactly the changed parts"
bound = false

[[acceptance]]
text = "Given a test that builds worktree A in a slot, then runs worktree B with an older version of one source file (older mtime) in the same slot, when B's run builds and runs, then B's binary reflects B's source (cargo and make or ninja both covered)"
bound = false
+++
