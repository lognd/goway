+++
id = "01M41T4HN7VTHZZZP7ZTQP4A3T"
title = "doctor on Windows hosts: MSVC Build Tools, rustup msvc toolchain and nextest, installed with winget and recorded"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T21:18:48Z"
updated = "2026-10-03T21:18:48Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/remote.ps1", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a Windows host without the C++ Build Tools, rustup, the msvc target or cargo-nextest, when goway doctor --fix runs, then it installs them (winget for Build Tools with the VC workload, rustup-init with a pinned hash, nextest with a pinned hash) after confirmation, records each for goway uninstall, and explains what needs administrator rights"
bound = false
+++
