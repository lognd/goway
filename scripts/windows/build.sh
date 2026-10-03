#!/usr/bin/env bash
# Cross-build goway.exe and the single-file installer goway-setup.exe (goway.exe embedded)
# for 64-bit Windows from Linux/WSL. Needs mingw-w64 (x86_64-w64-mingw32-gcc) and the rustup
# target: rustup target add x86_64-pc-windows-gnu
# x86_64 runs natively on x64 Windows and under emulation on Windows on ARM.
# Output: target/x86_64-pc-windows-gnu/release/goway-setup.exe
set -euo pipefail

target=x86_64-pc-windows-gnu
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
tdir="${CARGO_TARGET_DIR:-$root/target}"

cargo build --release --target "$target" -p goway
GOWAY_PAYLOAD="$tdir/$target/release/goway.exe" \
    cargo build --release --target "$target" -p goway-setup

echo "built: $tdir/$target/release/goway-setup.exe"
