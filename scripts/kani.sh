#!/usr/bin/env bash
# Kani proofs of the safety layer (cfg(kani) modules in ikeloud-safety).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec cargo kani --package ikeloud-safety "$@"
