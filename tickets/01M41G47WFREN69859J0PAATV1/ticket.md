+++
id = "01M41G47WFREN69859J0PAATV1"
title = "H2: host firewall rules scoped to local networks; password login off by default once a key works"
type = "security"
category = "in-progress"
priority = "critical"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:53Z"
updated = "2026-10-03T19:00:14Z"
labels = ["security"]
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "scripts/windows/**", "docs/install-windows.md"]

[[acceptance]]
text = "Given goway-setup install --host with default options, when it creates firewall rules, then they allow only Private/Domain profiles and the local subnet (wider only with an explicit --allow-from), and sshd password login is disabled once a key is authorized"
bound = true
+++
