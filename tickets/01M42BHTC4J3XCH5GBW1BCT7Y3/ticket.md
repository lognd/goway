+++
id = "01M42BHTC4J3XCH5GBW1BCT7Y3"
title = "Small, noexec or unusual temp and home file systems: pick safe places and report them"
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:09Z"
updated = "2026-10-04T05:50:59Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/doctor.rs", "docs/config.md", "crates/goway/tests/scratch_fs.rs"]

[[acceptance]]
text = "Given a helper whose /tmp is a small tmpfs or mounted noexec, or whose home is on a case-insensitive or network file system, when goway runs, then its scratch files live under its own root rather than /tmp, doctor reports the file system type and free space of the root, and a repository with names differing only in case is refused on a case-insensitive host with the clashing paths named"
bound = true
+++
