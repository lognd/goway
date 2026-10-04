+++
id = "01M41Q6DT69P172DECKXKXW2BJ"
title = "frob-v2 runs its Windows tests and clippy through goway before a push"
type = "task"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:24Z"
updated = "2026-10-04T01:18:43Z"
scope = ["docs/usage.md", "README.md"]

[[acceptance]]
text = "Given the frob-v2 checkout, when goway run --host win -- cargo nextest run --workspace and cargo clippy --workspace --all-targets -- -D warnings run, then both finish natively on Windows and match what Windows CI reports"
bound = true

[[acceptance]]
text = "Given that works, when it is done, then the frob coordinator is told so it can wire it into frob's pre-push routine, and ~/bin/winrun, winsync and winbuild are documented as replaced"
bound = true
+++
