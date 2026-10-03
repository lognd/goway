+++
id = "01M418J36509PKTMG2WQQRVXW2"
title = "Install journal core with provable inverse"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:14:38Z"
scope = ["crates/goway-setup/**", "docs/**", "crates/goway-journal/**", "Cargo.lock"]

[[acceptance]]
text = "Given any initial model system and component selection, when install then uninstall replay the journal, then the system equals the initial state (property test)"
bound = false
+++
