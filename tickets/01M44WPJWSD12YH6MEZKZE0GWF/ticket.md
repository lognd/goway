+++
id = "01M44WPJWSD12YH6MEZKZE0GWF"
title = "On WSL helpers goway measures the virtual disk, not the Windows drive, and cleanup never returns space to Windows"
type = "bug"
category = "in-progress"
priority = "critical"
points = 8
reporter = "lognd"
created = "2026-10-05T02:01:20Z"
updated = "2026-10-05T03:15:47Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/footprint.rs", "crates/goway/src/doctor/windows.rs", "crates/goway-setup/src/host.rs", "crates/goway/src/doctor.rs", "crates/goway/src/status.rs", "crates/goway/src/pool.rs", "crates/goway/src/config.rs", "crates/goway/src/run.rs", "crates/goway/src/gc.rs", "crates/goway-journal/src/change.rs", "crates/goway-journal/tests/properties.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/render.rs", "crates/goway/tests/wsl_drive.rs", "crates/goway-setup/tests/host_plan.rs", "crates/goway-setup/tests/hostsys.rs", "docs/usage.md", "docs/config.md", "docs/troubleshooting.md", "docs/changes.md", "crates/goway/tests/journal_invariant.rs", "docs/install-windows.md", "crates/goway-setup/src/tune.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/src/ps.rs", "crates/goway-setup/tests/tune.rs", "crates/goway/src/needs.rs", "crates/goway/tests/common/mod.rs"]

[[acceptance]]
text = "Given a WSL helper, When goway probes its disk, Then free space is the smaller of the ext4 free space and the free space of the Windows drive holding the distro's vhdx (read through the drvfs mount, without interop), and goway status shows both"
bound = true

[[acceptance]]
text = "Given a WSL helper whose Windows drive has less than a reserve free (documented default, e.g. 15 GB or 5 percent), When a run is admitted, Then goway evicts idle caches first and refuses the run if the reserve still cannot be kept, naming the drive and its free space"
bound = true

[[acceptance]]
text = "Given goway-setup install or tune on a WSL helper, When it runs, Then the distro's vhdx is set sparse (wsl --manage DISTRO --set-sparse true, journaled, with WSL version checked) and freed space is trimmed back (fstrim after gc evictions, or the fstrim timer), so deleting caches inside WSL shrinks the file on Windows"
bound = true

[[acceptance]]
text = "Given a WSL helper whose vhdx is not sparse or whose Windows drive is under the reserve, When goway doctor checks it, Then it warns with the journaled fix and, for a non-sparse vhdx that has grown, the steps to compact it once"
bound = true

[[acceptance]]
text = "Given no max_disk configured for a host, When the disk budget is computed, Then it is relative to the real drive (a documented fraction of its size), not a fixed size"
bound = true
+++

Observed 2026-10-04: a helper's Windows C: drive reached 0 bytes free (WSL then failed to start: HCS_E_CONNECTION_TIMEOUT). Inside WSL, df reports the ext4.vhdx's virtual capacity (about 1 TB), so the disk budget (default max_disk 300G) saw room while the real drive filled. And the vhdx only grows: gc frees blocks inside Linux but the file on C: keeps its size, so cleanup never gives space back. Permanent fix: measure the real drive, make freed space flow back, and refuse to fill a drive.
