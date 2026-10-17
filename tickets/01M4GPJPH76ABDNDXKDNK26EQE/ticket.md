+++
id = "01M4GPJPH76ABDNDXKDNK26EQE"
title = "Memory estimate ran about 0.5 GiB under the measured peak on the small helper (3.7 GiB estimated, 4.2 GiB of 4.8 GiB used): add a safety margin to the fitted model"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-09T16:05:14Z"
updated = "2026-10-17T10:11:51Z"

[[acceptance]]
text = "Given recorded runs whose peaks exceed the fitted line, when a job count is chosen, then a documented safety margin keeps the estimate at or above every recorded peak at that level, and a test covers it"
bound = true
+++

Found by the per-host fitting ticket's implementer on 2026-10-09 while proving fitting with frob-v2 suite runs.
