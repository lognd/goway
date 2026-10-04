+++
id = "01M43RNR0H7WHXDXKTKVDZE455"
title = "CI: per-test timeouts in nextest and per-job timeout-minutes, plus a stuck-process dump on macOS"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T15:31:44Z"
updated = "2026-10-04T15:42:31Z"
scope = [".config/nextest.toml", ".github/workflows/ci.yml", "scripts/ci-ps-watch.sh", "docs/macos.md"]

[[acceptance]]
text = "Given a test that hangs, When nextest runs it, Then it is terminated after its slow-timeout instead of running for hours"
bound = true

[[acceptance]]
text = "Given any CI job, When it runs, Then it has a timeout-minutes limit"
bound = true

[[acceptance]]
text = "Given a macOS test run that is slow, When a test process outlives 45s, Then the process tree and its open pipes are printed to the log"
bound = true
+++

Hung macOS runs ran for hours and starved the runner pool. Hard safety net first, then the diagnosis of ~E09XD8K.
