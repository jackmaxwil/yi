#!/usr/bin/env bash
# cc shim for cross-compiling via zig; used as CC_<target>/CARGO_TARGET_*_LINKER.
# Incident: cc-rs appends `--target=x86_64-unknown-linux-musl`, a triple zig
# 0.16 refuses ("UnknownOperatingSystem"); the target is ZIG_TARGET's alone.
set -euo pipefail
args=()
for arg in "$@"; do
  case "$arg" in --target=*) ;; *) args+=("$arg") ;; esac
done
exec zig cc -target "${ZIG_TARGET:?ZIG_TARGET not set}" "${args[@]}"
