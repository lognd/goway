+++
id = "01M43ETRMKFS15MWTTVZHX18YW"
title = "An unpinned cross-OS run whose translation is in doubt re-picks a host of the laptop's OS instead of stopping"
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T12:39:42Z"
updated = "2026-10-04T15:05:42Z"
scope = ["crates/goway/src/run.rs", "crates/goway/src/error.rs", "crates/goway/src/remote.sh", "crates/goway/src/remote.ps1", "crates/goway/tests/translate_repick.rs", "docs/design.md", "crates/goway/src/needs.rs", "crates/goway/tests/remote_ps1.rs"]

[[acceptance]]
text = "Given an --any-os (or cross_os) run that lands on another OS where translation is in doubt, when goway finds out, then it re-picks among hosts of the laptop's OS (excluding the doubtful host) and runs there with one note, instead of exiting 125; the doubtful host's synced work dir is removed; a pinned run still stops as specified"
bound = true
+++
