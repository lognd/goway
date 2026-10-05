+++
id = "01M452F1868GJRP418YRV593VK"
title = "remote.sh is within 300 bytes of the 128 KiB single-argument limit that tests (and any bash -c delivery) hit"
type = "bug"
category = "todo"
priority = "critical"
points = 2
reporter = "lognd"
created = "2026-10-05T03:42:04Z"
updated = "2026-10-05T03:57:39Z"
scope = ["crates/goway/src/remote.rs", "crates/goway/tests/disk_budget.rs", "crates/goway/tests/remote_root.rs", "crates/goway/tests/footprint.rs", "crates/goway/tests/scratch_fs.rs", "crates/goway/tests/mem_footprint.rs", "crates/goway/tests/translate_repick.rs", "crates/goway/tests/drift_refresh.rs", "crates/goway/tests/translate.rs", "crates/goway/tests/evict_live.rs", "crates/goway/tests/wsl_drive.rs", "crates/goway/tests/probe_epoch.rs", "crates/goway/tests/user_tool_path.rs", "crates/goway/tests/clock.rs"]

[[acceptance]]
text = "Given remote.sh grows past 131072 bytes, When the tests run it from a file, Then nothing fails with Argument list too long"
bound = false

[[acceptance]]
text = "Given the longest realistic verb and arguments, When the encoded command line would exceed MAX_LINE, Then a unit test fails naming the current size and the limit"
bound = false
+++

found while working ~ZQ447N2: remote.sh grew to 131224 bytes with one new function and every test that runs it as one bash -c argument failed with E2BIG (MAX_ARG_STRLEN is 131072); it is 130815 bytes now. Deliver the script by stdin or file in those tests, and check the real client path, or split the script.
