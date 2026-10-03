+++
id = "01M41Q1BTPDXN7HY00QGZZEDKG"
title = "Widen the secret-file denylist to common credential files"
type = "security"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:24:38Z"
updated = "2026-10-03T20:27:44Z"
scope = ["crates/goway/src/sync.rs", "docs/usage.md", "docs/positioning.md"]

[[acceptance]]
text = "Given untracked cargo credentials, *.env, tfstate, tfvars, kubeconfig and similar files, When files are collected, Then they are kept local unless allowed"
bound = true

[[acceptance]]
text = "Given ordinary source files with secret-like words in the name, When files are collected, Then they are still sent"
bound = true
+++
