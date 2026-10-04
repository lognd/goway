+++
id = "01M427WY4XBZ8F0E3BY3FFY4EN"
title = "Optional git metadata on the helper: --with-git gives the run a .git that matches the work tree, for tests that inspect the repository"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T01:19:19Z"
updated = "2026-10-04T01:19:19Z"
scope = ["crates/goway/src/sync.rs", "crates/goway/src/run.rs", "crates/goway/src/cli.rs", "crates/goway/src/remote.sh", "crates/goway/src/remote.ps1", "crates/goway/tests/with_git.rs", "docs/usage.md", "docs/design.md"]

[[acceptance]]
text = "Given goway run --with-git (or with_git = true in goway.toml), when the run starts, then the helper's tree has a .git whose HEAD, index and status match the laptop's work tree (for example from a shallow bundle of HEAD plus the index, built on the laptop and cached per repository), so tests that check git status or tracked paths behave as locally; credentials, remotes' URLs with tokens and hooks are never sent"
bound = false

[[acceptance]]
text = "Given frob-v2's two .git-dependent tests (frob-release rel001 and grimble-model tracked_paths), when run on a Windows host with --with-git, then they pass"
bound = false
+++
