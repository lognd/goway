+++
id = "01M41T2EH2C5MRX4SBCVY53SJM"
title = "goway run --needs and --prefers: hardware requirements and preferences"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:17:40Z"
updated = "2026-10-03T21:54:14Z"
scope = ["crates/goway/src/cli.rs", "crates/goway/src/pool.rs", "crates/goway/src/needs.rs", "crates/goway/src/run.rs", "crates/goway/src/shard.rs", "crates/goway/src/error.rs", "crates/goway/tests/**", "docs/usage.md", "crates/goway/src/facts.rs", "crates/goway/src/remote.sh", "crates/goway/src/config.rs", "crates/goway/src/lib.rs", "crates/goway/Cargo.toml", "Cargo.lock", "crates/goway/src/resolve.rs", "crates/goway/src/status.rs", "crates/goway/src/hosts.rs", "crates/goway/src/sshsetup.rs", "docs/config.md"]

[[links]]
kind = "blocked-by"
target = "01M41T2EDAC22Z3A6BP3Q0VGAX"

[[acceptance]]
text = "Given --needs terms (gpu, gpu=cuda, gpu=rocm, gpu-mem>=8G, cuda>=12.1, mem>=16G, cores>=8, arch=x86_64, os=linux, cpu=avx512f, kvm, docker, disk>=50G, label=NAME), when goway picks hosts, then only hosts meeting every term qualify; when none does, it exits 125 listing each host and what it lacks"
bound = true

[[acceptance]]
text = "Given --prefers terms, when several hosts qualify, then hosts meeting more preferences score better, without ever excluding a host"
bound = true

[[acceptance]]
text = "Given a sharded run with --needs, when hosts are chosen, then every shard's host meets the needs"
bound = true

[[acceptance]]
text = "Given a finished run, when the report is written, then it records the host facts the needs and prefers matched (for example GPU model and memory), so frob evidence says what the result was measured on"
bound = false
+++
