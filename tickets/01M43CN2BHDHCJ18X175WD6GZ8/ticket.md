+++
id = "01M43CN2BHDHCJ18X175WD6GZ8"
title = "Local job slot race: a slot file created but not yet locked is counted as free and deleted"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T12:01:39Z"
updated = "2026-10-04T12:01:46Z"
scope = ["crates/goway/src/local.rs"]

[[acceptance]]
text = "Given a slot file that exists but is not locked yet, when running_jobs runs, then it counts the file as running and does not delete it until it is older than a grace period"
bound = false
+++

found while working the queue: local_host max_jobs test got exit 0 instead of 125 under load; running_jobs saw the new file before its flock and removed it.
