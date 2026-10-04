+++
id = "01M41Q6DMFXAD67P17VTRQRCBB"
title = "gc, status and doctor for Windows hosts"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:24Z"
updated = "2026-10-04T06:12:28Z"
scope = ["crates/goway/src/gc.rs", "crates/goway/src/status.rs", "crates/goway/src/doctor.rs", "docs/usage.md", "crates/goway/src/doctor/windows.rs", "crates/goway/src/doctor/output.rs"]

[[acceptance]]
text = "Given Windows hosts, when goway gc and goway status run, then copies and caches are labelled, expire like on other hosts, and status shows Windows disk use"
bound = true

[[acceptance]]
text = "Given goway doctor on a Windows host, when it runs, then it checks powershell.exe or ssh, rustup with the msvc target, cargo-nextest and free space on the system drive, and prints exact fix commands"
bound = false
+++
