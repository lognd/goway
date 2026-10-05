+++
id = "01M44V923VV7R5AT0NVEM10R1J"
title = "Slot-wait test times out waiting for a held run to start on the Linux and macOS CI runners"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:36:28Z"
updated = "2026-10-05T02:18:49Z"
scope = ["crates/goway/tests/common/mod.rs", "crates/goway/tests/slot_wait.rs"]

[[acceptance]]
text = "Given a runner without a user manager, when the slot-wait test holds both build slots and runs with --wait 2s, then it exits 125 naming the busy slots"
bound = true
+++

CI run 37251466869 (linux and macos jobs): a_run_waiting_for_a_busy_slot_gives_up_after_wait_with_what_it_saw times out at common/mod.rs wait_started after 120 s. Root cause to be found from the held run's stderr.
