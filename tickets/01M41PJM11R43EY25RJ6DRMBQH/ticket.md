+++
id = "01M41PJM11R43EY25RJ6DRMBQH"
title = "Purge removes only goway's own entries; mark_root and remote_root guard foreign directories"
type = "security"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:16:35Z"
updated = "2026-10-03T20:18:51Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/config.rs", "crates/goway/tests/remote_root.rs", "docs/config.md", "crates/goway/tests/common/mod.rs"]

[[acceptance]]
text = "Given a root directory holding a foreign file and no marker, When goway receives into it, Then it refuses and the file survives"
bound = true

[[acceptance]]
text = "Given a marked root that also holds a foreign file, When purge runs, Then only work, seed, cache and the marker are removed and the foreign file stays"
bound = true

[[acceptance]]
text = 'Given remote_root = ".ssh", When the config is validated, Then it is rejected; the default .cache/goway still passes'
bound = true
+++
