+++
id = "01M4RZS8VMJ8TFSA4VTQY8JF8T"
title = "Say what most likely happened when a run loses its connection"
type = "story"
category = "in-progress"
priority = "medium"
points = 5
reporter = "lognd"
created = "2026-10-12T21:20:02Z"
updated = "2026-10-12T21:55:51Z"
scope = ["crates/goway/src/lostlink.rs", "crates/goway/src/run.rs", "crates/goway/src/shard.rs", "crates/goway/src/lib.rs", "crates/goway/src/termfilter.rs", "docs/**", "README.md", "changelog.d/**"]

[[acceptance]]
text = "Given evidence of a wall-clock gap far above the monotonic gap, when a dropped connection is classified, then the verdict is local machine slept and lists the gap"
bound = true

[[acceptance]]
text = "Given the local default route or source address changed or vanished since the run started, when classified, then the verdict is local network changed or lost"
bound = true

[[acceptance]]
text = "Given the re-probe failed, when classified, then the verdict is helper unreachable and names the reason (timeout, refused, no route)"
bound = true

[[acceptance]]
text = "Given exit 143 or a broken lifeline with an answering host, when classified, then the verdict is the helper stopped the job because the client looked gone"
bound = false

[[acceptance]]
text = "Given ssh stderr showing a reset or broken pipe while the host answers, when classified, then the verdict is ssh transport reset with both ends up"
bound = false

[[acceptance]]
text = "Given exit 255 or 143 with no supporting evidence, when classified, then the verdict is the command's own exit, with low confidence stated, and every verdict prints its evidence bullets"
bound = false
+++

Pure classifier in crates/goway/src/lostlink.rs, wired into run.rs stream. Exit codes unchanged (125 for host-gone). No remote verb queries a run by id, so the lifeline state stands in.
