+++
id = "01M41QKMG4K8M78GBJJP2C05F5"
title = "Security audit 3: persistent slot trees, Windows hosts, language adapters, publishing"
type = "security"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:34:37Z"
updated = "2026-10-04T21:49:28Z"
scope = ["crates/**", ".github/**", "scripts/**"]

[[acceptance]]
text = "Given the code added since audit 2, when a pessimistic audit runs, then every HIGH and MEDIUM finding is fixed with a regression test before v0.1.0"
bound = true
+++
