+++
id = "01M421JZEKNNR41FFNAXQ6C1D9"
title = "Release on goway-v* tags (protected environments), check the tag matches the Cargo version, publish to both crates.io and PyPI"
type = "task"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T23:29:01Z"
updated = "2026-10-03T23:56:30Z"
scope = [".github/workflows/release.yml", "crates/goway/tests/publishing.rs", "docs/release.md", ".github/workflows/ci.yml"]

[[acceptance]]
text = "Given release.yml, when a goway-vX.Y.Z tag is pushed, then the release runs, every publishing job (GitHub release, crates-io, pypi) is guarded by startsWith(github.ref, 'refs/tags/goway-v') and a push event, and a v* tag or a dry run publishes nothing"
bound = true

[[acceptance]]
text = "Given a goway-v tag whose version differs from the workspace version in Cargo.toml, when the release runs, then it fails before publishing anything and says which versions differ"
bound = true

[[acceptance]]
text = "Given a successful GitHub release, when it finishes, then both the crates-io job (environment crates-io) and the pypi job (environment pypi, trusted publishing) run, independently of each other, and the release is titled goway X.Y.Z"
bound = true
+++
