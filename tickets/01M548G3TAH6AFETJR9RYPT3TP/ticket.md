+++
id = "01M548G3TAH6AFETJR9RYPT3TP"
title = "resolve: roster stage for circle hosts"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:23:58Z"
updated = "2026-10-17T07:13:13Z"
scope = ["crates/goway/src/resolve.rs", "docs/config.md", "docs/hosts.md", "changelog.d/01M548G3TAH6AFETJR9RYPT3TP.added.md"]

[[acceptance]]
text = "A circle roster host (label circle, no address) resolves by roster name in a Roster stage after peer hints, with the host key still verified"
bound = true
+++

found while working ~Q5WP11P
