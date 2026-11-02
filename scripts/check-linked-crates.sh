#!/usr/bin/env bash
# Fail if a crate that may not be linked with OpenBabel gets into a platform's build. See
# "HTTPS and licences" in docs/DEVELOPMENT.md.
#
#   bash scripts/check-linked-crates.sh
#
# Each target is checked by itself because `cargo tree --target all` ignores cfg: native-tls
# declares `openssl` for Linux, so `openssl` is in Cargo.lock and in that output even though this
# app only uses native-tls on Windows and macOS. The whole list is searched, rather than asking
# `cargo tree -i <crate>`, which fails the same way for a crate that is absent as for a broken
# build.
set -euo pipefail

targets=(x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu)
# ring and openssl carry Apache-2.0 terms, which GPL-2.0-only does not allow; the rest would only
# bring them, or Mozilla's root certificates (CDLA-Permissive-2.0), back in.
forbidden=(ring rustls openssl openssl-sys webpki-roots webpki-root-certs)

failed=0
for target in "${targets[@]}"; do
  crates="$(cargo tree --locked -e normal --target "$target" --prefix none)"
  for crate in "${forbidden[@]}"; do
    if grep -q "^$crate v" <<<"$crates"; then
      echo "::error::$crate is in the build for $target (cargo tree -e normal --target $target -i $crate)"
      failed=1
    fi
  done
done
if [ "$failed" -eq 0 ]; then
  echo "None of ${forbidden[*]} is in the build for ${targets[*]}."
fi
exit "$failed"
