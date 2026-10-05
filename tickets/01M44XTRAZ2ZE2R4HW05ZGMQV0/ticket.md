+++
id = "01M44XTRAZ2ZE2R4HW05ZGMQV0"
title = "gc and status count hard-linked files once per link, so seeds look many times their real size"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T02:21:05Z"
updated = "2026-10-05T02:21:05Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/gc.rs"]

[[acceptance]]
text = "Given a seed whose files are hard-linked into slot trees, When gc or status reports its size, Then each inode is counted once across the entries (or shared bytes are shown separately), matching du"
bound = false
+++

On a helper, gc --dry-run listed each goway seed at about 4.4 GiB and a frob-v2 seed at 75 GiB, while du of the whole seed dir was 62 MB: seed files are hard-linked into slot trees and work dirs (a README had 18 links), and the size sum counts each link. The real space user was a 235 GiB build cache. Misleading sizes send the user after the wrong entries.
