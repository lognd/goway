+++
id = "01M42ERHA91FDR28QRFG7RAZP9"
title = "doctor warns about shell startup files that print text, naming the line to move"
type = "story"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T03:19:15Z"
updated = "2026-10-04T03:19:15Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/resolve.rs", "crates/goway/tests/shell_noise.rs"]

[[acceptance]]
text = "Given a helper whose startup files print text for non-interactive ssh, when goway doctor runs, then it warns with the first noise line and the guard to add (case $- in *i*) ;; *) return ;; esac) in the helper's ~/.bashrc"
bound = false
+++

split from ~QGP7M3S by lane C. Producer side exists: sync::capture returns Captured.noise and remote::split_frame returns the noise; SshProber.probe (resolve.rs) currently discards it. Carry it to doctor (for example a startup_noise fact line) and render the warning.
