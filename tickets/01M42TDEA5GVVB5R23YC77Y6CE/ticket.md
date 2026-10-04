+++
id = "01M42TDEA5GVVB5R23YC77Y6CE"
title = "The probe verb reports the host epoch"
type = "task"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T06:42:54Z"
updated = "2026-10-04T11:17:35Z"
scope = ["crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given the probe verb, when it runs on a Linux or macOS host, then it prints epoch=SECONDS since the Unix epoch"
bound = false
+++

Split from ~TGZTRVF because remote.sh was leased. Adds epoch=SECONDS to the probe output; remote.ps1 mirrors it (lane A).
