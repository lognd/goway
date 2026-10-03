+++
id = "01M41XX4KZ6VD5BQC7PH9QPJ0X"
title = "weighted_shards::a_bigger_host_runs_proportionally_more_partitions fails on helpers (also on clean main)"
type = "bug"
category = "todo"
priority = "medium"
reporter = "lognd"
created = "2026-10-03T22:24:40Z"
updated = "2026-10-03T22:24:40Z"
scope = ["crates/goway/tests/weighted_shards.rs"]

[[acceptance]]
text = "Given a helper run of the suite, when it runs, then weighted_shards passes"
bound = false
+++

found while working ~QNTX8FZ: fails at weighted_shards.rs:94 on quasar and xanders-laptop on unmodified main
