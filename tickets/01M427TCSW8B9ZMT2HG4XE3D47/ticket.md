+++
id = "01M427TCSW8B9ZMT2HG4XE3D47"
title = 'Windows run: put Git for Windows usr\bin on PATH when sh is missing, like CI images'
type = "story"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T01:17:56Z"
updated = "2026-10-04T01:19:45Z"
scope = ["crates/goway/src/remote.ps1", "docs/usage.md"]

[[acceptance]]
text = '''Given a Windows host where sh is not on PATH but Git for Windows is installed, when a command runs, then Git's usr\bin is appended to its PATH so tests that spawn sh pass as on CI'''
bound = false
+++

found while working XKXW2BJ: 5 gob-check, 5 gob-exec and 7 frob-check frob-v2 tests failed only for lack of sh.
