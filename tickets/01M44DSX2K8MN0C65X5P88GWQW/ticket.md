+++
id = "01M44DSX2K8MN0C65X5P88GWQW"
title = "Audit3 H2: administrator routes never start goway-setup by bare name; a protected, verified copy only"
type = "security"
category = "in-progress"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T21:41:00Z"
updated = "2026-10-04T21:47:55Z"
labels = ["security"]
scope = ["crates/goway/src/winadmin.rs", "crates/goway/src/winadmin.ps1", "crates/goway/src/doctor/windows.rs", "crates/goway/src/add.rs", "crates/goway/tests/win_elevate.rs", "crates/goway/tests/winadmin_setup.rs", "docs/install-windows.md", "crates/goway/tests/pwsh/mod.rs", "crates/goway/tests/ps_quote.rs"]

[[acceptance]]
text = "Given a goway-setup planted in a user-writable PATH directory on a Windows helper, when an administrator route runs the setup step, then the planted program never runs and the route reports that no protected goway-setup was found"
bound = true

[[acceptance]]
text = "Given a goway-setup in an administrator-only directory (trusted owner, no write access for others, no links), when the step runs, then that absolute path is run with the arguments exactly as given"
bound = true
+++
