#!/usr/bin/env bash
# Incident: one suite run left 23 checkpoint repos in the real ~/.yi, 11,633 of them by
# 2026-10-07. The suite's HOME is the worktree's target/yi-home, deleted with the worktree.
set -euo pipefail
recipe=$1
[ "$HOME" = "$(git rev-parse --show-toplevel)/target/yi-home" ] || exec scripts/wt_home.sh just "$recipe"
