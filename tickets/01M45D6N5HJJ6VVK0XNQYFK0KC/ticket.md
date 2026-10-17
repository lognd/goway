+++
id = "01M45D6N5HJJ6VVK0XNQYFK0KC"
title = "install.sh cannot upgrade an existing install: it says run goway uninstall first"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-05T06:49:44Z"
updated = "2026-10-17T08:59:51Z"
scope = ["scripts/install.sh", "crates/goway/tests/install_scripts.rs"]

[[acceptance]]
text = "Given an install recorded in the install journal, When install.sh runs again with a newer release, Then it replaces the binary, updates the journal, and goway --version shows the new version"
bound = true

[[acceptance]]
text = "Given the installed binary was changed by hand (hash differs from the journal), When install.sh runs, Then it refuses and names the file"
bound = true
+++

Re-running the documented installer over an older install stops with 'already installed (journal ...); run goway uninstall first'. A pre-release build may not even have uninstall, and uninstall removes far more than the binary. The installer should upgrade in place: verify the new download, replace the binary it installed (only if the file on disk still matches the journal's hash, else refuse naming the file), and update the journal's hash, so uninstall stays exact.
