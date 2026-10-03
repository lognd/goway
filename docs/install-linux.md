# Installing goway on Linux (and WSL)

    scripts/install.sh      # builds with cargo (release) and installs
    scripts/uninstall.sh    # removes exactly what install added

Install puts the binary at `~/.local/bin/goway` (`GOWAY_PREFIX`
overrides `~/.local`). If that directory is not on `PATH` yet, it
appends one marked line to `~/.profile`:

    export PATH="$HOME/.local/bin:$PATH" # added by goway install

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

Anything you changed after install is kept and reported. A second
install is refused while a journal exists.

`crates/goway/tests/install_scripts.rs` proves it. The test snapshots
every file, mode and byte under a scratch `$HOME`, installs, uninstalls,
and requires an identical snapshot. It covers four cases:
- an empty home
- an existing profile without a trailing newline and an existing
  `~/.local/bin`
- `~/.local/bin` already on `PATH`
- a binary replaced after install, plus a refused second install

Hosts need nothing installed. Run `goway doctor HOST --fix` for the
toolchain on a build host.
