+++
id = "01M42BHTRYPX7PM14FQWR8SNXP"
title = "SELinux or AppArmor denials on helpers are detected and explained"
type = "story"
category = "in-progress"
priority = "low"
points = 1
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:10Z"
updated = "2026-10-04T06:30:47Z"
scope = ["crates/goway/src/doctor.rs", "docs/troubleshooting.md", "crates/goway/src/doctor/mac.rs"]

[[acceptance]]
text = "Given a helper whose security module denies goway's root, setsid or flock, when a run fails that way, then doctor reports the denial (from ausearch or dmesg when readable) and the label or profile change to make"
bound = true
+++
