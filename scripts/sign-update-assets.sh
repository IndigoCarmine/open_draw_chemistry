#!/usr/bin/env bash
# Sign release assets for the app's automatic update: writes <asset>.sig (64 raw bytes) beside
# every file in a directory. See "Signing updates" in docs/DEVELOPMENT.md.
#
#   UPDATE_SIGNING_KEY="$(cat private-key.pem)" bash scripts/sign-update-assets.sh <directory>
#
# The signature is ed25519 over this message, which src/update/signature.rs builds in the same
# way: the asset's name (which holds the version, so that an older asset cannot pass as the
# latest) and its SHA-256.
#
#   open-draw-chemistry-update-v1\n<asset name>\n<sha256 in lower-case hex>\n
#
# Without the secret nothing is signed and the release still goes out: the app then sends users
# to the Release page. A secret that does not match resources/update-signing.pub stops the
# release, since every signature it made would be rejected. Needs OpenSSL 3.
set -euo pipefail

dir="${1:?usage: sign-update-assets.sh <directory>}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Overridden only to try this script with a throwaway key.
public_key_file="${UPDATE_SIGNING_PUBLIC_KEY_FILE:-$repo_root/resources/update-signing.pub}"

if [ -z "${UPDATE_SIGNING_KEY:-}" ]; then
  echo "::warning::UPDATE_SIGNING_KEY is not set: the release is not signed, so the app will send users to the Release page"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 700 "$work"
key="$work/key.pem"
(umask 077 && printf '%s\n' "$UPDATE_SIGNING_KEY" > "$key")

# The last 32 bytes of an ed25519 public key's DER form are the raw key.
public_hex="$(openssl pkey -in "$key" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n')"
if ! grep -qix "$public_hex" "$public_key_file"; then
  echo "::error::UPDATE_SIGNING_KEY does not match $public_key_file"
  exit 1
fi

signed=0
for asset in "$dir"/*; do
  [ -f "$asset" ] || continue
  case "$asset" in *.sig) continue ;; esac
  name="$(basename "$asset")"
  sha256="$(openssl dgst -sha256 -r "$asset" | cut -d ' ' -f 1)"
  printf 'open-draw-chemistry-update-v1\n%s\n%s\n' "$name" "$sha256" > "$work/message"
  openssl pkeyutl -sign -rawin -inkey "$key" -in "$work/message" -out "$asset.sig"
  echo "Signed $name"
  signed=$((signed + 1))
done
if [ "$signed" -eq 0 ]; then
  echo "::error::nothing to sign in $dir"
  exit 1
fi
