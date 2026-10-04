+++
id = "01M42FC4N6TYP7M8A4QWCBRFMA"
title = "--needs terms are shell-safe: accept cores:8 and mem:2G forms, and hint when a bare key suggests an unquoted >= redirect"
type = "story"
category = "in-progress"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T03:29:57Z"
updated = "2026-10-04T05:29:35Z"
scope = ["crates/goway/src/needs.rs", "crates/goway/src/error.rs", "docs/usage.md"]

[[acceptance]]
text = "Given --needs cores:8 (or mem:2G, gpu-mem:8G, disk:50G, cuda:12.1), when goway parses it, then it means the same minimum as cores>=8, so no quoting is needed; >= stays accepted"
bound = true

[[acceptance]]
text = "Given a bare key that needs a value (cores, mem, disk, gpu-mem, cuda), when it is used, then the error says that an unquoted >= is read by the shell as a redirect into a file, names the file it probably created (for example =8), and shows both cores:8 and 'cores>=8'"
bound = false
+++
