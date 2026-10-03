+++
id = "01M41DQSMXMRTN3BNYBHMA8K5M"
title = "Native aarch64 Windows build of goway.exe and goway-setup.exe"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:42:08Z"
updated = "2026-10-03T21:32:27Z"
scope = ["scripts/windows/**"]

[[acceptance]]
text = "Given scripts/windows/build.sh, when run, then it produces aarch64 and x86_64 installers and the aarch64 one runs natively on Windows on ARM"
bound = false
+++
