+++
id = "01M42DAB5MTYFTE6W9ZKWV0DSG"
title = "doctor output is quiet and grouped: problems first across hosts, one line each, explanations on request, fixes listed once, hardening opt-in"
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:54:01Z"
updated = "2026-10-04T03:55:18Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/render.rs", "crates/goway/src/cli.rs", "crates/goway/tests/doctor_output.rs", "docs/usage.md", "crates/goway/src/doctor/output.rs", "crates/goway/src/doctor/projneeds.rs", "crates/goway/src/add.rs", "crates/goway/tests/doctor_project.rs", "crates/goway/tests/run_local.rs"]

[[acceptance]]
text = "Given goway doctor over several hosts, when it prints, then it shows one summary line per host (problems and ok counts), then each problem once, grouped across the hosts that share it, in one short line plus at most one line of context; passing checks appear only with --all; long explanations (such as the cargo linker workaround) appear only with goway doctor --explain CHECK"
bound = true

[[acceptance]]
text = "Given --fix, when fixes are planned, then each fix is listed once in plain words with its hosts (never the same text twice), the exact commands are shown on request at the prompt ([s]how, [y]es, [N]o) and always before anything runs as root, and the prompt says exactly what will change on which host"
bound = true

[[acceptance]]
text = "Given hardening fixes (such as turning off ssh password login), when doctor --fix runs, then they are listed separately as optional and applied only with --harden or an explicit separate confirmation; never bundled with tool installs"
bound = true

[[acceptance]]
text = "Given golden-output tests for one host, three hosts with shared problems, --all, --explain and --fix with and without --harden, when the wording changes, then the diff shows it"
bound = false
+++
