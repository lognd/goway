# Other ways to install goway: uv, pipx, pip and cargo

The quickest install is the one in the README (`install.sh` on Linux
and WSL, `goway-setup.exe` on Windows). goway is also published as a
Python package on PyPI and as a Rust crate on crates.io, so the tools
you may already use can install it. Every command on this page is
**available from v0.1.0**, the first release that is published to PyPI
and crates.io.

Whichever way you install, the next step is the same: add a helper
laptop with `goway add` (see [usage](usage.md) and the README's quick
start). Nothing else needs setting up on this laptop first.

| Method | Program lands in | Put it on PATH | Update | Remove |
|---|---|---|---|---|
| uv | `~/.local/bin/goway` (a link into uv's tool environment) | `uv tool update-shell` | `uv tool upgrade goway` | `uv tool uninstall goway` |
| pipx | `~/.local/bin/goway` (a link into pipx's environment) | `pipx ensurepath` | `pipx upgrade goway` | `pipx uninstall goway` |
| pip, in a virtual environment | `<venv>/bin/goway` | activate the environment | `pip install --upgrade goway` | `pip uninstall goway` |
| cargo | `~/.cargo/bin/goway` | `~/.cargo/bin` on PATH (rustup does it) | rerun the install | `cargo uninstall goway` |

Open a new terminal after changing PATH.

## uv (preferred)

[uv](https://docs.astral.sh/uv/getting-started/installation/) installs
Python programs each in their own environment, so nothing clashes with
other software. Install uv first by following its page, then:

```bash
uv tool install goway    # available from v0.1.0
uv tool update-shell     # once: puts ~/.local/bin on PATH in your shell profile
uv tool upgrade goway    # later, to update
uv tool uninstall goway  # to remove the program
```

The program is `~/.local/bin/goway`; the environment behind it is under
`~/.local/share/uv/tools/goway` (`UV_TOOL_DIR` moves it). The PyPI file
contains the same compiled program as the release downloads, not Python
code. Next step: `goway add`.

## pipx

[pipx](https://pipx.pypa.io/) does the same job as `uv tool`:

```bash
pipx install goway       # available from v0.1.0
pipx ensurepath          # once: puts ~/.local/bin on PATH
pipx upgrade goway       # later, to update
pipx uninstall goway     # to remove the program
```

The program is `~/.local/bin/goway`; the environment is under
`~/.local/share/pipx/venvs/goway` (older pipx: `~/.local/pipx/venvs`;
`PIPX_HOME` moves it). Next step: `goway add`.

## pip, only inside a virtual environment

```bash
python3 -m venv ~/goway-venv                 # once
~/goway-venv/bin/pip install goway           # available from v0.1.0
~/goway-venv/bin/goway --version
```

The program is `~/goway-venv/bin/goway`; run it by that path or
activate the environment first. Update with `pip install --upgrade
goway`, remove with `pip uninstall goway` (or delete the directory).

Do not use `pip install goway` outside a virtual environment. Debian,
Ubuntu and several other distributions mark the system Python as
"externally managed" ([PEP 668](https://peps.python.org/pep-0668/)) and
make system-wide `pip install` fail with `externally-managed-environment`
on purpose: pip and the system package manager would overwrite each
other's files and can break the tools your system relies on. Do not
bypass it with `--break-system-packages`. Use uv or pipx instead; they
create the environment for you.

## cargo

[rustup](https://rustup.rs) installs Rust and cargo. Then:

```bash
cargo install --locked goway   # available from v0.1.0
cargo uninstall goway          # to remove the program
```

The program is `~/.cargo/bin/goway` (`CARGO_HOME` moves it). rustup
adds `~/.cargo/bin` to your PATH; if `goway` is not found, add it
yourself and open a new terminal. There is no update command: rerun
`cargo install --locked goway` and cargo replaces the old build. The
first install compiles goway, which takes a few minutes. Next step:
`goway add`.

## `goway uninstall` knows how it was installed

`goway uninstall` first removes goway's settings, keys and helper-side
files (see the README's Uninstall section). It then looks at where the
running program lives and removes the program the way it was installed:

| goway lives in | Method | `goway uninstall` runs, last |
|---|---|---|
| a journal from `install.sh` exists | install script | replays the journal (unchanged) |
| `$CARGO_HOME/bin` or `~/.cargo/bin` | cargo | `cargo uninstall goway` |
| `$UV_TOOL_DIR`, else `~/.local/share/uv/tools` | uv | `uv tool uninstall goway` |
| `$PIPX_HOME/venvs`, else `~/.local/share/pipx/venvs` or `~/.local/pipx/venvs` | pipx | `pipx uninstall goway` |
| any other virtual environment (a `pyvenv.cfg` one or two levels above the program) | pip | `<venv>/bin/python -m pip uninstall --yes goway` |
| anywhere else | unknown | nothing; goway says so and names the file |

The plan that `goway uninstall` prints (and `--dry-run` shows alone)
names the exact command with the absolute path of the tool found on
PATH, for example `/home/you/.local/bin/uv tool uninstall goway`. goway
runs it only after you confirm (or pass `--yes` or `--everywhere`), and
after everything else is removed. If the tool is not on PATH, the plan
says so and prints the command for you to run. On Windows a running
program cannot delete itself, so goway prints the command instead of
running it. If the command fails, goway says so and exits with status 1;
run the printed command yourself.

CI proves the uv case end to end: it installs the built wheel with
`uv tool install`, runs `goway uninstall --yes` with `HOME` and the XDG
directories pointed at a temporary directory, and checks that the tool
environment, the program link and goway's config are gone
(`.github/workflows/ci.yml`, job `wheel`).
