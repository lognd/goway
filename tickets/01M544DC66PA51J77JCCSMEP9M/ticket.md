+++
id = "01M544DC66PA51J77JCCSMEP9M"
title = "Unreachable helper: probe port 22 when the recorded port is closed, and stop blaming client isolation"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-17T05:12:34Z"
updated = "2026-10-17T05:14:44Z"
idempotency_key = "nova-port-fallback"
labels = ["windows", "onboarding"]
+++

Report from another session (host nova, Windows native sshd on 22, stale port 2222 recorded from an old WSL setup). status/run only probed 2222 and said 'looks like client isolation'. Ask: when the recorded port is closed, probe 22 (and 2222), and name the port tried in the error. Unverified here; reproduce first.
