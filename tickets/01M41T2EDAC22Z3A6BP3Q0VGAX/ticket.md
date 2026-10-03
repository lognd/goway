+++
id = "01M41T2EDAC22Z3A6BP3Q0VGAX"
title = "Host facts: discover GPUs, RAM, CPU features, KVM, Docker and disk on every host; show them in status"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:17:40Z"
updated = "2026-10-03T21:17:40Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/pool.rs", "crates/goway/src/facts.rs", "crates/goway/src/status.rs", "crates/goway/src/doctor.rs", "crates/goway/src/state.rs", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a host, when goway probes it, then it learns GPUs (vendor, model, memory, driver and CUDA version via nvidia-smi or rocm-smi), total and available RAM, CPU features (avx2, avx512f, neon), /dev/kvm, a working docker, and free disk for goway's root, parsed defensively with bounds like the existing probe"
bound = false

[[acceptance]]
text = "Given goway status, when it runs, then each host shows its GPU, RAM and notable features (also in --plain), and the facts are cached per host with their age"
bound = false

[[acceptance]]
text = "Given a WSL helper on a Windows laptop with an NVIDIA GPU but no WSL CUDA driver, when goway doctor runs, then it says the GPU is invisible to WSL and how to fix it"
bound = false
+++
