+++
id = "01M548GAQ2DM0A38BZPWF3BY7Y"
title = "Triage ~20 failing goway tests on main (run_local, slot_trees, client_loss, cmake_api, clock, copy_integrity) and 84 exec-failures"
type = "bug"
category = "todo"
priority = "medium"
reporter = "lognd"
created = "2026-10-17T06:24:05Z"
updated = "2026-10-17T06:24:05Z"
+++

Phase agents report 19-21 failures plus 84 exec-failed in the full -p goway -p goway-circle run. run_local also fails on a clean main. Unconfirmed whether it is environment (WSL, load, missing cmake) or real. Re-run on main, separate environment from real failures, fix or gate.
