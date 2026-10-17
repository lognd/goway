+++
id = "01M4WB91TXX36PQBHJ862N3Z9H"
title = "install --host --distro defaults to 'Ubuntu' instead of the installed or default WSL distro"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-14T04:38:37Z"
updated = "2026-10-17T10:11:34Z"
scope = ["crates/goway-setup/src/host.rs", "crates/goway-setup/src/cli.rs"]

[[acceptance]]
text = "With no --distro, goway-setup uses WSL's default distro, or the only installed one, when it is an Ubuntu/Debian distro"
bound = true

[[acceptance]]
text = "When several candidates exist and none is the default, the error lists them and suggests --distro NAME instead of wsl --install"
bound = true
+++

DEFAULT_DISTRO is the literal "Ubuntu" (crates/goway-setup/src/host.rs:22). On Nova the only distro is Ubuntu-22.04, so install --host stopped with NoDistro { installed: ["Ubuntu-22.04"] }. It then told the user to run `wsl --install -d Ubuntu` and restart, which installs a second distro they do not need. Only passing --distro Ubuntu-22.04 by hand worked.
