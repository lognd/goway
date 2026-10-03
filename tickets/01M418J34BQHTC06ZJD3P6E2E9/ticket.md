+++
id = "01M418J34BQHTC06ZJD3P6E2E9"
title = "Guided, reversible ssh setup: keys, authorized_keys, permissions, icacls"
type = "task"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T17:27:03Z"
scope = ["crates/goway/**", "crates/goway-journal/**", "docs/ssh-setup.md", "docs/usage.md", "README.md", "crates/goway-setup/src/hostsys.rs"]

[[acceptance]]
text = "Given a host, when goway ssh setup runs and then goway ssh setup --undo runs, then authorized_keys and local files equal their state before setup"
bound = false

[[acceptance]]
text = "Given a Windows client, when setup creates goway's key, then the key file's ACL is reduced to the current user (icacls), so Windows OpenSSH accepts it"
bound = false

[[acceptance]]
text = "Given a host whose key authentication does not work yet, when goway ssh setup runs, then it authorizes goway's key over one password login and goway's own key-only calls work afterwards"
bound = false
+++
