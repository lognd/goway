+++
id = "01M454TH22DPTG1J88CBJKDGAR"
title = "The end-of-run sweep mistakes the watchdog's half-second sleep for a leftover under load"
type = "bug"
category = "todo"
priority = "high"
points = 1
reporter = "lognd"
created = "2026-10-05T04:23:18Z"
updated = "2026-10-05T04:23:18Z"
scope = ["crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given a loaded helper, When a run ends cleanly, Then the sweep does not report a short-lived sleep of its own watchdog as a left-behind process"
bound = false
+++

found in the full-suite run for ~V630TRA: pid of 'sleep 0.5' seen at both looks of tagged_left, 0.3 s apart; the look interval must exceed the longest watchdog sleep
