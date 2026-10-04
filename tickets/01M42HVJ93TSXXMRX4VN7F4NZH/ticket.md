+++
id = "01M42HVJ93TSXXMRX4VN7F4NZH"
title = "Use a placeholder host name in the doctor output golden test"
type = "chore"
category = "done"
outcome = "wont-fix"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T04:13:20Z"
updated = "2026-10-04T04:13:33Z"
scope = ["crates/goway/tests/doctor_output.rs"]

[[acceptance]]
text = "Given crates/goway/tests/doctor_output.rs, when read, then its host names are placeholders (helios, orion and the like), and the golden tests still pass"
bound = false
+++
