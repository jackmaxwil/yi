#!/usr/bin/env bash
# Detached signature over a release artifact, using ssh-keygen rather than
# minisign or gpg: every machine that can push to this repo already has it.
#
# A sha256 sitting beside its own tarball on the same server proves nothing
# about the server, which is the only thing a signature is for here.
#
#   scripts/sign.sh <file>
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
file="${1:?usage: sign.sh <file>}"
key="${YI_SIGNING_KEY:-$HOME/.ssh/id_ed25519}"
signers="${YI_ALLOWED_SIGNERS:-allowed_signers}"
namespace="yi-release"

if [ ! -f "$key" ]; then
  echo "no signing key at $key"
  echo "  point YI_SIGNING_KEY at an ssh private key, or make one:"
  echo "    ssh-keygen -t ed25519 -f ~/.ssh/yi-release -C yi-release"
  exit 1
fi
if [ ! -f "$signers" ]; then
  echo "no $signers to verify against"
  echo "  commit the public half so a downloader can check a release:"
  echo "    printf '%s %s\\n' \"\$(git config user.email)\" \"\$(cat ${key}.pub)\" > $signers"
  exit 1
fi

rm -f "$file.sig"
ssh-keygen -Y sign -f "$key" -n "$namespace" "$file" >/dev/null 2>&1
# Signing with a key nobody can verify against is worse than not signing, so
# the signature is checked here rather than first by whoever downloads it.
identity=$(awk 'NR==1{print $1}' "$signers")
ssh-keygen -Y verify -f "$signers" -I "$identity" -n "$namespace" -s "$file.sig" < "$file" > /dev/null
echo "signed $file.sig ($identity)"
