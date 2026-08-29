#!/usr/bin/env bash
# Assemble one release tarball. The binary alone is not a release: the kernel
# bootstraps from `python/yi_runtime`, installs `python/skills` into the venv,
# and the model reads `skills/`, so all three ship beside it.
#
#   scripts/package.sh <version> <target> [binary-path]
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: package.sh <version> <target> [binary]}"
target="${2:?usage: package.sh <version> <target> [binary]}"
binary="${3:-target/${target}/dist/yi}"

[ -x "$binary" ] || { echo "no binary at $binary"; exit 1; }

name="yi-${version}-${target}"
stage="target/package/${name}"
rm -rf "$stage"
mkdir -p "$stage/bin"

cp "$binary" "$stage/bin/yi"
# `python/` must keep its layout: python_root() locates the tree by finding
# `python/yi_runtime` while walking up from the binary.
cp -R python "$stage/python"
cp -R skills "$stage/skills"
cp -R completions "$stage/completions"
cp README.md "$stage/README.md"
cp scripts/install.sh "$stage/install.sh"
chmod +x "$stage/install.sh"

mkdir -p target/package
tar -C target/package -czf "target/package/${name}.tar.gz" "$name"
( cd target/package && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256" )
echo "target/package/${name}.tar.gz"
