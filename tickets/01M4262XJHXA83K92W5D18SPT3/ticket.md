+++
id = "01M4262XJHXA83K92W5D18SPT3"
title = "goway add installs the key itself on a native Windows host (goway-setup --authorized-key)"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T00:47:38Z"
updated = "2026-10-04T03:11:41Z"
scope = ["crates/goway/src/add.rs", "crates/goway/src/hosts.rs", "docs/hosts.md", "crates/goway/src/sshsetup.rs"]

[[acceptance]]
text = "Given a native Windows host set up with goway-setup install --host --native, when goway add runs, then the key is handed over through --authorized-key (a key line or .pub path) and recorded, or the exact command is printed"
bound = true
+++

Coordinator item 3, decision: yes, goway add should print/offer the goway-setup command with the key; not done.
