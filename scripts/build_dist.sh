#!/usr/bin/env bash
# The one dist build, so the binary the size guardrail measures is byte-for-byte the
# binary that ships.
#
# `--remap-path-prefix` is the whole reason this is a script rather than four copies of
# a cargo line. Every dependency panic string carries the absolute path of the source
# file it came from, and on the measured build host that was 53,710 bytes of
# `~/.cargo/registry/src/index.crates.io-<hash>/` and `~/.rustup/toolchains/<name>/`
# prefixes -- plus the builder's username in any backtrace an operator pastes into an
# issue. Mapping each prefix to a single letter keeps `file:line` readable and makes the
# strings identical on every host, so two machines build the same bytes. Dev builds are
# left alone: their backtraces should stay clickable.
set -euo pipefail

remap=()
for dir in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/; do
  [ -d "$dir" ] && remap+=("--remap-path-prefix=${dir%/}=/c")
done
for dir in "${RUSTUP_HOME:-$HOME/.rustup}"/toolchains/*/; do
  [ -d "$dir" ] && remap+=("--remap-path-prefix=${dir%/}=/r")
done

exec env RUSTFLAGS="${RUSTFLAGS:-} ${remap[*]}" cargo build --profile dist -p yi-cli "$@"
