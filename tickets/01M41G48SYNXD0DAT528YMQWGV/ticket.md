+++
id = "01M41G48SYNXD0DAT528YMQWGV"
title = "M1: a stale deletions file must never delete files on a later sync"
type = "security"
category = "done"
outcome = "done"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:54Z"
updated = "2026-10-03T18:32:33Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a sync whose upload failed after its deletions were stored, when the file is restored locally and goway syncs again, then the file is present on the remote"
bound = true
+++
