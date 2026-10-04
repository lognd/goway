+++
id = "01M43CCF1E0CQW22RXDYS01CGE"
title = "remote.ps1 parity, part 3: future-mtime clamping and best-effort work dir cleanup"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T11:56:57Z"
updated = "2026-10-04T12:54:51Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/tests/remote_ps1.rs"]

[[acceptance]]
text = "Given a laptop clock ahead of the Windows helper (files dated in its future) and a work dir that cannot be fully removed, when remote.ps1 runs the same cases as remote.sh, then slot copies get the host's time and cleanup is best effort without failing the run"
bound = true
+++

left over from ~V4A27S6
