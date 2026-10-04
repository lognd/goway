+++
id = "01M42B5FH3SSX7Z89VQPYDHSTK"
title = "Windows mDNS lookup asks for A records only, which fails for Linux (avahi) hosts; ask without a type and keep the IPv4 answers"
type = "bug"
category = "todo"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:16:25Z"
updated = "2026-10-04T02:16:25Z"
scope = ["crates/goway/src/resolve.rs"]

[[acceptance]]
text = "Given a Linux helper advertised by avahi (found live: Resolve-DnsName NAME.local -Type A fails with 'DNS name does not exist' while the untyped query returns its IPv4 and IPv6 link-local addresses), when goway resolves it through the Windows lookup, then it gets the IPv4 address; IPv6 link-local answers are ignored; Windows hosts resolve as before"
bound = false
+++
