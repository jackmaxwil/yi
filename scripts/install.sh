#!/usr/bin/env bash
# Install yi. Beside an unpacked release this installs that tree; anywhere else (curl | bash) it
# downloads this machine's archive and installs it only if its sha256 matches the release's SHA256SUMS.
set -euo pipefail

releases=https://github.com/jackmaxwil/yi/releases

usage() {
  cat <<'EOF'
usage: install.sh [--dry-run] [--help]

  curl -fsSL https://raw.githubusercontent.com/jackmaxwil/yi/main/scripts/install.sh | bash

Installs yi into $YI_PREFIX (default ~/.local/bin) and its Python runtime and skills into ~/.yi.
Run from an unpacked release, it installs that release; otherwise it downloads one.

  --dry-run           print what would be downloaded and installed, and change nothing
  YI_VERSION=X.Y.Z    install that release, not the latest
  YI_RELEASE_BASE=URL read SHA256SUMS and the archive from URL (https:// or file://)
EOF
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
  install -m 755 "$tree/bin/yi" "$prefix/yi"
  rm -rf "$root/python"
  cp -R "$tree/python" "$root/python"
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

download() {
  local prefix=$1 dry=$2 triple base version pattern line want name got
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
  if [ "$dry" = 1 ]; then
    echo "would download $base/$name (sha256 $want)"
    echo "would install $prefix/yi, and the runtime and skills into $HOME/.yi"
    return 0
  fi
  get "$base/$name" "$work/$name"
  got=$(sha256 "$work/$name")
  if [ "$got" != "$want" ]; then
    echo "install.sh: $name has sha256 $got, but SHA256SUMS says $want; nothing installed" >&2
    return 1
  fi
  get "$base/$name.sig" "$work/$name.sig" 2>/dev/null \
    || echo "install.sh: this release is unsigned; integrity is the sha256 check only"
  tar -xzf "$work/$name" -C "$work"
  [ -x "$work/${name%.tar.gz}/bin/yi" ] || {
    echo "install.sh: $name holds no ${name%.tar.gz}/bin/yi; nothing installed" >&2; return 1
  }
  place "$work/${name%.tar.gz}" "$prefix"
}

main() {
  local dry=0 arg here prefix=${YI_PREFIX:-$HOME/.local/bin}
  for arg in "$@"; do
    case $arg in
      -h | --help) usage; return 0 ;;
      --dry-run) dry=1 ;;
      *) echo "install.sh: unknown argument $arg" >&2; usage >&2; return 2 ;;
    esac
  done
  # Piped into bash there is no script file, so only a file beside bin/yi is a release tree.
  here=${BASH_SOURCE[0]:-}
  if [ -n "$here" ] && [ -f "$here" ] && here=$(cd "$(dirname "$here")" && pwd) \
    && [ -x "$here/bin/yi" ] && [ -d "$here/python" ]; then
    if [ "$dry" = 1 ]; then echo "would install $here/bin/yi into $prefix"; return 0; fi
    place "$here" "$prefix"
    return
  fi
  download "$prefix" "$dry"
}

# One call at the end, so a download cut off mid-script runs nothing.
main "$@"
