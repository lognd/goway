+++
id = "01M4WB91WSA62K363KR6ETEFBQ"
title = "goway-setup*.exe needs elevation to start at all: Windows installer detection triggers on 'setup'"
type = "bug"
category = "todo"
priority = "medium"
reporter = "lognd"
created = "2026-10-14T04:38:37Z"
updated = "2026-10-17T14:52:07Z"
scope = ["crates/goway-setup/build.rs", "crates/goway-setup/Cargo.toml"]

[[acceptance]]
text = "Every goway-setup exe embeds an application manifest with requestedExecutionLevel asInvoker"
bound = false

[[acceptance]]
text = "goway-setup.exe install --host --dry-run runs unelevated under its release name, and the real install still elevates through its own prompt"
bound = false
+++

Running goway-setup-arm64.exe install --host --dry-run from PowerShell (not elevated) failed with "The requested operation requires elevation" before goway ran. A renamed copy (gw-host-arm64.exe) ran fine and did its own UAC elevation later. Windows installer detection elevates unmanifested exes whose names contain "setup"/"install". build.rs embeds no manifest. Effects: --dry-run, status and the client install all demand admin, and interop launches from WSL fail outright.
