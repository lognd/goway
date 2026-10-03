+++
id = "01M41Q7VQKB1DCE9BY6K5PTA8D"
title = "Uninstall replays only known undo actions; install journal revert is scoped; cargo undo spares a pre-existing ~/.cargo"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:28:11Z"
updated = "2026-10-03T20:33:31Z"
scope = ["crates/goway/src/uninstall.rs", "crates/goway/src/doctor.rs", "crates/goway/src/remote.sh", "crates/goway/tests/uninstall_records.rs", "crates/goway/tests/install_scripts.rs", "scripts/uninstall.sh", "docs/usage.md"]

[[acceptance]]
text = "Given an installed record naming an unknown check with a hostile undo string, When goway uninstall runs, Then nothing from the record is executed"
bound = true

[[acceptance]]
text = "Given a legacy record with undo and root fields, When it is loaded, Then those fields are ignored and the undo is derived from the check name"
bound = true

[[acceptance]]
text = "Given a journal naming a file outside the install prefix or a profile outside HOME, When the journal is reverted, Then those entries are skipped"
bound = true

[[acceptance]]
text = "Given a CRLF profile with goway PATH line, When the journal is reverted, Then the other lines keep their CRLF bytes exactly"
bound = true

[[acceptance]]
text = "Given rustup existed before doctor --fix, When uninstall runs, Then rustup self uninstall is not run"
bound = false
+++
