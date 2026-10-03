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
