+++
id = "01M426PCBA3AW3Q3XXMYKF9Q5D"
title = "remote_ps1 stamp test: build output must get a fresh mtime, not Copy-Item's preserved one"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T00:58:16Z"
updated = "2026-10-04T00:58:18Z"
scope = ["crates/goway/tests/remote_ps1.rs", "crates/goway/src/remote.ps1"]

[[acceptance]]
text = "Given Windows PowerShell 5.1 on CI, when changed_files_are_stamped_newer_than_the_slots_outputs runs, then a build that writes its output fresh sees B's source once and stays warm after"
bound = false
+++

CI run 37166058824 failed at remote_ps1.rs:764 under 5.1. STAMP_BUILD used Copy-Item, which preserves the source mtime, so the warm check compared equal-or-truncated times. Passes on aarch64 5.1 locally.
