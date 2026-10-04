+++
id = "01M42FN011Z3NBG491XAHHZCF5"
title = 'Portable command translation: the program name or path is translated per OS (python3 to py -3, ./gradlew to gradlew.bat, ./build/x to build\x.exe), shown and recorded'
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T03:34:48Z"
updated = "2026-10-04T12:20:39Z"
scope = ["crates/goway/src/translate.rs", "crates/goway/src/runners.rs", "crates/goway/src/project.rs", "crates/goway/src/run.rs", "crates/goway/src/remote.ps1", "crates/goway/src/remote.sh", "crates/goway/tests/translate.rs", "docs/config.md", "docs/design.md", "crates/goway/src/shard.rs", "crates/goway/src/lib.rs"]

[[links]]
kind = "blocked-by"
target = "01M42FJVGY91ND091THEDPP8DN"

[[acceptance]]
text = 'Given a command whose program differs by OS, when it runs on a host of another OS, then only the program (argv[0]) is translated, never other arguments: python3 or python to the py -3 launcher (else python.exe; never the WindowsApps Store stub) on Windows and to python3 on a Linux host without python; pip3 or pip to py -3 -m pip; ./gradlew to gradlew.bat; ./mvnw to mvnw.cmd; a ./relative/path program to .\relative\path with .exe appended when that file exists (and the multi-config build\Debug or build\Release location when only that exists); node, npm, npx, cargo, go and dotnet unchanged'
bound = true

[[acceptance]]
text = "Given a translation, when the run starts, then goway prints one line naming host and translation (for example laptop-windows: python3 -> py -3), and the --report records the requested and the actual program"
bound = true

[[acceptance]]
text = 'Given [translate] entries in goway.toml (for example mytool = { windows = "mytool.cmd", linux = "mytool" }), when goway translates, then project entries take precedence over built-in ones, and an unknown OS key is a config error naming file and line'
bound = true

[[acceptance]]
text = "Given a command goway can translate, when cross-OS rules apply (~EDPP8DN), then it counts as cross-platform (the same-OS default, the loud hint, --any-os, cross_os and --each-os all apply to it)"
bound = true

[[acceptance]]
text = "Given any doubt (no candidate, several plausible candidates, a candidate that is a Store stub or a reparse point, a probe that fails or times out, an internal error), when goway would translate, then it does not translate: an unpinned run stays on the laptop's OS, and a run pinned to another OS stops before running anything with what was looked for and why; a wrong or guessed program is never run (false negatives over false positives)"
bound = true

[[acceptance]]
text = "Given program resolution on a host, when a candidate is looked up, then only PATH directories are searched (never the current directory and never any path inside the synced work tree), so a repository cannot ship its own python.exe or gradlew.bat look-alike that a bare name resolves to; the resolved absolute path is what runs and what the report records"
bound = true

[[acceptance]]
text = "Given the translation table, when maintained, then it is one declarative table (program, per-OS ordered candidates) with a small reviewed set of entries each covered by per-OS tests; new tools go in goway.toml [translate], whose targets may only be bare program names resolved the same safe way or paths inside the work tree, documented as having the repository's own trust level"
bound = true

[[acceptance]]
text = "Given property tests over arbitrary command lines, when translation runs, then only argv[0] can change, every other argument is byte-identical, and the result is either unchanged or a program resolved from a PATH directory"
bound = true
+++
