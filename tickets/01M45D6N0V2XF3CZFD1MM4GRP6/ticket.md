+++
id = "01M45D6N0V2XF3CZFD1MM4GRP6"
title = "goway uninstall deletes the journals of changes on helpers without undoing them"
type = "bug"
category = "in-progress"
priority = "high"
points = 3
reporter = "lognd"
created = "2026-10-05T06:49:43Z"
updated = "2026-10-17T09:45:22Z"
scope = ["crates/goway/src/uninstall.rs", "changelog.d/01M45D6N0V2XF3CZFD1MM4GRP6.fixed.md"]

[[acceptance]]
text = "Given active change records for helpers, When goway uninstall runs without --everywhere, Then it keeps those records (or refuses), names each helper with active changes and the command that undoes them (goway uninstall --everywhere, or per host)"
bound = true

[[acceptance]]
text = "Given goway uninstall --everywhere, When it runs, Then each helper's changes are undone first and its record is deleted only after its undo succeeded"
bound = true
+++

goway uninstall --dry-run (without --everywhere) on a machine with helpers lists 'delete' for ~/.config/goway/installed-HOST.json, fixes-HOST.json and ssh-setup-HOST.json, the records goway needs to undo what it changed on each helper (installs, doctor fixes, authorized_keys). Deleting them without undoing the changes orphans those changes: they can no longer be listed or reverted, which breaks the journaling invariant (docs/changes.md).
