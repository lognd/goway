+++
id = "01M54ESS8WFQWH0V6P9CYZ0C0P"
title = "Circle cert expiry: learn expiry of certs issued by other admins"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T08:14:07Z"
updated = "2026-10-17T09:11:37Z"
scope = ["crates/goway/src/circle_push.rs", "crates/goway/src/circle.rs", "crates/goway/src/access.rs", "crates/goway/tests/circle_revoke_now.rs"]
+++

Left over from ~NA776V1: list shows cert expiry only for this machine and certs this machine issued. Certs minted by another admin are unknown here; carry expiry in the roster or have members report it. Also: push error classification (host key / auth) on the stdin ssh path is plain stderr text.
