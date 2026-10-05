+++
id = "01M44QW7SEHV2EJP5H1EPZMT2B"
title = "goway-setup tune accepts a WSL memory size that starves Windows, and doctor does not flag it"
type = "bug"
category = "todo"
priority = "high"
points = 3
reporter = "lognd"
created = "2026-10-05T00:37:02Z"
updated = "2026-10-05T00:37:02Z"
scope = ["crates/goway-setup/src/tune.rs", "crates/goway-setup/tests/cli.rs", "crates/goway/src/doctor/windows.rs"]

[[acceptance]]
text = "Given a requested WSL memory that leaves Windows less than a documented headroom (at least 3 GB, more on large machines), When goway-setup tune runs, Then it refuses with the machine's RAM, the requested size and the largest safe size, unless --force is given"
bound = false

[[acceptance]]
text = "Given tune with no explicit size and a request to give WSL more memory, When it computes a size, Then the result leaves the documented headroom"
bound = false

[[acceptance]]
text = "Given a helper whose .wslconfig memory leaves Windows under the headroom, When goway doctor checks that helper, Then it warns with the safe size and the journaled tune command that sets it"
bound = false
+++

Observed 2026-10-04: a helper with 7.4 GB of RAM was tuned to memory=5632MB, leaving Windows under 2 GB. The Windows event log recorded 'The paging file is too small for this operation to complete' at 16:58; WSL died two minutes later, and later deadlocked twice under concurrent test runs, taking the Windows OpenSSH server down with it. tune takes the size as given with no check against the machine.
