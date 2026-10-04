+++
id = "01M42K43FTZJG00K25DCDZ3HFD"
title = "doctor quiet-output test must not assume bash passes on macOS"
type = "bug"
category = "in-progress"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T04:35:28Z"
updated = "2026-10-04T04:41:45Z"
scope = ["crates/goway/tests/doctor_project.rs"]

[[acceptance]]
text = "Given the macOS CI helper where some checks fail, when doctor runs quiet and with --all, then every row --all shows as ok is absent from the quiet output, whatever the host passes or fails"
bound = true
+++

macOS CI run 37176708485 failed doctor_is_quiet_by_default_and_explains_one_check_on_request: bash is a problem there, so it is shown in the quiet output.
