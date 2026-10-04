+++
id = "01M42EZ3TAWHTCJ4MWVYKNEA39"
title = "The pool only picks hosts of the laptop's OS unless the run pins a host or asks for another OS"
type = "story"
category = "todo"
priority = "high"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T03:22:51Z"
updated = "2026-10-04T03:22:51Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/needs.rs", "crates/goway/src/shard.rs", "crates/goway/tests/os_pool.rs", "docs/usage.md", "docs/hosts.md"]

[[acceptance]]
text = "Given a pool with Linux and Windows hosts and a laptop running Linux (WSL counts as Linux), when goway run picks hosts without --host or --needs os=..., then only Linux hosts are candidates (likewise a macOS laptop picks macOS hosts unless asked), a sharded run never mixes OSes unless --needs allows it, and goway status marks which hosts the default pool uses"
bound = false

[[acceptance]]
text = "Given --needs os=windows (or a goway.toml rule) or --host <a Windows host>, when goway picks, then Windows hosts are used as today; a test with mixed fake hosts proves both directions"
bound = false
+++
