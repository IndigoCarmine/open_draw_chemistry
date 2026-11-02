#!/usr/bin/env bash
# What gen-update-key.sh and sign-update-assets.sh do with the public key file, with throwaway
# keys. CI runs this. Needs OpenSSL 3.
#
#   bash scripts/test-update-key-scripts.sh
set -uo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
check() { # <what> <expected exit code> <actual exit code>
  if [ "$2" -eq "$3" ]; then
    echo "ok: $1"
  else
    echo "FAILED: $1: exit code $3, expected $2"
    failed=1
  fi
}

generate() { # <private key file>, with the public key file in $UPDATE_SIGNING_PUBLIC_KEY_FILE
  bash "$scripts/gen-update-key.sh" "$1" >/dev/null 2>&1
}

sign() { # <directory>, with the secret in $UPDATE_SIGNING_KEY
  bash "$scripts/sign-update-assets.sh" "$1" >/dev/null 2>&1
}

printf '# A comment.\nPLACEHOLDER\n' >"$work/placeholder.pub"
export UPDATE_SIGNING_PUBLIC_KEY_FILE="$work/key.pub"
mkdir "$work/assets"
echo asset >"$work/assets/app.exe"

cp "$work/placeholder.pub" "$work/key.pub"
generate "$work/first.pem"
check "a key replaces the placeholder" 0 $?
real_key="$(cat "$work/key.pub")"

generate "$work/second.pem"
check "a second key does not replace a real one" 1 $?
check "and leaves its private key unwritten" 1 "$([ -e "$work/second.pem" ] && echo 0 || echo 1)"
check "and leaves the public key alone" 0 "$([ "$(cat "$work/key.pub")" = "$real_key" ] && echo 0 || echo 1)"

rm "$work/key.pub"
generate "$work/third.pem"
check "a key can be written where there is no file yet" 0 $?

printf '%s\n' "$real_key" >"$work/key.pub"
generate "$work/first.pem"
check "an existing private key is still not overwritten" 1 $?

unset UPDATE_SIGNING_KEY
sign "$work/assets"
check "no secret with a real key committed stops the release" 1 $?
check "and signs nothing" 1 "$([ -e "$work/assets/app.exe.sig" ] && echo 0 || echo 1)"

cp "$work/placeholder.pub" "$work/key.pub"
sign "$work/assets"
check "no secret with the placeholder only warns" 0 $?
check "and signs nothing" 1 "$([ -e "$work/assets/app.exe.sig" ] && echo 0 || echo 1)"

printf '%s\n' "$real_key" >"$work/key.pub"
UPDATE_SIGNING_KEY="$(cat "$work/first.pem")" sign "$work/assets"
check "the matching secret signs" 0 $?
check "and writes the signature" 0 "$([ -s "$work/assets/app.exe.sig" ] && echo 0 || echo 1)"

UPDATE_SIGNING_KEY="$(cat "$work/third.pem")" sign "$work/assets"
check "another secret stops the release" 1 $?

exit "$failed"
