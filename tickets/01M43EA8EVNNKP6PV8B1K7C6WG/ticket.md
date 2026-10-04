+++
id = "01M43EA8EVNNKP6PV8B1K7C6WG"
title = "Windows CI: two remote_ps1 tests fail (a gc test sees young work dirs; the auto-gc summary test used a far-future now)"
type = "bug"
category = "in-progress"
priority = "high"
points = 1
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T12:30:42Z"
updated = "2026-10-04T12:32:18Z"
scope = ["crates/goway/tests/remote_ps1.rs"]

[[acceptance]]
text = "Given the Windows CI job, when remote_ps1 runs, then gc_removes_expired_labelled_entries_and_keeps_busy_fresh_and_unlabelled_ones and the_automatic_gc_leaves_a_summary_and_probe_reports_the_budget pass"
bound = true
+++

found on a ci/ branch run after ~V4A27S6 and ~MVTNZHA
