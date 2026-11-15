#!/usr/bin/env bash
# Removes the build artifacts of our own workspace crates from target/.
#
# CI caches only compiled third-party dependencies (vendor/). Our own code
# must always be compiled and tested from the current sources, so its
# artifacts are removed after restoring the cache and before saving it.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
members="$(cargo metadata --format-version 1 --no-deps --locked --offline \
    | python3 -c 'import json, sys; print(" ".join(p["name"] for p in json.load(sys.stdin)["packages"]))')"
for member in $members; do
    cargo clean --offline --locked -p "$member"
done
# Incremental state and test binaries of our crates are never reused either.
rm -rf target/debug/incremental
echo "removed artifacts of: $members"
