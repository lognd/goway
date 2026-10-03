+++
id = "01M418J3BG08155MM6317X1700"
title = "Linux install and uninstall scripts"
type = "task"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T17:24:14Z"
scope = ["scripts/install.sh", "scripts/uninstall.sh", "crates/goway/tests/install_scripts.rs", "docs/install-linux.md", "README.md"]

[[acceptance]]
text = "Given a Linux user, when scripts/install.sh then scripts/uninstall.sh run, then ~/.local/bin and shell profile equal their state before"
bound = true
+++
