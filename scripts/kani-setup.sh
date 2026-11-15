#!/usr/bin/env bash
# Installs Kani in a pinned version from a bundle verified by its checksum
# (docs/rozhodnuti/0007: we trust nobody, so a prebuilt download is pinned
# like tla2tools.jar). Builds cargo-kani from crates.io source, outside the
# repository (inside it, .cargo/config.toml redirects crates.io to vendor/).
set -euo pipefail

KANI_VERSION=0.68.0
BUNDLE="kani-$KANI_VERSION-x86_64-unknown-linux-gnu.tar.gz"
BUNDLE_SHA256=32e2b484d73ede0bbf64a2cf0879c4259422497d8aec4ee67d448de9ae7843d3

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
cargo install --locked kani-verifier --version "$KANI_VERSION"
curl -sSLf --retry 5 --retry-all-errors -o "$BUNDLE" \
    "https://github.com/model-checking/kani/releases/download/kani-$KANI_VERSION/$BUNDLE"
echo "$BUNDLE_SHA256  $BUNDLE" | sha256sum --check --strict
cargo kani setup --use-local-bundle "$work/$BUNDLE"
