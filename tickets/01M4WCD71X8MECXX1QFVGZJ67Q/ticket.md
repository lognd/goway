+++
id = "01M4WCD71X8MECXX1QFVGZJ67Q"
title = "A failed or undone host install leaves openssh-server in rc state, which the next install refuses"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
reporter = "lognd"
created = "2026-10-14T04:58:22Z"
updated = "2026-10-17T10:42:21Z"
scope = ["crates/goway-setup/src/hostsys.rs"]

[[acceptance]]
text = "After install --host is rolled back or uninstalled, rerunning install --host succeeds without manual apt commands"
bound = true

[[acceptance]]
text = "The reinstalled sshd presents the same host key as before the uninstall"
bound = true

[[acceptance]]
text = "Test a_package_left_removed_with_its_configuration_is_installed_again_without_purging passes: rc state reinstalls without purge"
bound = true
+++

goway's own uninstall and rollback remove openssh-server with `apt-get remove` (deliberately never purge, so sshd_config and the host keys survive and pinned fingerprints stay valid). That leaves dpkg state "deinstall ok config-files" (rc). The next install --host then refuses the package step because dpkg_state treats rc as an odd state ("goway will not touch it"). So any failed install, and any uninstall, blocks the next install until the user runs `apt-get purge openssh-server` by hand, which also throws away the host keys the never-purge rule meant to keep. Hit three times on Nova on 2026-10-14 while retrying a failing install. Decide: accept rc as absent when installing (reinstall reuses the kept config and keys), or record in the journal that goway itself left the rc state and accept only that.
