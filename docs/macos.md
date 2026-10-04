# A Mac as a helper

Supported, untested by the maintainer (no such machine): the remote script
runs in CI on a macOS runner only.

goway's remote script uses GNU tools. On a Mac it puts Homebrew's copies
first on its own PATH (`/opt/homebrew` and `/usr/local`), so nothing about
your shell setup matters. Install them once, without sudo:

    brew install coreutils findutils gnu-sed gnu-tar grep util-linux flock

`goway doctor HOST --fix` does exactly that (and `xcode-select --install`
for the compiler is the one step it can only explain). Without Homebrew,
doctor says so and points at https://brew.sh.

What differs from Linux:

- load comes from `sysctl vm.loadavg`, cores from `hw.ncpu`, RAM from
  `hw.memsize` and `vm_stat`; the arch is reported as `aarch64` for Apple
  Silicon, like Linux; `--needs os=darwin` selects Macs.
- the script runs under the system bash 3.2.
- lock files are checked for being replaced with perl's `fstat` (macOS has
  no `/proc/self/fd`).
- GPU, CPU-flag and KVM facts are not probed.

Known gaps (the macOS CI job skips these tests): framework shard detection
(GoogleTest and Catch2 binaries), nested goway runs (`GOWAY_DEPTH`) and the
copy-verification rerun path, and the slot cleanup of leftover directories are not yet confirmed on macOS; the five
`sync.rs` protocol tests are still ignored there. Tests that assume Linux
facts (`os=linux`, `nice`) are skipped. Report anything else you hit.
