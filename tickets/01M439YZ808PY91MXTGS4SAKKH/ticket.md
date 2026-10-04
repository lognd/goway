+++
id = "01M439YZ808PY91MXTGS4SAKKH"
title = "Shard and each-os lines name the OS the host reported, not the config's os (macOS helper shows linux)"
type = "bug"
category = "todo"
priority = "high"
points = 1
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T11:14:37Z"
updated = "2026-10-04T11:14:37Z"
scope = ["crates/goway/src/shard.rs", "crates/goway/tests/cross_os.rs"]

[[acceptance]]
text = "Given a helper whose probe reports os=darwin but whose config os is linux, when --each-os or --shard runs, then the [host os] prefix, the summary line and the report name darwin"
bound = false
+++

found on macOS CI after ~EDPP8DN
