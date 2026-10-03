# Contributing to goway

Thanks for considering a contribution, from a typo fix to a new feature.

## TL;DR for experienced contributors

- Fork the repo, clone your fork, add `lognd/goway` as `upstream`.
- Rust is pinned in `rust-toolchain.toml`; rustup installs it on first
  use.
- The gate, all green before you open a pull request:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo nextest run --workspace --locked` (install with
    `cargo install cargo-nextest --locked`)
  - Windows code also has to pass
    `cargo clippy --target x86_64-pc-windows-gnu --workspace --all-targets --locked -- -D warnings`.
    CI checks Linux and Windows.
- The test suite needs Linux (or WSL): bash, GNU coreutils, findutils,
  tar, util-linux (`flock`, `setsid`), git and ssh-keygen. It never
  touches a real remote machine; a fake `ssh` runs goway's remote side
  locally.
- Commit format: `<type>(<scope>): <imperative summary, 72 chars max>`.
  Types: `feat`, `fix`, `chore`, `refactor`, `test`, `docs`, `perf`,
  `ci`, `build`. No trailing period. One logical change per commit.
- ASCII only in every file, no exceptions.
- Every public item gets a one-line doc comment saying why or what.
- goway's own output goes through `crates/goway/src/render.rs`; print
  macros are denied everywhere else.
- Fallible operations return typed errors; panics are for programmer
  bugs only.
- Anything the installers change must go through the journal
  (`crates/goway-journal`), so uninstall can undo it exactly. Add a
  property or snapshot test that proves the undo.

## Your first contribution (step by step)

If you have never opened a pull request against someone else's project,
this section is for you.

1. **Fork.** Open [github.com/lognd/goway](https://github.com/lognd/goway)
   and click "Fork" in the top right.
2. **Clone your fork:**

   ```bash
   git clone https://github.com/<your-username>/goway.git
   cd goway
   git remote add upstream https://github.com/lognd/goway.git
   ```

3. **Install Rust** from [rustup.rs](https://rustup.rs) and the test
   runner: `cargo install cargo-nextest --locked`.
4. **Run the tests once, to see green:** `cargo nextest run --workspace`.
5. **Make a branch**, change things, run the gate above, and commit.
6. **Push your branch** to your fork and open a pull request against
   `lognd/goway` `main`. Describe what changed and why, and how you
   tested it.

## Security issues

Please do not open public issues for vulnerabilities; see
[SECURITY.md](SECURITY.md).

## AI-assisted contributions

AI-assisted contributions are welcome under the same bar as any other:
you must understand and be able to explain every line you submit. You
are also responsible for running the gate and the tests yourself.

## Code of Conduct

Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
