+++
id = "01M547FDSMN6H0CJMRF3C79VYG"
title = "QR output for circle cards and pair-host address"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-17T06:06:07Z"
updated = "2026-10-17T10:12:04Z"
scope = ["crates/goway/src/lanpair.rs", "crates/goway/Cargo.toml", "crates/goway/src/cli.rs", "crates/goway/src/circle.rs", "crates/goway/src/access.rs", "docs/circles.md", "crates/goway/tests/circle_revoke_now.rs", "crates/goway/tests/circle_admin.rs"]
+++

Phase 4 left QR undone: no qrcode crate in the tree. Evaluate adding one; print the card and pair-host address as QR. Found while working ~42YF0JB.
