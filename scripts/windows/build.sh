#!/usr/bin/env bash
# Build goway.exe and the single-file installer goway-setup.exe (goway.exe embedded)
# for 64-bit Windows, x64 and/or ARM64.
#
#   scripts/windows/build.sh [--arch x64|arm64|all]     (default: x64)
#
# Output, in dist/: goway-setup.exe (x64) and goway-setup-arm64.exe (ARM64); the
# matching goway.exe stays in target/<triple>/release/. The ARM64 installer is a
# native aarch64 binary (it runs on Windows on ARM without emulation); x64 also
# runs on Windows on ARM under emulation.
#
# What each build needs:
#   on Windows (Git Bash, a CI runner): the MSVC toolchain; targets
#       x86_64-pc-windows-msvc and aarch64-pc-windows-msvc, built with plain cargo
#       (an ARM64 Windows machine builds both natively; an x64 one cross-compiles
#       arm64 with the "ARM64 build tools" component of Visual Studio).
#   on Linux/WSL: x64 uses mingw-w64 (x86_64-w64-mingw32-gcc) and the target
#       x86_64-pc-windows-gnu; arm64 uses cargo-xwin (cargo install cargo-xwin) and
#       the target aarch64-pc-windows-msvc.
set -euo pipefail

arch=x64
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) arch=${2:?--arch needs x64, arm64 or all}; shift 2 ;;
    -h | --help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "build.sh: unknown argument '$1' (try --help)" >&2; exit 2 ;;
  esac
done
case "$arch" in x64 | arm64 | all) ;; *) echo "build.sh: --arch must be x64, arm64 or all" >&2; exit 2 ;; esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
tdir="${CARGO_TARGET_DIR:-$root/target}"
mkdir -p dist

case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) on_windows=1 ;; *) on_windows=0 ;; esac

# build_one ARCH TRIPLE ASSET: build goway.exe, then the installer with it embedded.
build_one() {
  local a=$1 target=$2 asset=$3 verb=build
  # cargo-xwin cross-compiles the MSVC targets from Linux; everywhere else plain cargo.
  if [ "$on_windows" = 0 ] && [ "$a" = arm64 ]; then
    command -v cargo-xwin >/dev/null 2>&1 || { echo "build.sh: arm64 on Linux needs cargo-xwin (cargo install cargo-xwin)" >&2; exit 1; }
    verb="xwin build"
  fi
  rustup target add "$target" >/dev/null
  # shellcheck disable=SC2086
  cargo $verb --release --locked --target "$target" -p goway
  GOWAY_PAYLOAD="$tdir/$target/release/goway.exe" \
    cargo $verb --release --locked --target "$target" -p goway-setup
  cp "$tdir/$target/release/goway-setup.exe" "dist/$asset"
  echo "built: dist/$asset (goway.exe in $tdir/$target/release/)"
}

if [ "$arch" = x64 ] || [ "$arch" = all ]; then
  if [ "$on_windows" = 1 ]; then build_one x64 x86_64-pc-windows-msvc goway-setup.exe
  else build_one x64 x86_64-pc-windows-gnu goway-setup.exe; fi
fi
if [ "$arch" = arm64 ] || [ "$arch" = all ]; then
  build_one arm64 aarch64-pc-windows-msvc goway-setup-arm64.exe
fi
