#!/usr/bin/env bash
# Every signed release archive this host can build: the Mac one always, cross-linked through zig
# on Linux, and the static x86_64 Linux one on a Linux host. An archive this host can run is
# smoke-tested.
#
#   scripts/release_build.sh <version>
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:?usage: release_build.sh <version>}"
mac=aarch64-apple-darwin
linux=x86_64-unknown-linux-musl
if [ "$(uname -s)" = Linux ]; then
  : "${SDKROOT:?set SDKROOT to a macOS SDK; zig links the Mac binary against it}"
  CC_x86_64_unknown_linux_musl=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    scripts/build_dist.sh --target "$linux"
  scripts/package.sh "$version" "$linux"
  scripts/smoke.sh "target/package/yi-$version-$linux.tar.gz"
fi
scripts/build_dist.sh --target "$mac"
scripts/package.sh "$version" "$mac"
if [ "$(uname -s)" = Darwin ]; then scripts/smoke.sh "target/package/yi-$version-$mac.tar.gz"; fi
