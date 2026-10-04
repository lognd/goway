+++
id = "01M41T4HN7VTHZZZP7ZTQP4A3T"
title = "doctor on Windows hosts: MSVC Build Tools, rustup msvc toolchain and nextest, installed with winget and recorded"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T21:18:48Z"
updated = "2026-10-04T06:12:50Z"
scope = ["crates/goway/src/doctor.rs", "docs/usage.md", "crates/goway/src/doctor/windows.rs", "crates/goway/src/cli.rs", "crates/goway/src/uninstall.rs", "crates/goway/src/doctor/output.rs"]

[[links]]
kind = "blocked-by"
target = "01M41Q6D3QBJ6N93Q2HPG3XK9R"

[[acceptance]]
text = "Given a Windows host without the C++ Build Tools, rustup, the msvc target or cargo-nextest, when goway doctor --fix runs, then it installs them (winget for Build Tools with the VC workload, rustup-init with a pinned hash, nextest with a pinned hash) after confirmation, records each for goway uninstall, and explains what needs administrator rights"
bound = false
+++
