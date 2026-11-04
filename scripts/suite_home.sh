#!/usr/bin/env bash
# Incident: suite runs left 11,633 checkpoint repos in the real ~/.yi by 2026-10-07. Recipes source
# this file so its exec replaces the recipe; run as a child, it ran the whole suite twice.
set -euo pipefail
recipe=$1
[ "$HOME" = "$(git rev-parse --show-toplevel)/target/yi-home" ] || exec scripts/wt_home.sh just "$recipe"
