+++
id = "01M44R1CZMASJAK2HHC2XKB1QF"
title = "The journal invariant flags the hung-WSL recovery steps doctor prints as advice"
type = "bug"
category = "in-progress"
priority = "critical"
points = 1
reporter = "lognd"
created = "2026-10-05T00:39:52Z"
updated = "2026-10-05T00:40:41Z"
scope = ["crates/goway/tests/journal_invariant.rs"]

[[acceptance]]
text = "Given the wsl_down.rs allowlist entry, When the invariant test runs on main, Then it passes, and the entry's reason says the recovery steps are printed advice and the probes are read-only"
bound = true
+++

The invariant from ~XV3QHHT landed before ~ENKASAM's hung-WSL finding, whose printed recovery steps (Start-Service sshd, wsl --shutdown) the scanner reads as machine changes; main fails journal_invariant. wsl_down.rs only prints them; its probes stay read-only.
