+++
id = "01M43FANDGFZY0NRW51JGBJ33M"
title = "macOS CI: translate tests assume a symlink-free temp dir and pwsh on /usr/bin"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T12:48:23Z"
updated = "2026-10-04T12:48:27Z"
scope = ["crates/goway/tests/translate.rs"]

[[acceptance]]
text = "Given a temp dir behind a symlink and pwsh outside /usr/bin, When the translate tests run, Then they pass on macOS and Linux"
bound = false
+++

On macOS the temp dir lives under the /var symlink to /private/var, and resolve (rightly) answers with the real path; the powershell test overrides PATH when spawning pwsh, so a pwsh outside /usr/bin is not found. Found while working D6024D0.
