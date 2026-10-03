# Releases

A release is built by `.github/workflows/release.yml` when a version tag
is pushed:

    git tag -a v0.1.0 -m "goway 0.1.0" && git push origin v0.1.0

## Dry run

Running the `release` workflow by hand (Actions, release, Run workflow,
or `gh workflow run release.yml`) is a dry run. It builds every binary,
installer, wheel and the sdist, writes `SHA256SUMS` and `install.sh`,
and uploads all of them as workflow artifacts (`release-assets` holds the
GitHub release files, `pypi-*` the wheels and sdist). The three
publishing jobs (`publish`, `crates-io`, `pypi`) each carry
`if: startsWith(github.ref, 'refs/tags/v') && github.event_name == 'push'`,
so they run only for a pushed `v*` tag and never from a dry run; a test
(`publishing.rs`) fails if one loses that guard. All actions in the
workflows run on Node 24.

A tag push publishes:

| File | For |
|---|---|
| `goway-x86_64-unknown-linux-musl.tar.gz` | Linux and WSL on Intel/AMD |
| `goway-aarch64-unknown-linux-musl.tar.gz` | Linux and WSL on ARM |
| `goway-setup.exe` | Windows x64 (also runs on Windows on ARM) |
| `goway-setup-arm64.exe` | Windows on ARM, native |
| `install.sh` | the Linux/WSL installer (`curl ... \| bash`) |
| `SHA256SUMS` | checksums of all of the above |

The Linux binaries are static (musl), so they run on any distribution.

## Publishing to crates.io

After the GitHub release succeeds, the `crates-io` job publishes
`goway-journal` and then `goway` (in dependency order, in one
`cargo publish --workspace --exclude goway-setup`). `goway-setup` is
never published: it carries `publish = false`. CI runs the same command
with `--dry-run` on every push, so packaging problems show up before a
tag does.

The job runs in the GitHub environment `crates-io`, whose secret
`CARGO_REGISTRY_TOKEN` is passed to cargo as an environment variable.
The token is needed only for the first publish of each crate. Once the
crates exist, configure crates.io trusted publishing for this repository
and workflow, switch the job to it, and delete the token and the secret.

To withdraw a bad version, yank it (existing lockfiles keep working, new
resolutions skip it), then publish a fixed patch version:

    cargo yank --version 0.1.1 goway
    cargo yank --version 0.1.1 goway-journal

## Publishing to PyPI

The same tag publishes `goway` to PyPI so that `uv tool install goway`
or `pipx install goway` works. `pyproject.toml` at the repository root
tells maturin to build the `goway` executable into a wheel
(`bindings = "bin"`: there is no Python module, only the program) and
to take the version from Cargo.

| Wheel | Built on |
|---|---|
| manylinux, x86_64 and aarch64 | glibc systems (most distributions) |
| musllinux, x86_64 and aarch64 | Alpine and other musl systems (static binary) |
| Windows x86_64 | Windows |
| source distribution (sdist) | everything else, built with cargo |

A musllinux wheel is not accepted by pip on a glibc system, which is why
both Linux families are built. There are no macOS wheels yet: goway's
macOS support is experimental (see the README), so macOS users get the
sdist and need a Rust toolchain. Add the macOS targets to the `wheels`
matrix in `release.yml` once macOS is supported.

The `pypi` job runs after the GitHub release succeeds, in the GitHub
environment `pypi`, with `permissions: id-token: write`. It uses PyPI
trusted publishing (OIDC), so no token is stored anywhere. The trusted
publisher on PyPI is configured for owner `lognd`, repository `goway`,
workflow `release.yml`, environment `pypi`. CI builds a wheel on every
push, installs it into a clean virtual environment and runs
`goway --version`.

To withdraw a bad PyPI release, yank it on the project's page on PyPI
(Manage, Releases, Options, Yank); pip then skips it unless pinned.

## Verifying a download

`install.sh` checks the archive against `SHA256SUMS` before installing,
and refuses on a mismatch. To check a file yourself:

    sha256sum -c --ignore-missing SHA256SUMS

Each file also carries a GitHub build provenance attestation. It proves
the file was built by this repository's release workflow from the
tagged commit:

    gh attestation verify goway-setup.exe --repo lognd/goway

A checksum fetched from the same release protects against a damaged
download, not against a compromised release. The attestation covers
that case.
