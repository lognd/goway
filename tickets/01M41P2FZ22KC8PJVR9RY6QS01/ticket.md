+++
id = "01M41P2FZ22KC8PJVR9RY6QS01"
title = "Persistent slot trees: update each build slot in place instead of copy-then-delete; dependency dirs stay warm"
type = "story"
category = "in-progress"
priority = "medium"
points = 8
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T21:12:25Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/src/config.rs", "crates/goway/tests/**", "docs/usage.md", "docs/config.md", "crates/goway/src/gc.rs", "docs/design.md"]

[[acceptance]]
text = "Given two runs of the same worktree on the same slot, when the second starts, then its slot tree is updated in place from the seed (only changed files written, files deleted from the work tree removed) and no full copy of the tree is made"
bound = true

[[acceptance]]
text = "Given a project with package.json, pyproject.toml, CMakeLists.txt, pom.xml, build.gradle or Cargo.toml, when it runs twice on the same slot, then node_modules, .venv, build/ or other detected dependency and build directories from the first run are still there"
bound = true

[[acceptance]]
text = "Given files a previous run generated that are neither in the work tree nor on the keep list (detected plus the config keep list), when the next run starts, then they are removed first, so no run sees another run's leftovers"
bound = true

[[acceptance]]
text = "Given concurrent runs, when they overlap, then each holds its own slot exclusively, and --keep still leaves that run's tree for inspection"
bound = true

[[acceptance]]
text = "Given a slot idle past the cache expiry (7 days default), when auto gc or goway gc runs, then the slot tree is removed with its build cache"
bound = true
+++
