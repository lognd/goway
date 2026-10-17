+++
id = "01M548G3QRTQ230RQQWBA3HJ90"
title = "doctor: ssh-keygen -Y support and clock-skew checks for circle certs"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:23:58Z"
updated = "2026-10-17T07:17:18Z"
scope = ["crates/goway/src/doctor.rs", "docs/circles.md"]

[[acceptance]]
text = "goway doctor reports ssh-keygen without -Y support (OpenSSH < 8.0) and clock skew beyond the circle cert backdate"
bound = true
+++

found while working ~Q5WP11P
