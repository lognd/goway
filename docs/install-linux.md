# Installing goway on Linux (and WSL)

    curl -fsSL https://github.com/lognd/goway/releases/latest/download/install.sh | bash
                            # downloads the release binary, checks it against
                            # SHA256SUMS, installs (docs/release.md)
    scripts/install.sh      # from a source checkout: builds with cargo and installs
    scripts/uninstall.sh    # removes exactly what install added
    goway uninstall         # removes goway everywhere: helpers, config, then the program

Other ways to install (uv, pipx, pip in a virtual environment, cargo) are in
[install-methods.md](install-methods.md); `goway uninstall` removes the program
the way it was installed.

Install puts the binary at `~/.local/bin/goway` (`GOWAY_PREFIX`
overrides `~/.local`). If that directory is not on `PATH` yet, it
appends one marked line to `~/.profile`:

    export PATH="$PATH:$HOME/.local/bin" # added by goway install

No root is needed, and nothing outside your home directory is touched.
`GOWAY_INSTALL_BINARY=path` installs a prebuilt binary instead of
building one.

## Reversible by construction

Every change is recorded with its prior state in
`${XDG_STATE_HOME:-~/.local/state}/goway/install-journal`:

- the directories install created,
- the binary with its sha256,
- the PATH line, and whether `~/.profile` existed before,
- the trailing newline added to a profile that lacked one.

`uninstall.sh` replays the journal backwards:
- It removes the binary only if it is still the installed build.
- It removes the exact PATH line, and deletes `~/.profile` only if
  install created it and it is now empty.
- It drops the added newline.
- It removes created directories only if they are empty.

Anything you changed after install is kept and reported.

## Upgrading, and running the installer again

Run the same command again to upgrade in place; do not uninstall first.
The installer downloads and verifies the new release, then looks at the
journal:

- **Binary matches the journal**: it replaces `~/.local/bin/goway` with
  the new build (moved into place, so a running goway is not disturbed)
  and records the new sha256 in the journal. The PATH line and the
  directories are left alone, and `scripts/uninstall.sh` stays exact.
- **Same build already installed**: it says so and changes nothing.
- **Binary changed by hand** (its hash differs from the journal): it
  refuses and names the file. Move your file aside and re-run, or run
  `goway uninstall` first.
- **Journal from another `GOWAY_PREFIX`, or damaged**: it refuses and
  says which journal; nothing is changed.
- **Journal left by a cut-off install** (no binary, no PATH line): it
  clears that journal and the empty directories it made, then installs
  normally.

## Use bash, not sh

The documented command ends in `| bash`. `| sh` fails on Debian and
Ubuntu, where `sh` is dash. The installer notices before it does
anything and prints `this installer needs bash, not sh`, with the right
command. Nothing was changed; re-run with `| bash`.

`crates/goway/tests/install_scripts.rs` proves it. The test snapshots
every file, mode and byte under a scratch `$HOME`, installs, uninstalls,
and requires an identical snapshot. It covers these cases:
- an empty home
- an existing profile without a trailing newline and an existing
  `~/.local/bin`
- `~/.local/bin` already on `PATH`
- a binary replaced after install, plus a refused upgrade over it
- re-running the same build (no change) and a newer build (upgrade in
  place, journal hash updated, uninstall still exact)
- the installer piped into `sh` (stops with the `| bash` hint)

Hosts need nothing installed. Run `goway doctor HOST --fix` for the
toolchain on a build host.
