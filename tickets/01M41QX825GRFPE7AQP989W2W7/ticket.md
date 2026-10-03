+++
id = "01M41QX825GRFPE7AQP989W2W7"
title = "Verify the ssh control dir, call sudo by absolute path, validate sshd config before reload"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:39:52Z"
updated = "2026-10-03T20:41:54Z"
scope = ["crates/goway/src/ssh.rs", "crates/goway/src/add.rs", "crates/goway/src/doctor.rs", "crates/goway/src/remote.sh", "docs/config.md", "docs/hosts.md"]

[[acceptance]]
text = "Given a control dir that is a symlink or not mode 0700 owned by the user, When an ssh command is built, Then multiplexing is disabled"
bound = true

[[acceptance]]
text = "Given the sshd password fix, When it runs on a host whose unit is sshd, Then sshd -t runs first and reload falls back to sshd without aborting later fixes"
bound = true

[[acceptance]]
text = "Given goway add --lsudo, When sudo is spawned, Then it is /usr/bin/sudo or another root-owned system path, never a PATH lookup"
bound = true
+++
