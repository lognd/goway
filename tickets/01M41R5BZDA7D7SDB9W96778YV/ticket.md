+++
id = "01M41R5BZDA7D7SDB9W96778YV"
title = "ssh setup pins only the confirmed host key, restricts the authorized key and prefers goway's own key"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:44:18Z"
updated = "2026-10-03T20:46:50Z"
scope = ["crates/goway/src/sshsetup.rs", "crates/goway/src/hosts.rs", "docs/ssh-setup.md", "crates/goway/tests/ssh_setup.rs"]

[[acceptance]]
text = "Given a scratch known_hosts with several keys, When one fingerprint is confirmed, Then only that key line is pinned"
bound = true

[[acceptance]]
text = "Given a stale scratch file with the same name, When ssh setup starts, Then it is removed first"
bound = true

[[acceptance]]
text = "Given the key line written to authorized_keys, When it is planned, Then it carries no-agent-forwarding, no-port-forwarding and no-X11-forwarding and is still recognised as present"
bound = true

[[acceptance]]
text = "Given goway already has its own key, When ssh setup chooses a key, Then it uses it and records it as the identity"
bound = true
+++
