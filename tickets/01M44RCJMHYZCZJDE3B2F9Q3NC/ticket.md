+++
id = "01M44RCJMHYZCZJDE3B2F9Q3NC"
title = "Tests that run goway inside a goway run leave real systemd scopes behind on the helper"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T00:45:58Z"
updated = "2026-10-05T00:45:58Z"
scope = ["crates/goway/tests/common/mod.rs", "docs/design.md"]

[[acceptance]]
text = "Given goway's own test suite running inside a goway run on a helper, when it ends or is killed, then no goway-*.scope unit of a test world remains (test worlds use a fake systemd-run unless a test opts into a real scope, and stopping an outer run's scope also stops the scopes of runs nested in it); found 2026-10-04 while working ~SZFHCRG: two nested scopes outlived their outer run by 2 hours"
bound = false
+++
