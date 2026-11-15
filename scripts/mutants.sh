#!/usr/bin/env bash
# Mutation testing of the safety layer (spec: no mutant survives in
# ikeloud-safety without a justified exception). Only the crate's own tests
# count: the safety layer must be correct on its own.
#
# CI skips it when scripts/ci-cache-key.sh --mutants (the fingerprint of
# every input of this run) matches a run that already passed.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
# src/proofs.rs is compiled only for Kani; its mutants could never be caught.
exec cargo mutants --package ikeloud-safety --test-package ikeloud-safety \
    --exclude crates/ikeloud-safety/src/proofs.rs \
    --jobs "${MUTANTS_JOBS:-4}" --no-shuffle "$@"
