+++
id = "01M4WB98GJNV6VG7GHZYPB8VS9"
title = "doctor warns 'no ssh key found' when the host config names goway's own identity key"
type = "bug"
category = "in-progress"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-14T04:38:44Z"
updated = "2026-10-17T09:41:48Z"
scope = ["crates/goway/src/sshenv.rs"]

[[acceptance]]
text = "The local ssh check counts a host's configured identity (when its .pub exists) as a key and does not warn"
bound = true

[[acceptance]]
text = "It still warns when neither the agent, the ssh defaults nor the host's identity provides a key"
bound = false
+++

On a fresh client after goway add (0.1.1), goway created ~/.config/goway/id_ed25519 and wrote `identity = <that path>` into each [[host]]. Key login works and goway run succeeds, yet the doctor shown at the end of add reports "WARN local ssh no ssh key found (no agent keys, no identity file); run `goway ssh setup HOST`" for every host. The local-ssh check (crates/goway/src/sshenv.rs) looks only at agent keys and ssh -G defaults and ignores the per-host identity.
