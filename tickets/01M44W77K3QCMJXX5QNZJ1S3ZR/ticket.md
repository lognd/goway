+++
id = "01M44W77K3QCMJXX5QNZJ1S3ZR"
title = "A seed evicted between the manifest and the hashes call fails the run instead of retrying"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:52:57Z"
updated = "2026-10-05T02:25:44Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/evict_live.rs"]

[[acceptance]]
text = "Given a seed tree removed after the manifest, when the hashes call runs, then it succeeds with no output so the sync retries through the generation check"
bound = true
+++

CI run 37252169628 (linux): a_wave_over_a_tiny_budget_never_loses_a_live_runs_work_dir failed with 'remote call to local failed: cd: .../seed/.../tree: No such file or directory' at hashes. The seed lock is held per ssh call only, so an eviction between manifest and hashes removes the tree; hashes must answer empty and let receive notice the changed generation (exit 75, retried).
