+++
id = "01M43S56R7BSW1JHQ0JJS14CQ5"
title = "macOS: lifeline reports silence as a closed pipe (bash 3.2 read -t returns 1 on timeout), evict_live lifeline test fails"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T15:40:10Z"
updated = "2026-10-04T17:21:58Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/evict_live.rs"]

[[acceptance]]
text = "Given a macOS runner, When the lifeline hears nothing for its timeout, Then it records silent for Ns and not connection closed"
bound = true
+++

found while working E09XD8K: CI run on branch ci/vdze455, evict_live the_lifeline_says_why_it_stopped_a_job_and_tells_silence_from_a_closed_pipe fails on macOS-14 (panics at evict_live.rs:155 with 'connection closed'). Likely bash 3.2 returns 1, not >128, from read -t on timeout, so the rc>128 test in remote.sh lifeline misclassifies silence.
