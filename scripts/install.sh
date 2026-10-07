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

base=$(basename "$PWD")
rest=${base#yi-}
if [ "$rest" != "$base" ] && [ "${rest#*-}" != "$rest" ]; then
  version=${rest%%-*}
  target=${rest#*-}
  json_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '%s' "$s"
  }
  printf '{"prefix":"%s","version":"%s","target":"%s"}\n' \
    "$(json_escape "$prefix")" "$(json_escape "$version")" "$(json_escape "$target")" \
    > "$root/install.json"
fi

echo "installed $prefix/yi"
case ":$PATH:" in
  *":$prefix:"*) ;;
  *) echo "warning: $prefix is not on PATH" ;;
esac
