#!/usr/bin/env bash
# Every signed release archive this host can build, through `just package` where the host can run
# it: the Mac one on a Mac; on Linux the static x86_64 Linux one (smoked and ELF-checked) and the
# Mac one, cross-linked through zig and packaged unrun, since nothing on Linux can run it.
#
#   scripts/release_build.sh <version>
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:?usage: release_build.sh <version>}"
mac=aarch64-apple-darwin
linux=x86_64-unknown-linux-musl
if [ "$(uname -s)" = Darwin ]; then
  just package "$version" "$mac"
  exit 0
fi
: "${SDKROOT:?set SDKROOT to a macOS SDK; zig links the Mac binary against it}"
CC_x86_64_unknown_linux_musl=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  just package "$version" "$linux"
python3 scripts/check_elf.py "target/$linux/dist/yi" "$linux"
scripts/build_dist.sh --target "$mac"
scripts/package.sh "$version" "$mac"
