+++
id = "01M452F1868GJRP418YRV593VK"
title = "remote.sh is within 300 bytes of the 128 KiB single-argument limit that tests (and any bash -c delivery) hit"
type = "bug"
category = "todo"
priority = "critical"
points = 2
reporter = "lognd"
created = "2026-10-05T03:42:04Z"
updated = "2026-10-05T03:56:53Z"
scope = ["crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given remote.sh grows past 131072 bytes, When the tests and the client run it, Then nothing fails with Argument list too long"
bound = false
+++

found while working ~ZQ447N2: remote.sh grew to 131224 bytes with one new function and every test that runs it as one bash -c argument failed with E2BIG (MAX_ARG_STRLEN is 131072); it is 130815 bytes now. Deliver the script by stdin or file in those tests, and check the real client path, or split the script.
