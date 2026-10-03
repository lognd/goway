+++
id = "01M41QXBJQSNY0SFGH8B5DPKXF"
title = "Release dry run: build every release artifact on demand without publishing; move actions off Node 20"
type = "task"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:39:56Z"
updated = "2026-10-03T21:18:37Z"
scope = [".github/workflows/release.yml", ".github/workflows/ci.yml", "docs/release.md", "crates/goway/tests/publishing.rs"]

[[acceptance]]
text = "Given a manual workflow_dispatch of release.yml with publish off, when it runs, then every binary, wheel, sdist and checksum builds and uploads as workflow artifacts, and nothing is published to GitHub releases, crates.io or PyPI"
bound = true

[[acceptance]]
text = "Given the workflows, when they run, then no action reports the Node 20 deprecation warning"
bound = true
+++
