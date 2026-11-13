#!/usr/bin/env bash
# Incident: suite runs left 11,633 checkpoint repos in the real ~/.yi by 2026-10-07. Recipes source
# this file so its exec replaces the recipe; run as a child, it ran the whole suite twice.
case $- in *e*) caller_had_e=1 ;; *) caller_had_e= ;; esac
set -euo pipefail
recipe=$1
just --show "$recipe" >/dev/null 2>&1 || {
    echo "suite_home.sh: '$recipe' is not a just recipe. Recipes source this file; to run one command with the lane's HOME use scripts/wt_home.sh <command>." >&2
    [ -n "$caller_had_e" ] || set +euo pipefail
    return 1 2>/dev/null || exit 1
}
[ "$HOME" = "$(git rev-parse --show-toplevel)/target/yi-home" ] || exec scripts/wt_home.sh just "$recipe"
