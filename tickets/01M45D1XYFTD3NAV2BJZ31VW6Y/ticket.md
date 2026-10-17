+++
id = "01M45D1XYFTD3NAV2BJZ31VW6Y"
title = "install.sh piped into sh (dash) fails with a cryptic 'set: Illegal option -o pipefail'"
type = "bug"
category = "in-progress"
priority = "low"
points = 1
reporter = "lognd"
created = "2026-10-05T06:47:09Z"
updated = "2026-10-17T08:43:14Z"
scope = ["scripts/install.sh", "crates/goway/tests/install_scripts.rs"]

[[acceptance]]
text = "Given install.sh piped into dash, When it runs, Then it either completes through bash or stops with a one-line message naming the right command, never a shell syntax error"
bound = true
+++

The documented command is 'curl ... | bash' and works. 'curl ... | sh' is a common habit and on Debian/Ubuntu (sh is dash) it dies at line 24 with 'set: Illegal option -o pipefail' and curl's 'Failed writing body'. The script should detect a non-bash shell before any bash-only syntax and either re-exec itself in bash when bash exists (it reads from a pipe, so re-exec needs the script text) or stop with 'run it with bash: curl ... | bash'.
