#!/usr/bin/env bash
# Install an unpacked release. The sources land in ~/.yi because python_root()
# falls back there, which is what lets the binary live anywhere on PATH.
set -euo pipefail
cd "$(dirname "$0")"

prefix="${YI_PREFIX:-$HOME/.local/bin}"
root="$HOME/.yi"

mkdir -p "$prefix" "$root"
install -m 755 bin/yi "$prefix/yi"
rm -rf "$root/python"
cp -R python "$root/python"
mkdir -p "$root/skills"
cp -R skills/. "$root/skills/"

echo "installed $prefix/yi"
case ":$PATH:" in
  *":$prefix:"*) ;;
  *) echo "warning: $prefix is not on PATH" ;;
esac
