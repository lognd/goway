+++
id = "01M41T4HF1ATS7YSPG5GDJCK7F"
title = "doctor and goway-setup help a WSL helper get hardware: RAM, swap, cores, nested virtualization (journaled .wslconfig), GPU driver and CUDA guidance"
type = "story"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:18:48Z"
updated = "2026-10-04T02:24:54Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/facts.rs", "crates/goway-setup/src/tune.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/src/host.rs", "docs/install-windows.md", "crates/goway-setup/tests/tune.rs", "crates/goway-setup/src/lib.rs", "crates/goway/src/remote.sh", "crates/goway-setup/src/render.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/error.rs", "docs/troubleshooting.md"]

[[links]]
kind = "blocked-by"
target = "01M41T2EDAC22Z3A6BP3Q0VGAX"

[[acceptance]]
text = "Given a WSL helper whose RAM, swap or cores are far below the laptop's, when goway doctor runs, then it shows what WSL gets versus what the laptop has and the exact goway-setup command to change it, with a suggested size that leaves Windows at least 4 GiB or 25% of RAM"
bound = true

[[acceptance]]
text = "Given goway-setup tune --memory/--swap/--processors/--nested-virtualization on the helper, when it runs, then it edits .wslconfig through the journal (prior values recorded), refuses while goway jobs run unless --yes, explains that wsl --shutdown stops everything in WSL, asks before running it, and goway-setup uninstall --host restores the exact previous values"
bound = true

[[acceptance]]
text = "Given an NVIDIA GPU that Windows sees but WSL does not, when doctor runs, then it says so and prints the Windows driver step (never a Linux driver inside WSL); given the GPU is visible but CUDA is missing, it offers NVIDIA's WSL CUDA toolkit through the --rsudo flow, recorded for uninstall"
bound = true
+++
