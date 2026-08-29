#!/usr/bin/env bash
# cc shim for cross-compiling via zig; used as CC_<target>/CARGO_TARGET_*_LINKER.
set -euo pipefail
exec zig cc -target "${ZIG_TARGET:?ZIG_TARGET not set}" "$@"
