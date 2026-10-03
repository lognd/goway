+++
id = "01M41RK5MER3TPE7839XC3XB05"
title = "Windows installer hygiene and NAT relay hardening: key ACL by SID, local config dir, absolute task paths, private subnet-bound relay target (audit L15 and relay review)"
type = "security"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:51:51Z"
updated = "2026-10-03T21:00:25Z"
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "docs/**", "scripts/windows/**", "crates/goway/**"]

[[acceptance]]
text = "Given a Windows private key, when goway restricts its ACL, then the grant for the token SID comes before inheritance is removed and the config lives in LocalAppData"
bound = true

[[acceptance]]
text = "Given a distro that prints any address, when the NAT relay is created or refreshed, then only a private address inside the WSL adapter subnet other than the gateway is ever forwarded to"
bound = true

[[acceptance]]
text = "Given scheduled tasks and the relay refresh task, when they are registered, then every program and script path is absolute and the system directory comes from Windows, not the environment"
bound = true

[[acceptance]]
text = "Given a pre-created empty state directory or a leading-dash distro name, when the host install runs, then the directory is replaced and the name refused"
bound = true
+++
