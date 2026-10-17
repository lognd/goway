+++
id = "01M54BYD885RDTAR95TXQGX5E1"
title = "Push the circle roster to helpers on revoke --now so they adopt the new epoch"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T07:24:12Z"
updated = "2026-10-17T07:40:58Z"
idempotency_key = "4EW94KV-roster-push"
+++

found while working ~4EW94KV: revoke --now only reruns goway circle access on helpers, which uses the helper's own roster copy; push the roster ops files too. Also store issued certificate expiry per member so list can show it for other machines.
