+++
id = "01M42RAM7D56M1KH49NTGZTRVF"
title = "goway measures each helper clock offset in the probe, status shows offsets over 2 seconds, doctor warns with the fix"
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T06:06:25Z"
updated = "2026-10-04T06:44:31Z"
scope = ["crates/goway/src/facts.rs", "crates/goway/src/status.rs", "crates/goway/src/doctor.rs", "docs/troubleshooting.md", "crates/goway/src/facts/clock.rs"]

[[acceptance]]
text = "Given a probe, when it runs, then goway measures the helper clock offset from the laptop (helper epoch time against the laptop time, corrected by half the round trip), goway status shows offsets over 2 seconds, and goway doctor warns with the fix (WSL after sleep: wsl --shutdown or sudo hwclock -s; Windows: resync time)"
bound = false
+++

Criterion 1 of ~VAJPQC3, split so gc liveness and mtime clamping can land while status.rs and doctor.rs are leased.
