+++
id = "01M42BHSGD4HSJY1GQXVCXDFW2"
title = "Never trip fail2ban or sshguard: bound login attempts per host and back off"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:08Z"
updated = "2026-10-04T05:19:09Z"
scope = ["crates/goway/src/resolve.rs", "crates/goway/src/ssh.rs", "crates/goway/src/hosts.rs", "crates/goway/tests/host_add.rs", "docs/troubleshooting.md", "crates/goway/src/ssh/attempts.rs", "crates/goway/src/sshsetup.rs", "crates/goway/src/error.rs"]

[[acceptance]]
text = "Given a helper with fail2ban (typically 5 failures in 10 minutes), when goway resolves, probes and adds hosts, then it never makes more than 2 failed authentications per host per 10 minutes (candidates are tried with the pinned key only, IdentitiesOnly, no password retries), and a connection refused right after failures is reported as a probable ban with how to lift it"
bound = true
+++
