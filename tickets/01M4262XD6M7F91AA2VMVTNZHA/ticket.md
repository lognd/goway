+++
id = "01M4262XD6M7F91AA2VMVTNZHA"
title = "remote.ps1 parity with remote.sh: disk budget LRU eviction, budget probe word, heartbeat, clock safety, best-effort cleanup"
type = "story"
category = "in-progress"
priority = "high"
points = 8
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T00:47:38Z"
updated = "2026-10-04T11:28:54Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/tests/remote_ps1.rs"]

[[acceptance]]
text = "Given the run ttls argument cache:orphan:kept:max_disk:min_free:cache_size and the probe word budget:MAX:MIN_FREE, when remote.ps1 runs them, then it evicts least recently used entries to the budget like remote.sh and probe reports disk_max and disk_min_free"
bound = false

[[acceptance]]
text = "Given remote.sh's best-effort work dir cleanup, heartbeat watchdog, clock offset, liveness-not-age gc protection and future-mtime clamping, when remote.ps1 runs the same cases, then it behaves the same"
bound = false
+++

Coordinator note: remote.sh changed (~0ZXHQZJ, ~7JEJJBC, ~WXVMJKT, ~VAJPQC3). remote.ps1 today tolerates the extra ttls fields and the budget word (ignores them); no eviction yet. Found while working ~YGM627C.
