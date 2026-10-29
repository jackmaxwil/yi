#!/usr/bin/env bash
# Install yi. Beside an unpacked release this installs that tree; anywhere else (curl | bash) it
# downloads this machine's archive and installs it only if its sha256 matches the release's SHA256SUMS.
set -euo pipefail

releases=https://github.com/jackmaxwil/yi/releases
# Invariant: the one line of allowed_signers, which yi update compiles in; a release is signed by it.
signer='jackmaxwil-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICRKvW82jaXOFg8jkpg7ZHj8YkZ5TKQr9U1bU1ygJiAj'

usage() {
  cat <<'EOF'
usage: install.sh [--help]

  curl -fsSL https://raw.githubusercontent.com/jackmaxwil/yi/main/scripts/install.sh | bash

Installs yi into $YI_PREFIX (default ~/.local/bin) and its Python runtime and skills into ~/.yi.
Run from an unpacked release, it installs that release; otherwise it downloads one.

  YI_VERSION=X.Y.Z    install that release, not the latest
  YI_RELEASE_BASE=URL read SHA256SUMS and the archive from URL (https:// or file://)

With ssh-keygen on PATH it also checks the archive's .sig against the release key:
EOF
  printf '  %s\n' "$signer"
}

target() {
  local os arch
  case "$(uname -s)" in
    Darwin) os=apple-darwin ;;
    Linux) os=unknown-linux-musl ;;
    *) echo "install.sh: no yi release for $(uname -s)" >&2; return 1 ;;
  esac
  case "$(uname -m)" in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) echo "install.sh: no yi release for $(uname -m)" >&2; return 1 ;;
  esac
  printf '%s-%s\n' "$arch" "$os"
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

get() {
  curl -fsSL --proto '=https,file' --proto-redir '=https' -o "$2" "$1"
}

# The sources land in ~/.yi because python_root() falls back there, which is what lets the
# binary live anywhere on PATH. The tree's name, yi-<version>-<target>, is the receipt.
place() {
  local tree=$1 prefix=$2 root="$HOME/.yi" base rest
  mkdir -p "$prefix" "$root"
  # Invariant: each swap is a rename, so a yi that is running or starting meanwhile never reads a
  # half-copied binary or runtime; a running one keeps the file it opened.
  install -m 755 "$tree/bin/yi" "$prefix/.yi.new.$$"
  mv -f "$prefix/.yi.new.$$" "$prefix/yi"
  rm -rf "$root/.python.new" "$root/.python.old"
  cp -R "$tree/python" "$root/.python.new"
  [ ! -e "$root/python" ] || mv "$root/python" "$root/.python.old"
  mv "$root/.python.new" "$root/python"
  rm -rf "$root/.python.old"
  mkdir -p "$root/skills"
  # The release owns each skill directory it ships: one it dropped must not linger.
  for dir in "$tree"/skills/*/; do rm -rf "$root/skills/$(basename "$dir")"; done
  cp -R "$tree/skills/." "$root/skills/"

  base=$(basename "$tree")
  rest=${base#yi-}
  if [ "$rest" != "$base" ] && [ "${rest#*-}" != "$rest" ]; then
    printf '{"prefix":"%s","version":"%s","target":"%s"}\n' \
      "$(json_escape "$prefix")" "$(json_escape "${rest%%-*}")" "$(json_escape "${rest#*-}")" \
      > "$root/install.json"
  fi

  echo "installed $("$prefix/yi" --version </dev/null) at $prefix/yi"
  case ":$PATH:" in
    *":$prefix:"*) ;;
    *) echo "$prefix is not on PATH; add it: export PATH=\"$prefix:\$PATH\"" ;;
  esac
}

json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  printf '%s' "$s"
}

# A bad signature refuses the install; with no .sig or no ssh-keygen the sha256 is the only check,
# and that is said. YI_ALLOWED_SIGNERS names another signers file, as for scripts/sign.sh.
verify() {
  local base=$1 work=$2 name=$3 signers identity
  local got=0
  get "$base/$name.sig" "$work/$name.sig" 2>/dev/null || got=$?
  # curl's 22 is an HTTP error status and 37 an unreadable file://: the .sig is absent. Any other
  # failure is a fetch that went wrong, which is not evidence the release is unsigned.
  case $got in
    0) ;;
    22 | 37) echo "install.sh: this release has no signature; integrity is the sha256 check only" >&2; return 0 ;;
    *) echo "install.sh: fetching $name.sig failed (curl exit $got); nothing installed" >&2; return 1 ;;
  esac
  if ! command -v ssh-keygen >/dev/null; then
    echo "install.sh: no ssh-keygen here to check the signature; integrity is the sha256 check only" >&2
    return 0
  fi
  signers=${YI_ALLOWED_SIGNERS:-$work/allowed_signers}
  [ -n "${YI_ALLOWED_SIGNERS:-}" ] || printf '%s\n' "$signer" > "$signers"
  identity=$(awk 'NR == 1 { print $1 }' "$signers")
  if ! ssh-keygen -Y verify -f "$signers" -I "$identity" -n yi-release -s "$work/$name.sig" \
    < "$work/$name" >/dev/null 2>&1; then
    echo "install.sh: $name.sig is not $identity's signature of $name; nothing installed" >&2
    return 1
  fi
  echo "install.sh: $name is signed by $identity"
}

download() {
  local prefix=$1 triple base version pattern line want name got
  triple=$(target)
  version=${YI_VERSION:-}
  version=${version#v}
  if [ -n "${YI_RELEASE_BASE:-}" ]; then base=${YI_RELEASE_BASE%/}
  elif [ -n "$version" ]; then base="$releases/download/v$version"
  else base="$releases/latest/download"; fi

  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  get "$base/SHA256SUMS" "$work/SHA256SUMS" || {
    echo "install.sh: no SHA256SUMS at $base" >&2; return 1
  }
  pattern=${version//./\\.}
  line=$(grep -E "^[0-9a-f]{64}  yi-${pattern:-[0-9A-Za-z.+]+}-${triple}\.tar\.gz\$" "$work/SHA256SUMS" | head -n 1) || {
    echo "install.sh: the release at $base has no ${version:+yi-$version }archive for $triple" >&2; return 1
  }
  want=${line%% *}
  name=${line##* }
  get "$base/$name" "$work/$name"
  got=$(sha256 "$work/$name")
  if [ "$got" != "$want" ]; then
    echo "install.sh: $name has sha256 $got, but SHA256SUMS says $want; nothing installed" >&2
    return 1
  fi
  verify "$base" "$work" "$name" || return 1
  tar -xzf "$work/$name" -C "$work"
  [ -x "$work/${name%.tar.gz}/bin/yi" ] || {
    echo "install.sh: $name holds no ${name%.tar.gz}/bin/yi; nothing installed" >&2; return 1
  }
  place "$work/${name%.tar.gz}" "$prefix"
}

main() {
  local arg here prefix=${YI_PREFIX:-$HOME/.local/bin}
  for arg in "$@"; do
    case $arg in
      -h | --help) usage; return 0 ;;
      *) echo "install.sh: unknown argument $arg" >&2; usage >&2; return 2 ;;
    esac
  done
  # Piped into bash there is no script file, so only a file beside bin/yi is a release tree.
  here=${BASH_SOURCE[0]:-}
  if [ -n "$here" ] && [ -f "$here" ] && here=$(cd "$(dirname "$here")" && pwd) \
    && [ -x "$here/bin/yi" ] && [ -d "$here/python" ]; then
    place "$here" "$prefix"
    return
  fi
  download "$prefix"
}

# One call at the end, so a download cut off mid-script runs nothing.
main "$@"
