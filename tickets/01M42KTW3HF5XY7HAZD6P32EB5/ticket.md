+++
id = "01M42KTW3HF5XY7HAZD6P32EB5"
title = "Per-repository prerequisites in goway.toml: rust targets, tools and distro packages that doctor checks and installs"
type = "story"
category = "in-progress"
priority = "high"
points = 3
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-04T04:47:54Z"
updated = "2026-10-04T05:39:25Z"
scope = ["crates/goway/src/project.rs", "crates/goway/src/doctor.rs", "crates/goway/src/doctor/projneeds.rs", "crates/goway/tests/doctor_project.rs", "docs/config.md", "docs/usage.md", "crates/goway/src/doctor/prereq.rs", "crates/goway/src/doctor/output.rs"]

[[acceptance]]
text = """Given goway.toml [toolchain] with rust_targets = ["x86_64-pc-windows-gnu"], tools = ["x86_64-w64-mingw32-gcc"] and packages = { apt = ["gcc-mingw-w64-x86-64"], dnf = [...], pacman = [...] }, when goway doctor runs in that project, then it checks each on every host of the project's OS (targets for the toolchain the project pins), and --fix installs rust targets user-level (rustup target add for that toolchain) and packages through --rsudo for the host's package manager"""
bound = true

[[acceptance]]
text = "Given packages a repository asks for, when they would be installed as root, then the plan names the repository and goway.toml as the source, lists every package, and needs the usual explicit confirmation; package names are validated against the package manager's name syntax (no options, paths or shell characters) so a repository cannot inject anything but package names"
bound = true

[[acceptance]]
text = "Given rust-toolchain.toml with targets = [...], when doctor runs, then those targets count as rust_targets too"
bound = true
+++
