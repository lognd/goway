+++
id = "01M41RBQJ30AQHAETMSJY6AG0H"
title = "Timing-sensitive integration tests fail under machine load"
type = "bug"
category = "todo"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:47:47Z"
updated = "2026-10-03T20:47:47Z"
scope = ["crates/goway/tests/**"]

[[acceptance]]
text = "Given a fully loaded machine (the suite run with many parallel builds), when the suite runs ten times, then ssh_setup::uninstall_everywhere_removes_goway_from_helper_and_laptop, run_local::shards_run_on_n_hosts_and_any_failure_fails_the_run, run_local::cargo_target_dir_is_a_free_per_repo_slot and run_local::gc_removes_expired_unlocked_entries_and_keeps_locked_or_fresh_ones never fail; waits are on conditions, not on fixed sleeps"
bound = false
+++
