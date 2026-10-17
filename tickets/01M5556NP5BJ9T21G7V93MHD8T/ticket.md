+++
id = "01M5556NP5BJ9T21G7V93MHD8T"
title = "pair-host --qr does not detect the LAN address"
type = "task"
category = "todo"
priority = "low"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T14:45:37Z"
updated = "2026-10-17T14:45:37Z"
idempotency_key = "gap-qrlan"
labels = ["circles"]

[[acceptance]]
text = "Given pair-host --qr, when no address is passed, then the LAN address is detected and encoded"
bound = false
+++
