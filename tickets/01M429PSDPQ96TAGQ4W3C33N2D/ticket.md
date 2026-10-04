+++
id = "01M429PSDPQ96TAGQ4W3C33N2D"
title = "macOS helper gaps: shard detection, nested runs, copy-verify rerun, slot leftovers, sync.rs ignores"
type = "task"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-04T01:50:55Z"
updated = "2026-10-04T01:50:55Z"
scope = ["docs/macos.md", ".github/workflows/ci.yml"]

[[acceptance]]
text = "Given the macOS CI job, when it runs, then none of the listed tests are skipped"
bound = false
+++

found while working ~FKHDK2A: skipped in the macOS CI job (see the filter in ci.yml and docs/macos.md): shard_detect and weighted_shards binaries, nesting::recursive_goway_stops_at_the_depth_limit (hangs on macOS after depth 1), copy_integrity a_failure_with_a_bad_copy_is_rerun_once_on_a_rebuilt_copy (verify-verdict: no work dir), slot_trees deleted_files_and_leftovers_go_but_dependency_dirs_stay, five ignored sync.rs protocol tests (sync.rs was leased), Linux-fact tests (needs, project_rules, doctor, install_methods, priority).
