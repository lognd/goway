+++
id = "01M42BHR7CAP9XQWNM2GXR2KV2"
title = "Permission denied explained: the exact cause and next command (wrong password, password login off, wrong --user, fail2ban, AllowUsers)"
type = "story"
category = "todo"
priority = "high"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:07Z"
updated = "2026-10-04T02:31:20Z"
scope = ["crates/goway/src/error.rs", "crates/goway/src/hosts.rs", "crates/goway/src/add.rs", "crates/goway/tests/host_add.rs", "docs/troubleshooting.md", "crates/goway/src/cli.rs", "crates/goway/src/sshsetup.rs", "crates/goway/tests/ssh_setup.rs", "README.md", "docs/ssh-setup.md"]

[[acceptance]]
text = "Given goway add or any ssh login that ends in Permission denied, when goway reports it, then the message names the likely causes in plain words (a mistyped password; password login switched off on the helper; the wrong --user; a ban after failed attempts; an AllowUsers rule) and gives the exact next command for each (for example ssh-copy-id with goway's key, or the sshd_config line to check), and docs/troubleshooting.md has a section for it"
bound = false

[[acceptance]]
text = "Given a helper account with no password at all (passwd -S shows NP, common with automatic login; sshd never accepts empty passwords, found live on 2026-10-03), when goway add's password login is refused, then the hint names this case first among the causes with the check (passwd -S USER) and the two fixes: add goway's public key line to ~/.ssh/authorized_keys on the helper (goway prints the exact line and commands), or set a password with passwd"
bound = false
+++
