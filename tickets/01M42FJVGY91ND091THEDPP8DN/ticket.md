+++
id = "01M42FJVGY91ND091THEDPP8DN"
title = "Cross-OS runs: same OS by default, a loud hint for portable test runners, --any-os and cross_os in goway.toml, and --each-os to run once per OS"
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T03:33:37Z"
updated = "2026-10-04T04:25:44Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/runners.rs", "crates/goway/src/project.rs", "crates/goway/src/run.rs", "crates/goway/src/shard.rs", "crates/goway/src/cli.rs", "crates/goway/tests/cross_os.rs", "docs/usage.md", "docs/config.md"]

[[acceptance]]
text = "Given a recognized cross-platform command (cargo build/test/nextest/clippy, pytest or python -m pytest, go test, npm/pnpm/yarn test, npx vitest/jest, mvn, gradle or gradlew, dotnet test, ctest) and hosts of another OS in the fleet, when goway run picks hosts with no --any-os and no cross_os setting, then it stays on the laptop's OS and prints a prominent warning (its own block, not a one-line note) naming the other-OS hosts that could take it and the two ways to allow it: --any-os for this run, or cross_os = true in the project's goway.toml; cross_os = false silences the warning for that project"
bound = false

[[acceptance]]
text = "Given --any-os or cross_os = true, when goway picks hosts or shards, then hosts of every OS are candidates (each OS keeps its own caches), and the report and the per-shard lines name each shard's OS"
bound = false

[[acceptance]]
text = "Given goway run --each-os -- CMD, when it runs, then CMD runs once on one host of each OS in the fleet in parallel, output lines carry [host os] prefixes, a summary lists each OS's exit code, and goway exits non-zero if any OS failed (the first failing exit code, or 125 if goway itself failed)"
bound = false

[[acceptance]]
text = "Given an unrecognized command (bash -c, a script, an arbitrary program), when it is run, then no warning is printed and it stays same-OS unless --any-os or --each-os asks otherwise"
bound = false
+++
