#!/usr/bin/env bash
set -euo pipefail
version=$(cat "$(dirname "$0")/gitleaks.version")
dest=/usr/local/bin
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) asset=linux_x64
    hash=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
    binary=88f91962aa2f93ac6ab281d553b9e125f5197bbbce38f9f2437f7299c32e5509 ;;
  Darwin-arm64) asset=darwin_arm64
    hash=b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5
    binary=ba52fb1bfabbcde42f032afad3d6e0b19dff8ed105229a16e7caa338bbc0e84f ;;
  *) echo "gitleaks: unsupported platform $(uname -s)-$(uname -m)"; exit 1 ;;
esac
eval "$(sed -n '/^sha256() {$/,/^}$/p' "$(dirname "$0")/../install.sh")"
declare -F sha256 >/dev/null || { echo "gitleaks: scripts/install.sh defines no sha256()"; exit 1; }
# The installed file is trusted by content alone, and the last line printed is the path check_scan.py runs.
if [ -x "$dest/gitleaks" ] && [ "$(sha256 "$dest/gitleaks")" = "$binary" ]; then
  echo "gitleaks: $version already installed at $dest/gitleaks"
  printf '%s\n' "$dest/gitleaks"
  exit 0
fi
work=$(mktemp -d)
staged="$dest/.gitleaks.$$"
trap 'rm -rf "$work" "$staged"' EXIT
url="https://github.com/gitleaks/gitleaks/releases/download/v${version}/gitleaks_${version}_${asset}.tar.gz"
curl -fsSL -o "$work/gitleaks.tar.gz" "$url"
[ "$(sha256 "$work/gitleaks.tar.gz")" = "$hash" ] || { echo "gitleaks: SHA-256 mismatch"; exit 1; }
tar -xzf "$work/gitleaks.tar.gz" -C "$work" gitleaks
chmod +x "$work/gitleaks"
[ "$(sha256 "$work/gitleaks")" = "$binary" ] || { echo "gitleaks: extracted binary is not the pinned artifact"; exit 1; }
cp "$work/gitleaks" "$staged"
mv -f "$staged" "$dest/gitleaks"
"$dest/gitleaks" version
printf '%s\n' "$dest/gitleaks"
