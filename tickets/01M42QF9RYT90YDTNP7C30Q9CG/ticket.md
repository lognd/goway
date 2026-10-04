+++
id = "01M42QF9RYT90YDTNP7C30Q9CG"
title = "Doctor names the proxy variables set on a helper, never their values"
type = "story"
category = "todo"
priority = "medium"
points = 1
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T05:51:30Z"
updated = "2026-10-04T05:51:30Z"
scope = ["crates/goway/src/doctor.rs"]

[[acceptance]]
text = "Given a helper whose environment sets HTTPS_PROXY or similar, when goway doctor checks it, then it names the proxy variables set (from the proxy_vars host fact) and never prints their values"
bound = false
+++

Split from ~E51QN55: the remote.sh proxy_vars fact lands there; this shows it in doctor.
