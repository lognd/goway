+++
id = "01M4GPJFQAPBRSEJJT4F2WWHE0"
title = "Every long run ends with 'the lifeline connection to <host> broke while the job ran (ssh exit status: 0)'"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-09T16:05:07Z"
updated = "2026-10-17T08:41:02Z"

[[acceptance]]
text = "Given the condition in the title, when a fix lands, then a test reproduces it and the condition no longer occurs"
bound = false
+++

Found by ~K60F05J's implementer on 2026-10-09 while proving per-host fitting with frob-v2 runs.
