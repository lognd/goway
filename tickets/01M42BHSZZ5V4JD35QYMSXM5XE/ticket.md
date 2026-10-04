+++
id = "01M42BHSZZ5V4JD35QYMSXM5XE"
title = "Password login off or 2FA on the helper: goway add explains how to install its key by hand"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 1
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:09Z"
updated = "2026-10-04T02:39:44Z"
scope = ["crates/goway/src/add.rs", "crates/goway/src/sshsetup.rs", "docs/ssh-setup.md"]

[[acceptance]]
text = "Given a helper that refuses password login or asks for a second factor, when goway add needs to install its key, then it prints goway's public key and the exact commands to add it on the helper (or ssh-copy-id with the second factor typed by the user), then continues once the key works"
bound = true
+++
