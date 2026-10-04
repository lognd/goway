+++
id = "01M424MYD4HA48XG5HEAWVVJCJ"
title = "doctor asks each ecosystem's own tool for the project's needs: cargo metadata, go list, mvn help:effective-pom, Gradle, dotnet --list-sdks"
type = "story"
category = "todo"
priority = "medium"
reporter = "lognd"
created = "2026-10-04T00:22:32Z"
updated = "2026-10-04T00:22:32Z"
scope = ["crates/goway/src/ecotools.rs", "crates/goway/src/doctor.rs", "docs/usage.md"]

[[acceptance]]
text = "Given other ecosystems, when doctor determines needs, then it uses each tool's machine-readable output where it exists (cargo metadata, go list -m -json and go env -json, mvn help:effective-pom, Gradle's toolchain report, dotnet --list-sdks) and real parsers (TOML, JSON, XML) for declarative files; plain text matching is only a fallback when the tool is missing, and doctor labels such results approximate and says to install the tool first"
bound = false
+++

Split from ~XP4RZVS (criterion 6); that ticket landed the declarative TOML and JSON detection this refines.
