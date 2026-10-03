+++
id = "01M41TNZH9N2J377DRPQNTX8FZ"
title = "Slot trees can keep stale content: a same-size edit within the same second as the last sync is not copied"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:28:20Z"
updated = "2026-10-03T22:26:34Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/sync.rs", "crates/goway/tests/**", "docs/design.md"]

[[acceptance]]
text = "Given a file edited without changing its size within the same second as the previous sync (seed mtimes are whole seconds), when the next run reconciles its slot tree, then the slot gets the new content (decided from what receive actually wrote, for example a per-generation list of changed paths, or a content comparison for racy entries, never from size and whole-second mtime alone)"
bound = true

[[acceptance]]
text = "Given the slot_trees tests, when they run fast or slow, then they check content, not inode numbers or whole-second times, and CI run 37155069651's failure (second_run_updates_in_place_and_writes_only_changes) cannot recur"
bound = true
+++
