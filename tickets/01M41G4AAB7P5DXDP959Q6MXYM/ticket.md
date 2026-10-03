+++
id = "01M41G4AAB7P5DXDP959Q6MXYM"
title = "M8: harden every ssh invocation against forwarding and env sending"
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
text = "Given any goway ssh call, when it is built, then it disables agent, X11 and port forwarding and SendEnv regardless of the user's ssh config"
bound = false
+++
