+++
id = "01M41P2G171VZJW77Z1XP4RZVS"
title = "doctor checks exactly what a project needs on every host: build tools, CMake, test frameworks and libraries, language toolchains; nothing it does not need"
type = "story"
category = "in-progress"
priority = "medium"
points = 13
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-04T00:30:14Z"
scope = ["crates/goway/src/doctor.rs", "docs/usage.md", "crates/goway/tests/doctor_project.rs", "crates/goway/src/doctor/projneeds.rs", "crates/goway/src/project.rs"]

[[acceptance]]
text = "Given a project folder, when goway doctor runs there, then it detects the ecosystems from the project's files and checks each host for their tools, with exact fixes (user-level where possible, root fixes through --rsudo)"
bound = true

[[acceptance]]
text = "Given a project, when doctor runs, then it checks only what the project's files require (Rust: cargo and nextest only when used, rust-toolchain.toml; Python: requires-python, uv, pytest and xdist when declared; Node: .nvmrc or engines and the lockfile's package manager; Java: release or toolchain version plus Maven or Gradle; Go: go.mod version; Ruby: .ruby-version and bundler; .NET: global.json) plus goway.toml [toolchain] entries, and skips everything else (a C++ project is never asked for cargo)"
bound = false

[[acceptance]]
text = 'Given goway.toml [toolchain] (for example cmake = ">=3.24", gcc = "14", tools = ["protoc"]), when doctor runs, then those requirements are checked like detected ones and take precedence'
bound = false

[[acceptance]]
text = "Given a C or C++ project, when goway doctor runs, then it checks a compiler and make or ninja (build-essential on Debian and Ubuntu), CMake against cmake_minimum_required, and git when FetchContent or CPM fetch from git, reading CMakeLists.txt as text and labelling those findings approximate (CMake's own interfaces are the follow-up ticket)"
bound = false
+++
