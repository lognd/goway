+++
id = "01M418J2R4E5AMB16T81JJVK4G"
title = "Address resolution without static IPs: cache, resolver, mDNS, WSL interop"
type = "task"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:24:13Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a host whose cached address fails, when goway resolves it, then it tries the next candidates in order and caches the first that passes the host key check"
bound = true

[[acceptance]]
text = "Given goway runs under WSL in NAT mode, when NAME.local does not resolve natively, then goway asks Windows via interop and parses the answer"
bound = true
+++
