+++
id = "01M42MNTTGT910S380T82TBT2S"
title = "Many concurrent runs over one ssh master hit the helper's MaxSessions: refused sessions and stale control sockets must be handled quietly"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T05:02:38Z"
updated = "2026-10-04T05:24:59Z"
scope = ["crates/goway/src/ssh.rs", "crates/goway/src/paths.rs", "crates/goway/tests/ssh_mux.rs", "docs/troubleshooting.md", "crates/goway/src/ssh/mux.rs"]

[[acceptance]]
text = "Given more concurrent goway calls to one helper than its sshd MaxSessions (default 10) allows over one ControlMaster (found live: 'mux_client_request_session: session request failed: Session open refused by peer' then 'ControlSocket ... already exists, disabling multiplexing'), when a session is refused, then goway opens the call without multiplexing or through a second master, prints nothing to the user unless it repeats (one note per run at most), and never leaves a stale control socket behind"
bound = false

[[acceptance]]
text = "Given a stale control socket (its master is gone), when goway starts, then it removes and replaces it instead of disabling multiplexing for the rest of the run"
bound = false
+++
