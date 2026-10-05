+++
id = "01M455YWY9KTYZNB25D119KERS"
title = "client_loss does not compile on macOS: the stray-sweep test calls the Linux-only process_mentions"
type = "bug"
category = "todo"
priority = "critical"
points = 1
reporter = "lognd"
created = "2026-10-05T04:43:10Z"
updated = "2026-10-05T04:43:10Z"
scope = ["crates/goway/tests/client_loss.rs"]

[[acceptance]]
text = "Given the macOS CI job, When it builds the tests, Then client_loss compiles, and a_real_stray_is_stopped_and_named_while_the_sccache_server_stays still runs on Linux"
bound = false
+++

macOS CI on 3424bcb4 fails to compile crates/goway/tests/client_loss.rs:286 (E0425, common::process_mentions is cfg(target_os = linux)). The test proves the end-of-run sweep stops a real stray, and the sweep reads /proc, so on macOS it finds nothing by design (docs/design.md); the test belongs to Linux only.
