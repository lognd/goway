+++
id = "01M41G4AG3AFPNS7Y52RHVPGCP"
title = "M9: per-user sccache endpoint instead of a predictable shared TCP port"
type = "security"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:55Z"
updated = "2026-10-03T18:23:55Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a run with sccache, when it starts the cache server, then the server is reachable only by the owning user"
bound = false
+++
