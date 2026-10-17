+++
id = "01M54D7R6PPDKHA7CGZM7H9TBV"
title = "goway-setup --join: native (non-WSL) helpers, node key creation and card output"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T07:46:47Z"
updated = "2026-10-17T08:12:52Z"
scope = ["crates/goway-setup/**", "docs/**"]

[[acceptance]]
text = "goway-setup install --host --join works for native helpers and creates the node key and prints the circle card before joining"
bound = true
+++

leftover from ~05GDWS6: --join supports WSL helpers with an existing node key only; native helpers and creating the card in the installer are not done
