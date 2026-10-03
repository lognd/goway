+++
id = "01M41P2G171VZJW77Z1XP4RZVS"
title = "doctor checks exactly what a project needs on every host: build tools, CMake, test frameworks and libraries, language toolchains; nothing it does not need"
type = "story"
category = "todo"
priority = "medium"
points = 8
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T22:31:26Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a project folder, when goway doctor runs there, then it detects the ecosystems from the project's files and checks each host for their tools, with exact fixes (user-level where possible, root fixes through --rsudo)"
bound = false

[[acceptance]]
text = "Given a C or C++ project, when goway doctor runs, then it checks a compiler and make or ninja (build-essential on Debian and Ubuntu), CMake against cmake_minimum_required, git and network when FetchContent or CPM fetch from git, and system libraries the project finds (find_package(GTest) needs libgtest-dev, find_package(Catch2) needs catch2, pkg_check_modules entries), mapped to apt, dnf and pacman package names by one table; FetchContent and CPM dependencies need no system package"
bound = false

[[acceptance]]
text = "Given a project, when doctor runs, then it checks only what the project's files require (Rust: cargo and nextest only when used, rust-toolchain.toml; Python: requires-python, uv, pytest and xdist when declared; Node: .nvmrc or engines and the lockfile's package manager; Java: release or toolchain version plus Maven or Gradle; Go: go.mod version; Ruby: .ruby-version and bundler; .NET: global.json) plus goway.toml [toolchain] entries, and skips everything else (a C++ project is never asked for cargo)"
bound = false

[[acceptance]]
text = 'Given goway.toml [toolchain] (for example cmake = ">=3.24", gcc = "14", tools = ["protoc"]), when doctor runs, then those requirements are checked like detected ones and take precedence'
bound = false
+++
