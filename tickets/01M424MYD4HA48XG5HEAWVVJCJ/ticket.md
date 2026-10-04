+++
id = "01M424MYD4HA48XG5HEAWVVJCJ"
title = "doctor asks each ecosystem's own tool for the project's needs: cargo metadata, go list, mvn help:effective-pom, Gradle, dotnet --list-sdks"
type = "story"
category = "in-progress"
priority = "medium"
reporter = "lognd"
created = "2026-10-04T00:22:32Z"
updated = "2026-10-04T02:26:31Z"
scope = ["crates/goway/src/ecotools.rs", "crates/goway/src/doctor.rs", "docs/usage.md", "crates/goway/src/doctor/projneeds.rs", "crates/goway/src/lib.rs"]

[[acceptance]]
text = "Given other ecosystems, when doctor determines needs, then it uses each tool's machine-readable output where it exists (cargo metadata, go list -m -json and go env -json, mvn help:effective-pom, Gradle's toolchain report, dotnet --list-sdks) and real parsers (TOML, JSON, XML) for declarative files; plain text matching is only a fallback when the tool is missing, and doctor labels such results approximate and says to install the tool first"
bound = false

[[acceptance]]
text = """Given a Rust project whose .cargo/config.toml (or a parent's, or CARGO_* env) names a linker or link-arg such as linker = "clang" with -fuse-ld=mold or lld, for the helper's target triple, when doctor runs, then it checks that linker and that ld backend on every helper (cargo config get, or a TOML parse of every config.toml cargo would read), reports a missing one as an error with the fix (system package through --rsudo: clang, mold, lld), and names the one-run override (CARGO_TARGET_<TRIPLE>_LINKER and _RUSTFLAGS via goway run --env) without ever applying it silently; found live on 2026-10-03: frob-v2 needs clang and mold, and neither helper has them"""
bound = false
+++

Split from ~XP4RZVS (criterion 6); that ticket landed the declarative TOML and JSON detection this refines.
