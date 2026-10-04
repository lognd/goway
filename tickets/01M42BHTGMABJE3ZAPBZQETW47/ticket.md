+++
id = "01M42BHTGMABJE3ZAPBZQETW47"
title = "Networks that hide helpers: client isolation, blocked mDNS, VPN-only addresses, duplicate names"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:09Z"
updated = "2026-10-04T11:58:07Z"
scope = ["crates/goway/src/resolve.rs", "crates/goway/src/error.rs", "docs/hosts.md", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a helper that resolves but cannot be reached (guest Wi-Fi client isolation, a VPN that routes only some addresses), or two machines answering for one name, when goway fails to reach or confirm it, then the message says which case it looks like and what to do (a different network, an address in the config, renaming one machine), and duplicate answers never pick an unconfirmed key"
bound = true
+++
