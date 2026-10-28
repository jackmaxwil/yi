#!/usr/bin/env bash
set -euo pipefail
version=8.30.1
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) asset=linux_x64; hash=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb ;;
  Darwin-arm64) asset=darwin_arm64; hash=b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5 ;;
  *) echo "gitleaks: unsupported platform $(uname -s)-$(uname -m)"; exit 1 ;;
esac
dest="${1:-/usr/local/bin}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
url="https://github.com/gitleaks/gitleaks/releases/download/v${version}/gitleaks_${version}_${asset}.tar.gz"
curl -fsSL -o "$work/gitleaks.tar.gz" "$url"
echo "$hash  $work/gitleaks.tar.gz" | sha256sum -c -
tar -xzf "$work/gitleaks.tar.gz" -C "$work" gitleaks
chmod +x "$work/gitleaks"
mkdir -p "$dest"
mv "$work/gitleaks" "$dest/gitleaks"
"$dest/gitleaks" version
