#!/usr/bin/env bash
# Incident: a suite run left ~96,000 entries (20 GiB) in the user TMPDIR by 2026-11-03: tests that
# build dirs under temp_dir() by hand, and runs nextest killed before any Drop ran.
# Every test process gets its own TMPDIR and this wrapper removes it, however the test ends.
set -u
tmp=$(mktemp -d /tmp/yt.XXXXXX) || exit 1
trap 'rm -rf "$tmp"' EXIT
trap 'exit 143' TERM INT HUP
TMPDIR=$tmp "$@"
