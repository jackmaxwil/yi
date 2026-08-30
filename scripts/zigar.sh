#!/usr/bin/env bash
# ar shim for cross-compiling via zig; used as AR_<target>.
set -euo pipefail
exec zig ar "$@"
