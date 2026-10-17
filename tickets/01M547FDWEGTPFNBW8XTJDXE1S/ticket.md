+++
id = "01M547FDWEGTPFNBW8XTJDXE1S"
title = "Upgrade LAN pairing to real CPace"
type = "task"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:06:07Z"
updated = "2026-10-17T11:40:59Z"
scope = ["crates/goway-circle/src/lan.rs"]
+++

Phase 4 uses a commit-reveal SAS because no prime-order group crate is available. Add a vetted ristretto255 crate and implement CPace with a short code. Found while working ~42YF0JB.
