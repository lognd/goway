+++
id = "01M41G48MQT87PV0WBJQS3GJHQ"
title = "H3: confirm host key fingerprints before pinning; never send a password to an unpinned host"
type = "security"
category = "in-progress"
priority = "critical"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:54Z"
updated = "2026-10-03T18:29:32Z"
labels = ["security"]
scope = ["crates/goway/**", "docs/hosts.md", "docs/ssh-setup.md"]

[[acceptance]]
text = "Given a host that answers with a matching hostname, when goway host add or goway ssh setup runs without an interactive confirmation or --fingerprint, then no key is pinned and no password prompt is shown"
bound = true
+++
