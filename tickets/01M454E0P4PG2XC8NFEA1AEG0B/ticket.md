+++
id = "01M454E0P4PG2XC8NFEA1AEG0B"
title = "Full-suite run: a stray file-redirect noise, the line guard, and one E2BIG test remain"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-05T04:16:28Z"
updated = "2026-10-05T04:16:30Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/remote.rs", "crates/goway/tests/slot_trees.rs", "crates/goway/tests/client_loss.rs"]

[[acceptance]]
text = "Given a run on a helper with processes of other users, When the run starts, Then no environ permission error reaches its output (redirect order of the tmpdir probe)"
bound = false

[[acceptance]]
text = "Given the full suite, When it runs, Then the encoded-line guard and the slot_trees script test pass (MAX_LINE raised to a limit with headroom, script read from its path)"
bound = false
+++

found while running the full suite for ~V630TRA and ~5HN8AZK: line 2827 of remote.sh opens environ before silencing stderr; remote.sh grew past MAX_LINE (a real fix is to split the script, not raise the limit); slot_trees passes the script as one bash -c argument
