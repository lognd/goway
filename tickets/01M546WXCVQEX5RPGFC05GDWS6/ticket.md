+++
id = "01M546WXCVQEX5RPGFC05GDWS6"
title = "goway-setup install --host --join <welcome>: join a circle from the helper installer"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:56:01Z"
updated = "2026-10-17T07:46:47Z"
labels = ["circles", "creates:changelog.d/01M546WXCVQEX5RPGFC05GDWS6.added.md"]
scope = ["crates/goway-setup/**", "docs/circles.md", "docs/install-windows.md", "changelog.d/01M546WXCVQEX5RPGFC05GDWS6.added.md"]

[[acceptance]]
text = "goway-setup install --host --join <welcome> --join-fingerprint <fp> validates the paste up front, then joins the circle and installs access lines inside the WSL distro"
bound = true
+++

deferred from ~NRHA10F (circles phase 2)
