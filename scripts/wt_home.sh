#!/usr/bin/env bash
# Incident: 116 test homes under ~/Development/.yi-homes were never deleted (2026-10-07). HOME at
# <worktree>/target/yi-home dies with the worktree; toolchains and the uv cache stay shared.
set -euo pipefail
home="$(git rev-parse --show-toplevel)/target/yi-home"
if [ "$HOME" = "$home" ]; then exec "$@"; fi
real="$HOME"
cargo_home="${CARGO_HOME:-$real/.cargo}"
mkdir -p "$home"
if [ -f "$real/.gitconfig" ]; then cp "$real/.gitconfig" "$home/.gitconfig"; fi
# RUSTUP_AUTO_INSTALL=0: a shared RUSTUP_HOME is safe to read but two concurrent installs of one
# toolchain can corrupt it (rust-lang/rustup#988), so a missing toolchain fails instead.
cargo_bin="$cargo_home/bin"
case ":${PATH:-}:" in
  *":$cargo_bin:"*) path="${PATH:-}" ;;
  *) path="${PATH:+$PATH:}$cargo_bin" ;;
esac
exec env HOME="$home" \
  CARGO_HOME="$cargo_home" \
  RUSTUP_HOME="${RUSTUP_HOME:-$real/.rustup}" \
  UV_CACHE_DIR="${UV_CACHE_DIR:-$real/.cache/uv}" \
  RUSTUP_AUTO_INSTALL=0 \
  PATH="$path" \
  "$@"
