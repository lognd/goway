+++
id = "01M44GXPWBS9QVWWPKRXV3QHHT"
title = "Every change goway or goway-setup makes to a machine is journaled, and a check fails the build when one is not"
type = "invariant"
category = "in-progress"
priority = "critical"
points = 5
reporter = "lognd"
created = "2026-10-04T22:35:31Z"
updated = "2026-10-04T23:33:38Z"
scope = ["crates/goway-journal/src/change.rs", "crates/goway-journal/src/journal.rs", "crates/goway-journal/src/plan.rs", "crates/goway-journal/src/apply.rs", "crates/goway-journal/src/lib.rs", "crates/goway-journal/tests/actions.rs", "crates/goway-setup/src/host.rs", "crates/goway-setup/src/app.rs", "crates/goway-setup/src/render.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/tests/app.rs", "crates/goway/src/changelog.rs", "crates/goway/src/lib.rs", "crates/goway/src/cli.rs", "crates/goway/src/config.rs", "crates/goway/src/hosts.rs", "crates/goway/src/doctor/windows.rs", "crates/goway/src/add.rs", "crates/goway/tests/journal_invariant.rs", "crates/goway/tests/changes_cmd.rs", "docs/changes.md", "docs/config.md", "Cargo.toml", "crates/goway-setup/tests/elevated.rs", "crates/goway-setup/src/tune.rs", "crates/goway-setup/tests/tune.rs", "crates/goway/src/doctor.rs", "crates/goway/src/uninstall.rs", "crates/goway/src/error.rs", "crates/goway/src/sshsetup.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway/src/remotesys.rs", "crates/goway/src/doctor/projneeds.rs", "crates/goway/src/doctor/prereq.rs", "crates/goway/tests/pinned_tools.rs", "crates/goway-journal/src/model.rs"]

[[acceptance]]
text = "Given the audit, When it is complete, Then every host-mutation site is listed in the done report with the journal entry it now produces, or the reason it is out of scope (goway run state)"
bound = false

[[acceptance]]
text = "Given a new mutating command or file write added outside the journal-backed modules, When the test suite runs, Then a test fails naming the file and the verb"
bound = false

[[acceptance]]
text = "Given a non-invertible action such as starting a scheduled task or shutting WSL down, When goway performs it, Then the journal records it with time, host and reason, and undo reports it as not reversible instead of silently skipping it"
bound = false

[[acceptance]]
text = "Given the docs, When a reader looks up what goway changes on a machine, Then one page lists every change kind, how to list the journal and how to undo"
bound = false
+++

Owner rule: every change to a machine goes through goway-journal so it can be listed and undone; enforce it in the codebase, not by review. Audit every mutation site in goway and goway-setup (files, registry, scheduled tasks, firewall, portproxy, services, packages, units, wsl.conf/.wslconfig, ssh keys/authorized_keys/config, the user's goway config, doctor --fix actions, loginctl linger, wsl --shutdown/restarts, starting tasks) and route each through the journal. Actions that cannot be inverted (start a task, restart WSL) get a recorded, non-invertible entry kind so the journal is still complete. Excluded by design: goway's own run state (slot trees, caches, footprints, locks, run scopes), which gc owns; document that boundary. Enforcement: a structural test (and clippy disallowed-methods where it fits) that fails when a mutating command or script verb (Register-ScheduledTask, schtasks /create|/delete|/run, netsh, New-NetFirewallRule, Set-ItemProperty, reg add, apt/apt-get/dnf install|remove, systemctl enable|disable|mask, loginctl enable-linger, wsl --shutdown|--terminate, std::fs writes outside the System impl) appears outside the journal-backed modules, with a reviewed allowlist.
