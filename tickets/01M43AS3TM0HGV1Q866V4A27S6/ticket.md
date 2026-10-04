+++
id = "01M43AS3TM0HGV1Q866V4A27S6"
title = "remote.ps1 parity, part 2: lifeline heartbeat, keep-awake, clock safety, best-effort work dir cleanup"
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T11:28:54Z"
updated = "2026-10-04T12:00:30Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/tests/remote_ps1.rs", "crates/goway/src/sync.rs", "crates/goway/src/run.rs"]

[[acceptance]]
text = "Given remote.sh's lifeline verb (stops the job's group when stdin ends or is silent for 30s), runner file, done marker and SIGTERM handling, when a Windows helper runs a job, then remote.ps1 has the same lifeline (stops the job object), writes the done file, and the client starts it for Windows hosts too"
bound = true

[[acceptance]]
text = "Given a Windows helper that would sleep on idle, when a goway job runs there, then remote.ps1 holds SetThreadExecutionState ES_SYSTEM_REQUIRED from the job's own process for exactly the job's lifetime"
bound = true

[[acceptance]]
text = "Given the probe and doctor verbs and remote.sh's clock offset and liveness-not-age protection, when remote.ps1 runs them, then probe and doctor report epoch=, probe takes tools:A,B and prints want.TOOL lines, and a starting run's work dir is protected from gc by its creator process or the monotonic clock, not by wall-clock age"
bound = true
+++

split from ~MVTNZHA (disk budget first); lifeline request from lane D's ~WXVMJKT
