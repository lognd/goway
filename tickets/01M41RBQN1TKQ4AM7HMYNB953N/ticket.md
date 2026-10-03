+++
id = "01M41RBQN1TKQ4AM7HMYNB953N"
title = "Audit 2 leftovers: resolve keeps trying candidates after a host-key refusal (L14), tar leaf TOCTOU and NTFS junctions (L8), helper resource limits (L10)"
type = "security"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:47:47Z"
updated = "2026-10-03T23:23:50Z"
scope = ["crates/goway/src/resolve.rs", "crates/goway/src/sync.rs", "crates/goway/src/config.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/config.md"]

[[acceptance]]
text = "Given a LAN attacker answering first for a host name with an unknown key, when goway resolves the host, then it tries the remaining candidates before failing"
bound = true

[[acceptance]]
text = "Given a file swapped for a symlink between listing and archiving, or a path under an NTFS junction, when goway syncs, then the file is not followed out of the work tree"
bound = true

[[acceptance]]
text = "Given a helper, when max_jobs is unset, then a sane default limit applies and docs explain disk and memory use and how to cap them"
bound = true
+++
