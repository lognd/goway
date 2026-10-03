# Releases

A release is built by `.github/workflows/release.yml` when a version tag
is pushed:

    git tag -a v0.1.0 -m "goway 0.1.0" && git push origin v0.1.0

It publishes:

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
