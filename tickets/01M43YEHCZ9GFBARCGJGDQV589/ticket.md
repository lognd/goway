+++
id = "01M43YEHCZ9GFBARCGJGDQV589"
title = "macOS: bash 3.2 prints 'Terminated' for the killed watchdog on a run's stderr; footprint making_room test flakes"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T17:12:39Z"
updated = "2026-10-04T22:11:54Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/footprint.rs"]

[[acceptance]]
text = "Given a macOS helper, When a run ends, Then nothing about the watchdog is printed on the job's stderr"
bound = true
+++

found while working E09XD8K: macOS CI stderr carries 'goway: line 574: Terminated: 15 watchdog ...' after kill of the background watchdog (bash 3.2 job notification; use disown or wait with 2>/dev/null in a subshell). Also footprint::making_room_never_evicts_the_runs_own_seed failed once in four stress iterations (footprint.rs:220), probably the same stderr noise or a race.
