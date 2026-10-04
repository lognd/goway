+++
id = "01M42CDHAVDBNFHDW8MSSBC8M1"
title = "A dry run of a host install prints the plan even when the elevated guard cannot touch WSL"
type = "bug"
category = "in-progress"
priority = "critical"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:38:17Z"
updated = "2026-10-04T02:40:30Z"
scope = ["crates/goway-setup/src/cli.rs", "crates/goway-setup/tests/cli.rs"]

[[acceptance]]
text = "Given an elevated Windows process whose WSL distro is not running, when goway-setup install --host --dry-run runs, then it prints the assumed plan with a notice that a real install must start the distro from a normal terminal, and succeeds"
bound = false
+++
