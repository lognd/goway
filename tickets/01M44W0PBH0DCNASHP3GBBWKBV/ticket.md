+++
id = "01M44W0PBH0DCNASHP3GBBWKBV"
title = "A short Windows run warns that its lifeline broke although the job finished normally"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:49:23Z"
updated = "2026-10-05T01:49:23Z"
scope = ["crates/goway/src/run.rs"]

[[acceptance]]
text = "Given a run whose lifeline connection ends with status 0 at or after the job's end, When the run finishes, Then no lifeline warning is printed"
bound = false

[[acceptance]]
text = "Given a lifeline that ends while the job is still running, When the run finishes, Then the warning is still printed"
bound = false
+++

Seen on 01fb3f47: goway run --host <windows host> -- cmd /c ver printed the output, then 'warning: the lifeline connection to <host> broke while the job ran (ssh exit status: 0) ... unexpected exit 143', then 'done: exit 0'. The lifeline ended cleanly (status 0) as the job finished; that is not a break and must not warn.
