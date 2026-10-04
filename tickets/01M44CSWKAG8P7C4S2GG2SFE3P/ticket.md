+++
id = "01M44CSWKAG8P7C4S2GG2SFE3P"
title = "Audit3 M3: the release publish job runs in a protected environment and only for commits on main"
type = "security"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T21:23:31Z"
updated = "2026-10-04T21:30:01Z"
labels = ["security"]
scope = [".github/workflows/release.yml", "docs/release.md", "crates/goway/tests/publishing.rs"]

[[acceptance]]
text = "Given release.yml, when the publish job is read, then it declares environment github-release and every job with contents write or id-token write declares an environment"
bound = true

[[acceptance]]
text = "Given a goway-v tag on a commit that is not an ancestor of origin/main, when version-tag runs, then it fails; docs/release.md lists the GitHub settings the owner must add"
bound = true
+++
