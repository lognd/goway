+++
id = "01M418J2PA3M2FESKBF2Y6VZWP"
title = "Config file and host identity: TOML config, state file, pinned host keys via HostKeyAlias"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:14:38Z"
scope = ["crates/goway/**", "docs/**", "Cargo.lock"]

[[acceptance]]
text = "Given a config.toml with hosts, when goway loads it, then hosts, defaults and durations parse and unknown keys are rejected"
bound = false

[[acceptance]]
text = "Given a host, when goway builds an ssh command, then it uses HostKeyAlias goway-NAME, StrictHostKeyChecking yes and goway's known_hosts file"
bound = false
+++
