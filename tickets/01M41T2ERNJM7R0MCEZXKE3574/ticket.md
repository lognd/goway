+++
id = "01M41T2ERNJM7R0MCEZXKE3574"
title = "GPU slots: one GPU job per GPU by default, with CUDA_VISIBLE_DEVICES set"
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:17:40Z"
updated = "2026-10-03T22:03:06Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/src/config.rs", "crates/goway/tests/**", "docs/config.md", "crates/goway/src/needs.rs", "crates/goway/src/shard.rs", "crates/goway/src/pool.rs", "crates/goway/src/resolve.rs", "crates/goway/src/status.rs", "crates/goway/src/hosts.rs", "crates/goway/src/sshsetup.rs", "docs/usage.md"]

[[links]]
kind = "blocked-by"
target = "01M41T2EDAC22Z3A6BP3Q0VGAX"

[[acceptance]]
text = "Given a run that needs a GPU on a host with K GPUs, when it starts, then it holds a lock on one free GPU (waiting with a note if all are busy), CUDA_VISIBLE_DEVICES and ROCR_VISIBLE_DEVICES name that GPU unless the user set them, and the lock is released however the run ends"
bound = true

[[acceptance]]
text = "Given gpu_jobs = N on a host, when set, then up to N GPU runs share each GPU"
bound = false
+++
