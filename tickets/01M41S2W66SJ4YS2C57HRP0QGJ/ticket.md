+++
id = "01M41S2W66SJ4YS2C57HRP0QGJ"
title = "NAT relay task runs elevated in the user's environment: scrub runtime-injection variables (COR_PROFILER and friends)"
type = "security"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:00:25Z"
updated = "2026-10-03T21:00:25Z"
scope = ["crates/goway-setup/**"]

[[acceptance]]
text = "Given a user-level COR_PROFILER variable, when the relay refresh task runs, then no code from it is loaded into the elevated process"
bound = false
+++

The refresh task runs PowerShell with the highest privileges as the invoking user, so a same-account process that can set user-level environment variables (COR_ENABLE_PROFILING, COR_PROFILER, COMPlus_*) or HKCU\Software\Microsoft\Command Processor\AutoRun can get code into the elevated process. The script file, its paths and the system directory are already administrator-only or absolute. Fix direction: launch through a non-.NET stub that clears the variables, or replace the PowerShell script by a small signed helper; document the residual until then. Found in the NAT relay review.
