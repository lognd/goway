+++
id = "01M41T2EMRZFGJGYGC82SH8Y2M"
title = "goway.toml project rules and host labels"
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:17:40Z"
updated = "2026-10-03T21:57:48Z"
scope = ["crates/goway/src/project.rs", "crates/goway/src/config.rs", "crates/goway/src/run.rs", "crates/goway/tests/**", "docs/config.md", "docs/usage.md", "crates/goway/src/needs.rs", "crates/goway/src/lib.rs", "crates/goway/src/shard.rs"]

[[links]]
kind = "blocked-by"
target = "01M41T2EH2C5MRX4SBCVY53SJM"

[[acceptance]]
text = "Given goway.toml at the repository root with [[rule]] entries (command glob, needs, prefers), when goway run starts, then the first matching rule's needs and prefers apply, merged with the command line (command line wins), and goway says which rule applied"
bound = true

[[acceptance]]
text = "Given labels = [...] on a [[host]] in the user config, when --needs label=NAME or a rule asks for it, then only hosts with that label qualify"
bound = true

[[acceptance]]
text = "Given goway.toml, when it is synced or parsed, then it holds no host names or secrets and an unknown key is an error with the file and line"
bound = true
+++
