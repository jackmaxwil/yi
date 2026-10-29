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
cp README.md LICENSE NOTICE "$stage/"
cp scripts/install.sh "$stage/install.sh"
chmod +x "$stage/install.sh"

mkdir -p target/package
tar -C target/package -czf "target/package/${name}.tar.gz" "$name"
( cd target/package && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256" )
# Invariant: an archive leaves here signed; a signature that fails takes the archive with it, so
# no unsigned one is left for `just publish` to find.
if ! scripts/sign.sh "target/package/${name}.tar.gz"; then
  rm -f "target/package/${name}.tar.gz" "target/package/${name}.tar.gz.sha256"
  exit 1
fi
echo "target/package/${name}.tar.gz"
