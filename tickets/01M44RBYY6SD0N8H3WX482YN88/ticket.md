+++
id = "01M44RBYY6SD0N8H3WX482YN88"
title = "goway changes prints whole fix scripts and unix times, and says undo no for a journaled pinned-tool install"
type = "bug"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T00:45:38Z"
updated = "2026-10-05T00:45:38Z"
scope = ["crates/goway/src/changelog.rs"]

[[acceptance]]
text = "Given a journaled pinned-tool install, When goway changes lists it, Then the row names the tool, version and host in one line, shows a local time, and marks it undoable"
bound = false

[[acceptance]]
text = "Given goway changes undo on that row, When it runs, Then the tool tree and its links are removed through the fix journal"
bound = false
+++

Seen right after the first real doctor --fix uv install: each row is the full curl/tar/ln shell script, the time is 'unix time 1791161006', and the undo column says no although the install is journaled in fixes-HOST.json and can be undone. Rows should name the change (installed uv 0.12.23 user-level on HOST), show a local time, and point at the undo that applies.
