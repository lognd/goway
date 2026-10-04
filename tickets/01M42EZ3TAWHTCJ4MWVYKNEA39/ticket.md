+++
id = "01M42EZ3TAWHTCJ4MWVYKNEA39"
title = "The pool only picks hosts of the laptop's OS unless the run pins a host or asks for another OS"
type = "story"
category = "done"
outcome = "done"
priority = "high"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T03:22:51Z"
updated = "2026-10-04T03:28:15Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/needs.rs", "docs/hosts.md", "crates/goway/src/status.rs", "crates/goway/src/project.rs"]

[[acceptance]]
text = "Given a pool with Linux and Windows hosts and a laptop running Linux (WSL counts as Linux), when goway run picks hosts without --host or --needs os=..., then only Linux hosts are candidates (likewise a macOS laptop picks macOS hosts unless asked), a sharded run never mixes OSes unless --needs allows it, and goway status marks which hosts the default pool uses"
bound = true

[[acceptance]]
text = "Given --needs os=windows (or a goway.toml rule) or --host <a Windows host>, when goway picks, then Windows hosts are used as today; a test with mixed fake hosts proves both directions"
bound = true
+++
