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

# Incident: a forge run that restored no cache had no registry sources to remap yet, and the same
# tree measured 24,576 bytes more; fetching first unpacks every source the lockfile names.
cargo fetch -q
remap_text=$(python3 scripts/guardrails/_common.py)
remap=()
while IFS= read -r flag; do
  [ -n "$flag" ] && remap+=("$flag")
done <<< "$remap_text"


build="${CARGO_TARGET_DIR:-target}"
previous=""
for argument in "$@"; do
  [ "$previous" = "--target" ] && build="$build/$argument"
  previous="$argument"
done

env RUSTFLAGS="${RUSTFLAGS:-} ${remap[*]}" cargo build --profile dist -p yi-cli "$@"

bin="$build/dist/yi"
if [ "$(uname -s)" = Linux ] && command -v objcopy >/dev/null && [ -x "$bin" ]; then
  objcopy --remove-section=.eh_frame --remove-section=.eh_frame_hdr "$bin"
fi
