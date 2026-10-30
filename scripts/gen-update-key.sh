#!/usr/bin/env bash
# Generate the ed25519 key pair that signs the app's updates, once. See "Signing updates" in
# docs/DEVELOPMENT.md.
#
#   bash scripts/gen-update-key.sh [private-key-file]
#
# Writes the private key to the file given (by default outside the repository), which is never
# to be committed, and the public key to resources/update-signing.pub, which is. Needs OpenSSL 3.
set -euo pipefail

private_key="${1:-$HOME/open-draw-chemistry-update-signing.pem}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
public_key_file="${UPDATE_SIGNING_PUBLIC_KEY_FILE:-$repo_root/resources/update-signing.pub}"

if [ -e "$private_key" ]; then
  echo "error: $private_key already exists; a new key would lock out every installed copy" >&2
  exit 1
fi

(umask 077 && openssl genpkey -algorithm ed25519 -out "$private_key")

# The last 32 bytes of an ed25519 public key's DER form are the raw key.
public_hex="$(openssl pkey -in "$private_key" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n')"
{
  echo "# The public key the app checks updates against (see src/update/signature.rs and \"Signing"
  echo "# updates\" in docs/DEVELOPMENT.md): 64 hex digits, written by scripts/gen-update-key.sh."
  echo "$public_hex"
} > "$public_key_file"

cat <<EOF
Private key: $private_key (keep it safe and out of the repository)
Public key:  $public_key_file

Next:
  1. gh secret set UPDATE_SIGNING_KEY --repo IndigoCarmine/open_draw_chemistry < "$private_key"
  2. Commit $public_key_file.
EOF
