+++
id = "01M4557B4FQ51Y1T49Y0B12Y4D"
title = "Stop sending all of remote.sh on every call: install it once per version on the helper, like remote.ps1"
type = "story"
category = "todo"
priority = "high"
points = 5
reporter = "lognd"
created = "2026-10-05T04:30:18Z"
updated = "2026-10-05T04:30:18Z"
scope = ["crates/goway/src/remote.rs", "crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given a helper without the current script version, When a call runs, Then the client installs it once (by stdin, verified by hash) and retries, and later calls send only a short command line"
bound = false

[[acceptance]]
text = "Given the installed script was modified on the helper, When a call runs, Then the hash check refuses it and the client reinstalls"
bound = false

[[acceptance]]
text = "Given the guard test, When it measures the per-call command line, Then it stays under a few KiB regardless of remote.sh's size"
bound = false
+++

The encoded remote.sh line is 120624 bytes of a hard 131072 per-argument limit (MAX_LINE was raised from 120000 to 124000 to pass); a few more features break every call to every Unix helper at once. remote.ps1 already installs once per version and each call runs the installed copy (NOT_INSTALLED_MARK and retry). Do the same for remote.sh: a short bootstrap line runs ~/.cache/goway/bin/remote-<version>.sh if present and its hash matches, else reports not installed and the client sends the script once by stdin.
