+++
id = "01M55X0T6HNRH6D9RJWJEPYXQC"
title = "Audit L6+L10: admit validation and key dir checks"
type = "bug"
category = "todo"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T21:41:51Z"
idempotency_key = "audit-l6l10"
scope = ["crates/goway-circle/src/state.rs"]

[[acceptance]]
text = "Admit of existing member refused; symlinked key dir mode checked"
bound = false
+++

See audit-0.2.0.md L6 and L10 (keys.rs). Admit refuses existing members and invalid keys; follow symlinks for key dir mode check; 0700 plus check in generate_sk_keypair.
