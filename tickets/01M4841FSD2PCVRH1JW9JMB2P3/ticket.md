+++
id = "01M4841FSD2PCVRH1JW9JMB2P3"
title = "Send named refs with the minimal .git (with-ref) so ref-backed ledgers resolve on helpers"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-06T08:07:21Z"
updated = "2026-10-06T08:15:59Z"
scope = ["crates/goway/src/**", "crates/goway/tests/with_git.rs", "docs/usage.md", "docs/design.md", "changelog.d/**"]

[[acceptance]]
text = "Given --with-ref REF (repeatable) or with_refs in goway.toml, when the run starts, then the helper's .git has REF at the same commit and git show REF:path reads its tree, with no remotes, credentials, hooks or extra config"
bound = true

[[acceptance]]
text = "Given a REF that does not exist locally, when the run starts, then goway refuses with a clear error before syncing"
bound = true

[[acceptance]]
text = "Given an unchanged HEAD and refs, when run again, then nothing is rebuilt or resent"
bound = true
+++
