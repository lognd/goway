+++
id = "01M41G49ZQGA15P0674EA0DRCN"
title = "M6: validate host names at the CLI boundary before any side effect"
type = "security"
category = "todo"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:55Z"
updated = "2026-10-03T18:23:55Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given goway host add 'a*' or goway ssh setup ../x, when it runs, then it fails before writing known_hosts or any file"
bound = false
+++
