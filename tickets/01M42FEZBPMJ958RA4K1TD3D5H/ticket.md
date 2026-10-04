+++
id = "01M42FEZBPMJ958RA4K1TD3D5H"
title = "A Windows host's first sync into an empty goway root fails with seed changed (have empty generation)"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T03:31:30Z"
updated = "2026-10-04T03:41:43Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/tests/remote_ps1.rs", "crates/goway/src/error.rs", "crates/goway/src/interop.rs", "crates/goway/tests/ps_session.rs"]

[[acceptance]]
text = """Given a Windows host (interop or ssh) whose goway root does not exist yet, when the first goway run syncs to it, then the seed's generation is created and the run proceeds (found live on 2026-10-04: three runs failed with 'seed changed (have "", expected ...)' after two retries), covered by a test under pwsh 7 and Windows PowerShell 5.1"""
bound = true

[[acceptance]]
text = "Given an interop host, when a remote call fails, then the error does not say 'ssh to' (it names the transport that failed)"
bound = true
+++
