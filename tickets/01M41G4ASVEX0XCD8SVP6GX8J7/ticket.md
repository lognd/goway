+++
id = "01M41G4ASVEX0XCD8SVP6GX8J7"
title = "L1-L6: non-UTF-8 names, atomic private writes, 0700 control dir, true key-only verify, pinned doctor downloads, safe profile line"
type = "security"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:56Z"
updated = "2026-10-03T18:23:56Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given each LOW finding L1-L6 of the 2026-10-03 audit, when its regression test runs, then the unsafe behaviour is gone"
bound = false
+++
