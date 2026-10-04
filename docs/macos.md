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

The macOS CI job runs the whole suite on a Mac, repeated several times for
the tests that start many helper calls at once, with no retries. Only the
PowerShell contract tests (`remote_ps1`) are left out: a Mac has no
PowerShell. Tests that assume Linux facts derive them from the host (`os`,
the config directory), so they run here too.

Things a Mac helper needed that Linux did not:

- goway runs helper calls from several threads at once. Rust creates a
  pipe and marks it close-on-exec in two steps on macOS, so a child started
  by another thread in between inherited the pipe, and a call's answer then
  waited for an unrelated job to exit (the copy-verification verdict and
  nested runs stalled for minutes). All such spawns now go through one lock
  (`spawn.rs`). That has to be every spawn, not just the ones that capture
  output: an unlocked fork (the run's lifeline call was one) can carry
  another thread's half-made pipe into a child that lives as long as the
  run, and the reader of that pipe then waits for the run, which may be
  waiting for the reader. A unit test fails if production code spawns
  without the lock.
- bash 3.2 mishandles `IFS=$'\001'` in `read`, which broke the detection of
  dependency directories to keep in a slot; the script splits on `\037`.
- test binaries are Mach-O, not ELF: framework detection accepts both.
- the core count comes from `sysctl -n hw.ncpu`.
- goway's config directory is `~/Library/Application Support/goway` here
  (not `~/.config/goway`) unless `GOWAY_CONFIG_DIR` is set.
- Apple's `make` (3.81) compares whole seconds, so two builds inside one
  second can look equal; use `gmake` or Ninja for fast edit-build loops.

Report anything else you hit.

## Hangs and timeouts in CI

A hung test must not hold a runner for hours. `.config/nextest.toml` marks
every test SLOW after 60 seconds and kills it after five of those periods
(ten for the shard, CMake and footprint suites, which do real work). Every
CI job also has a `timeout-minutes` limit. On macOS the test steps run
`scripts/ci-ps-watch.sh` beside nextest: once a process has run for 45
seconds it prints the process table and the open pipes of the goway, bash
and nextest processes, so a stuck run leaves its evidence in the log.
