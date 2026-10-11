#!/usr/bin/env bash
# Incident: 116 test homes under ~/Development/.yi-homes were never deleted (2026-10-07). HOME at
# <worktree>/target/yi-home dies with the worktree; toolchains and the uv cache stay shared.
set -euo pipefail
home="$(git rev-parse --show-toplevel)/target/yi-home"
if [ "$HOME" = "$home" ]; then exec "$@"; fi
real="$HOME"
mkdir -p "$home"
# RUSTUP_AUTO_INSTALL=0: a shared RUSTUP_HOME is safe to read but two concurrent installs of one
# toolchain can corrupt it (rust-lang/rustup#988), so a missing toolchain fails instead.
exec env HOME="$home" \
  CARGO_HOME="${CARGO_HOME:-$real/.cargo}" \
  RUSTUP_HOME="${RUSTUP_HOME:-$real/.rustup}" \
  UV_CACHE_DIR="${UV_CACHE_DIR:-$real/.cache/uv}" \
  RUSTUP_AUTO_INSTALL=0 \
  "$@"
