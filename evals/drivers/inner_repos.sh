#!/bin/sh
# The inner loop's real repos (evals/inner/gen/mutate.py REPOS, revert.py REPOS), cloned whole into
# one directory: revert reads their history, so a shallow clone is refused by revert.available().
# Each is checked for the commit its generator pins, so a moved upstream fails here, by name.
set -eu
dest="${1:?usage: inner_repos.sh <dir>}"
mkdir -p "$dest"
clone() {
  name=$1 url=$2 pin=$3
  [ -d "$dest/$name/.git" ] || git clone --quiet "$url" "$dest/$name"
  git -C "$dest/$name" cat-file -e "$pin^{commit}" || { echo "refused: $name lacks its pinned commit $pin" >&2; exit 1; }
}
clone markdown https://github.com/Python-Markdown/markdown.git 0ffbf00
clone more-itertools https://github.com/more-itertools/more-itertools.git 790bb0b
clone pyparsing https://github.com/pyparsing/pyparsing.git f803ae8
clone tomli https://github.com/hukkin/tomli.git 5a77b12
