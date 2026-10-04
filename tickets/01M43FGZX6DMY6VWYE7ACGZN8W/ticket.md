+++
id = "01M43FGZX6DMY6VWYE7ACGZN8W"
title = "macOS CI: footprint tests' fake df is shadowed by Homebrew GNU tools first on PATH"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T12:51:51Z"
updated = "2026-10-04T15:30:18Z"
scope = ["crates/goway/tests/footprint.rs"]

[[acceptance]]
text = "Given Homebrew GNU tools first on PATH, When the footprint tests run, Then the fake df is the one remote.sh sees on macOS and Linux"
bound = true
+++

remote.sh puts Homebrew gnubin first on macOS, so the fake df in the world bin is never reached and the three disk-pressure tests see the real disk. Export the fake as a bash function (BASH_FUNC_df%%), which beats any PATH entry. Found while working D6024D0.
