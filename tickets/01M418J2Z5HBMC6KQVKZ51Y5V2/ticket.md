+++
id = "01M418J2Z5HBMC6KQVKZ51Y5V2"
title = "Cleanup: labels, expiry, auto gc and goway gc filters"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:53:51Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given entries past their TTL with free locks, when gc runs, then they are removed and locked or fresh entries are kept"
bound = false

[[acceptance]]
text = "Given goway gc --repo R --older-than D --dry-run, when it runs, then it lists exactly the matching entries and removes nothing"
bound = false
+++
