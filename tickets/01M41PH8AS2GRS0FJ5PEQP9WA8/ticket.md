+++
id = "01M41PH8AS2GRS0FJ5PEQP9WA8"
title = "Publish goway wheels to PyPI (maturin bin bindings) with trusted publishing"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:15:51Z"
updated = "2026-10-03T20:15:51Z"
scope = ["Cargo.toml", "crates/*/Cargo.toml", ".github/workflows/release.yml", ".github/workflows/ci.yml", "docs/release.md", "README.md", "pyproject.toml"]

[[acceptance]]
text = "Given a version tag, when the release workflow runs, then PyPI receives goway wheels for Linux x86_64/aarch64, macOS and Windows built by maturin with bindings = bin, uploaded through PyPI trusted publishing with no stored token"
bound = false

[[acceptance]]
text = "Given a fresh machine with uv, when uv tool install goway runs, then goway --version prints the released version"
bound = false
+++
