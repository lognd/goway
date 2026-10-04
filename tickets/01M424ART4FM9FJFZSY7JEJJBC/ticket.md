+++
id = "01M424ART4FM9FJFZSY7JEJJBC"
title = "A failing rm -rf of the run's work dir (Directory not empty) turns a good run into exit 125"
type = "bug"
category = "todo"
priority = "high"
reporter = "lognd"
created = "2026-10-04T00:16:58Z"
updated = "2026-10-04T00:16:58Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/remote_cleanup.rs"]

[[acceptance]]
text = "Given the work dir cleanup after a run hits Directory not empty under load, when the command itself succeeded, then the run still exits with the command's exit code and cleanup is retried, leaving at most a dir that gc collects"
bound = false
+++

Found while working ~JY6AG0H: under load on a helper, 'rm: cannot remove .../work/<id>: Directory not empty' at remote.sh 'rm -rf $work' (set -e) made run_local::piped_output_is_byte_identical_even_with_escape_sequences, paths_baked_by_a_build_stay_valid_for_later_runs_in_the_slot, shard_runners::ctest_shards_run_every_test_exactly_once and local_host::a_pooled_local_machine_competes_with_a_margin_and_respects_max_jobs fail (exit 125 or work dir left). Cleanup should be best-effort with one retry.
