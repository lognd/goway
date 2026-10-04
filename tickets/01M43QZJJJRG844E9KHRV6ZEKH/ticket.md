+++
id = "01M43QZJJJRG844E9KHRV6ZEKH"
title = "remote.ps1 parity: lifeline ends at once on closed stdin and waits 120 s on silence with the reason in the lost marker, and gc skips a work dir whose runner or job is alive"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T15:19:37Z"
updated = "2026-10-04T15:27:08Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/tests/remote_ps1.rs"]

[[acceptance]]
text = "Given a Windows helper run whose client closes its lifeline or goes silent, when the lifeline ends, then the job stops at once on end of input or after the timeout (GOWAY_LIFELINE_TIMEOUT, default 120 s) on silence, the lost marker holds the reason and the run prints 'stopped by the helper: ...'; and given a work dir whose recorded runner or job process is alive, when gc or eviction considers it, then it is reported busy and kept even if its lock looks free"
bound = true
+++
