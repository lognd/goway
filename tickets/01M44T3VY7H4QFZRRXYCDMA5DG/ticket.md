+++
id = "01M44T3VY7H4QFZRRXYCDMA5DG"
title = "The end-of-run sweep mistakes its own processes for leftovers, and the client_loss tests assume /proc and flock"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:16:10Z"
updated = "2026-10-05T01:25:43Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/client_loss.rs", "docs/design.md"]

[[acceptance]]
text = "Given a plain run on a helper without systemd scopes, when it ends, then no 'left processes behind' note appears and the run's seed is kept (footprint making_room test passes on Linux without a user manager)"
bound = true

[[acceptance]]
text = "Given the client_loss tests on a machine without /proc or flock (macOS), when they run, then they check liveness with kill/ps and skip what the platform cannot do (the session-escaping straggler, the slot lock probe), and pass"
bound = true
+++
