+++
id = "01M544DC66PA51J77JCCSMEP9M"
title = "Unreachable helper: probe port 22 when the recorded port is closed, and stop blaming client isolation"
type = "bug"
category = "done"
outcome = "done"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-17T05:12:34Z"
updated = "2026-10-17T05:31:04Z"
idempotency_key = "nova-port-fallback"
labels = ["windows", "onboarding"]
scope = ["crates/goway/src/error.rs", "crates/goway/src/resolve.rs", "docs/hosts.md", "docs/troubleshooting.md", "changelog.d/01M544DC66PA51J77JCCSMEP9M.fixed.md"]

[[acceptance]]
text = "Given a recorded port that is closed, when the helper answers on 22 or 2222, then goway connects there and the unreachable error names every port tried"
bound = true
+++

Report from another session (host nova, Windows native sshd on 22, stale port 2222 recorded from an old WSL setup). status/run only probed 2222 and said 'looks like client isolation'. Ask: when the recorded port is closed, probe 22 (and 2222), and name the port tried in the error. Unverified here; reproduce first.
