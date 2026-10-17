+++
id = "01M547FDWEGTPFNBW8XTJDXE1S"
title = "Upgrade LAN pairing to real CPace"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:06:07Z"
updated = "2026-10-17T11:44:59Z"
scope = ["crates/goway-circle/src/lan.rs", "crates/goway-circle/**", "crates/goway/src/lanpair.rs", "docs/circles.md", "README.md", "Cargo.lock"]

[[acceptance]]
text = "A correct one-time code plus admin approval admits the machine over loopback"
bound = false

[[acceptance]]
text = "A wrong code fails both sides and changes nothing"
bound = false

[[acceptance]]
text = "Degenerate or malformed CPace points are rejected"
bound = false
+++

Phase 4 uses a commit-reveal SAS because no prime-order group crate is available. Add a vetted ristretto255 crate and implement CPace with a short code. Found while working ~42YF0JB.
