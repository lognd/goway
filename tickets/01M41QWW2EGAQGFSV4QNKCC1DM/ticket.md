+++
id = "01M41QWW2EGAQGFSV4QNKCC1DM"
title = "Install and uninstall through uv, pipx, pip and cargo: documented, and goway uninstall removes the program however it was installed"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:39:40Z"
updated = "2026-10-03T22:54:57Z"
scope = ["README.md", "docs/install-linux.md", "crates/goway/src/uninstall.rs", ".github/workflows/ci.yml", "crates/goway/tests/install_methods.rs", "docs/install-methods.md"]

[[acceptance]]
text = "Given README.md, when a reader wants goway from PyPI or crates.io, then it explains uv (preferred, with a link to its installer), pipx, pip in a virtual environment (and why system pip is refused on Debian and Ubuntu), and cargo install (with a link to rustup.rs), each with where the program lands, how to get it on PATH, how to update and how to remove it"
bound = false

[[acceptance]]
text = "Given goway installed by cargo install, uv tool install, pipx or pip in a virtual environment, when goway uninstall runs, then its plan names the exact removal command for that method and, after confirmation, runs it last (printing it instead on Windows)"
bound = false

[[acceptance]]
text = "Given a goway installed with uv from the built wheel in CI, when goway uninstall --yes runs, then the program, its settings and its uv tool environment are gone"
bound = false
+++
