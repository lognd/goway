+++
id = "01M42BTWC93QV5RKM95ZQD22QE"
title = "doctor asks cargo metadata, go list, mvn, Gradle and dotnet for the project needs, with real parsers and approximate fallbacks"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:28:06Z"
updated = "2026-10-04T04:24:34Z"
scope = ["crates/goway/src/ecotools.rs", "crates/goway/src/doctor.rs", "crates/goway/src/doctor/projneeds.rs", "docs/usage.md", "crates/goway/src/drift.rs"]

[[acceptance]]
text = "Given other ecosystems, when doctor determines needs, then it uses each tool's machine-readable output where it exists (cargo metadata, go list -m -json and go env -json, mvn help:effective-pom, Gradle's toolchain report, dotnet --list-sdks) and real parsers (TOML, JSON, XML) for declarative files; plain text matching is only a fallback when the tool is missing, and doctor labels such results approximate and says to install the tool first"
bound = true
+++

Criterion 1 of ~AWVVJCJ, split so the cargo linker check (criterion 2) could land first.
