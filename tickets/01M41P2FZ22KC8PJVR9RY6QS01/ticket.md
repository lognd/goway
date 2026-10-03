+++
id = "01M41P2FZ22KC8PJVR9RY6QS01"
title = "Warm dependency directories per build slot (node_modules, .venv, build/, ...)"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T20:07:47Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/src/config.rs", "crates/goway/tests/**", "docs/usage.md", "docs/config.md"]

[[acceptance]]
text = "Given a project with package.json, pyproject.toml, CMakeLists.txt, pom.xml or build.gradle, when it runs twice on the same slot, then the second run finds its node_modules, .venv or build directory from the first run"
bound = false

[[acceptance]]
text = "Given a config keep list, when set, then exactly those paths persist per slot, and gc removes them with the slot's cache"
bound = false
+++
