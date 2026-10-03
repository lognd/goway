+++
id = "01M41G49PSNGVFHZGPM91P2Y64"
title = "M5: --env values never in remote argv, logs or reports"
type = "security"
category = "todo"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:55Z"
updated = "2026-10-03T18:23:55Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given goway run -vv -e TOKEN=sentinel, when it runs, then the sentinel appears neither in goway's log output nor in the remote process command line"
bound = false
+++
