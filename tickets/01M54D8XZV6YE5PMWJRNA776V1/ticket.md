+++
id = "01M54D8XZV6YE5PMWJRNA776V1"
title = "Circle push leftovers: stdin transport for large rosters and per-member cert expiry"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T07:47:26Z"
updated = "2026-10-17T08:46:27Z"
idempotency_key = "XQGX5E1-leftovers"
labels = ["creates:changelog.d/01M54D8XZV6YE5PMWJRNA776V1.added.md"]
scope = ["crates/goway/src/circle*.rs", "crates/goway/src/access.rs", "crates/goway/src/cli.rs", "crates/goway/tests/circle_revoke_now.rs", "docs/circles.md", "changelog.d/01M54D8XZV6YE5PMWJRNA776V1.added.md"]

[[acceptance]]
text = "Roster pushes ride ssh stdin with no size cap, and circle list shows the issued cert expiry of other members"
bound = true
+++

Left over from ~XQGX5E1: roster push rides the ssh command line (100 KB cap) - move to stdin; store issued cert expiry per member so circle list shows it for other machines.
