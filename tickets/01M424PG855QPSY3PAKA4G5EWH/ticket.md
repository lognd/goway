+++
id = "01M424PG855QPSY3PAKA4G5EWH"
title = "WSL started from an elevated process gives every WSL user Windows admin through interop; goway must never start WSL elevated and must detect it"
type = "security"
category = "in-progress"
priority = "critical"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:23:23Z"
updated = "2026-10-04T02:05:52Z"
scope = ["crates/goway-setup/src/relay.rs", "crates/goway-setup/src/ps.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/host.rs", "crates/goway-setup/tests/relay.rs", "crates/goway-setup/tests/hostsys.rs", "crates/goway/src/doctor.rs", "crates/goway/src/facts.rs", "crates/goway/src/remote.sh", "crates/goway/tests/host_facts.rs", "docs/install-windows.md", "SECURITY.md", "crates/goway-setup/src/cli.rs", "crates/goway/src/status.rs", "crates/goway-setup/src/error.rs", "crates/goway-setup/tests/host_plan.rs"]

[[acceptance]]
text = "Given goway-setup's elevated code (the elevated install or uninstall child and the NAT relay refresh task, which runs with highest privileges), when it needs a WSL distro, then it never starts WSL: it only queries distros already running (wsl.exe --list --running), and anything that must start WSL does so through a limited token (the Limited keepalive task), so WSL's interop never inherits an elevated token from goway"
bound = true

[[acceptance]]
text = "Given a helper whose WSL was started elevated (interop powershell.exe runs with the Administrators role enabled), when goway doctor or status probes it, then it reports a security warning: anyone who can log in to that WSL can act as a Windows administrator; with the fix (stop WSL with wsl --shutdown from a normal, non-admin terminal; it restarts limited through the keepalive task)"
bound = true

[[acceptance]]
text = "Given SECURITY.md and docs/install-windows.md, when read, then they explain that WSL interop runs Windows programs with the token of whatever started WSL, that goway never starts it elevated, and that an admin ssh session on the Windows side must not start WSL"
bound = true

[[acceptance]]
text = "Given goway-setup install --host --keepalive boot (an S4U task, needed to start WSL before anyone logs in), when it is chosen, then goway explains that S4U tasks of an administrator get the full admin token whatever their run level (verified on a real helper on 2026-10-03: a Limited S4U keepalive still produced elevated interop), and either (a) also disables WSL interop for that distro through a journaled /etc/wsl.conf [interop] enabled=false (restored on uninstall), or (b) refuses boot mode for administrator accounts unless --allow-elevated-wsl is given; the default logon keepalive (Interactive, filtered token) stays the recommended mode"
bound = true
+++
