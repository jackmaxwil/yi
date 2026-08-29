#!/usr/bin/env bash
# Run the artifact the way a user gets it. The build tree is not the shipped
# tree: `python_root()` used to bake the build machine's absolute path into the
# binary, which every in-repo test passed straight through because the repo was
# still there. Unpacking under a directory with no checkout above it, against a
# throwaway HOME, is what makes that class of bug fail here rather than on
# someone's laptop.
#
#   scripts/smoke.sh <tarball>
set -euo pipefail
tarball="$(cd "$(dirname "${1:?usage: smoke.sh <tarball>}")" && pwd)/$(basename "$1")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export HOME="$work/home"
mkdir -p "$HOME"

tar -C "$work" -xzf "$tarball"
tree="$(find "$work" -maxdepth 1 -mindepth 1 -type d -not -name home)"

# Unpacked in place: the sources are found by walking up from bin/yi.
"$tree/bin/yi" --version < /dev/null
"$tree/bin/yi" ask --json --model faux/faux-1 ping < /dev/null > "$work/unpacked.json"
grep -q '"text":"faux: ping"' "$work/unpacked.json" || {
  echo "unpacked binary produced no answer"; cat "$work/unpacked.json"; exit 1;
}

# Installed: the tree is gone, so only the ~/.yi fallback can carry the sources.
( cd "$tree" && YI_PREFIX="$HOME/bin" ./install.sh )
rm -rf "$tree"
[ -d "$HOME/.yi/python/yi_runtime" ] || { echo "install.sh did not place the runtime"; exit 1; }
[ -d "$HOME/.yi/skills/yi" ] || { echo "install.sh did not place the skills"; exit 1; }

# An empty directory, not the repo and not `/`: nothing above it is a checkout,
# which is the property under test. (`/` would work too, but `yi` walks its cwd
# and takes minutes to do it there.)
mkdir -p "$work/elsewhere"
cd "$work/elsewhere"
"$HOME/bin/yi" --version < /dev/null
"$HOME/bin/yi" ask --json --model faux/faux-1 ping < /dev/null > "$work/installed.json"
grep -q '"text":"faux: ping"' "$work/installed.json" || {
  echo "installed binary produced no answer"; cat "$work/installed.json"; exit 1;
}
echo "smoke: ok"
