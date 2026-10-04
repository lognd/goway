+++
id = "01M43EG3SHVE0BBZHEQGDD7F0D"
title = "Detect and restart a broken shared sccache server before a run uses it (a server left over from an older goway keeps failing every build)"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T12:33:53Z"
updated = "2026-10-04T12:43:30Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/sccache_tmpdir.rs", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a per-repository sccache server that is running but broken (its TMPDIR was deleted, it answers with an error, or its socket is dead), when a run is about to use sccache, then goway detects it cheaply (for example sccache --show-stats over the socket, or a check that the server's TMPDIR still exists), stops it and starts a fresh one with the stable TMPDIR, says so in one note, and the build works; found live: every cargo run on a helper failed with 'sccache: Failed to create temp dir' while the long-lived server stayed busy, so its idle timeout never fired"
bound = false

[[acceptance]]
text = "Given a test that starts a server under a TMPDIR, deletes that dir, then runs a build through goway, when the build runs, then it succeeds and the server was restarted"
bound = false
+++
