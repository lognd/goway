+++
id = "01M42JQMHSDJS2ASHQEB9V2YY4"
title = "owner_awareness tests pass on macOS: niceness is relative to the runner's own, idle time is shimmed"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T04:28:40Z"
updated = "2026-10-04T04:28:48Z"
scope = ["crates/goway/tests/owner_awareness.rs"]

[[acceptance]]
text = "Given the macOS CI job, when owner_awareness runs, then every test passes whatever niceness the runner has and whatever its HID idle time is"
bound = false
+++

found by main CI run on macOS after FMD718W landed: base nice -10 and a live HID idle time
