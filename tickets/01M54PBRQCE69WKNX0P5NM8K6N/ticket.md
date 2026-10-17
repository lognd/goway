+++
id = "01M54PBRQCE69WKNX0P5NM8K6N"
title = "Four goway tests fail on main: spawn lock invariant, journal invariant, footprint full-disk message, evict_live duplicate min_free key"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T10:26:16Z"
updated = "2026-10-17T10:51:11Z"
scope = ["crates/goway/src/doctor.rs"]

[[acceptance]]
text = "no_production_code_spawns_a_child_outside_the_lock passes"
bound = true

[[acceptance]]
text = "no_machine_change_appears_outside_the_journal_backed_modules passes"
bound = false

[[acceptance]]
text = "a_failure_on_a_nearly_full_disk_is_explained_in_plain_words passes"
bound = false

[[acceptance]]
text = "a_wave_over_a_tiny_budget_never_loses_a_live_runs_work_dir passes"
bound = false
+++
