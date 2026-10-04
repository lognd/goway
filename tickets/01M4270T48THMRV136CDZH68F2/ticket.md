+++
id = "01M4270T48THMRV136CDZH68F2"
title = "The same-second slot test must not depend on how long a run takes"
type = "bug"
category = "todo"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T01:03:58Z"
updated = "2026-10-04T01:03:58Z"
scope = ["crates/goway/tests/slot_trees.rs"]

[[acceptance]]
text = "Given a slow or loaded helper, when same_size_edit_within_the_same_second_reaches_the_slot runs, then it passes because every edit is dated inside the racy window the sender protects, not at a fixed second that ages while runs take seconds"
bound = false
+++

Product verified correct (racy_from is taken before files are read; stale content cannot reach a run for organic edits). The test pinned a stamp at test start; on a loaded host the stamp is older than now-1 by the third run, so it is no longer racy and an mtime-and-size-identical edit is (by design) invisible.
