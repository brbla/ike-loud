#!/usr/bin/env bash
# Prints the cache key of compiled third-party dependencies (target/).
#
# The key is a fingerprint of EVERY input the cached artifacts depend on.
# If any input changes, the key changes and the cache is not used.
# Our own crates are not an input: they are never cached
# (scripts/ci-clean-workspace.sh) and always rebuilt.
#
# Usage: scripts/ci-cache-key.sh            -> the key
#        scripts/ci-cache-key.sh --explain  -> the fingerprinted inputs
#
# Adding an input: add it below AND a case to scripts/test-cache-key.sh.
# Rules: .claude/skills/ci-cache/SKILL.md
set -euo pipefail
export LC_ALL=C

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Bump when the meaning of the cache changes (layout, cleaning rules).
SCHEMA=3

# Names of environment variables that can affect compilation.
ENV_PATTERN='CARGO_[A-Z0-9_]+|RUST[A-Z0-9_]*|CC|CC_[A-Za-z0-9_]+|CXX|CXX_[A-Za-z0-9_]+|CFLAGS[A-Za-z0-9_]*|CXXFLAGS[A-Za-z0-9_]*|CPPFLAGS[A-Za-z0-9_]*|LDFLAGS[A-Za-z0-9_]*|AR|AR_[A-Za-z0-9_]+|ARFLAGS[A-Za-z0-9_]*|RANLIB[A-Za-z0-9_]*|TARGET_[A-Za-z0-9_]+|HOST_[A-Za-z0-9_]+|CRATE_CC_[A-Z0-9_]+|PKG_CONFIG[A-Za-z0-9_]*|LIBSQLITE3[A-Z0-9_]*|SQLITE[A-Z0-9_]*|VCPKG[A-Z0-9_]*|LIBC_[A-Z0-9_]+|DEP_[A-Z0-9_]+|DOCS_RS|NUM_JOBS|OPT_LEVEL|DEBUG|PROFILE'

first_line() { "$@" 2>/dev/null | head -n1 || echo "missing"; }

inputs() {
    echo "schema: $SCHEMA"
    # Platform and runner image (system libraries, linker).
    echo "os: $(uname -s) $(uname -m)"
    echo "runner-image: ${ImageOS:-} ${ImageVersion:-}"
    # Rust toolchain (rustup resolves rust-toolchain.toml here).
    rustc -vV | sed 's/^/rustc: /'
    echo "cargo: $(cargo -V)"
    # C compiler used by build scripts (the bundled SQLite is C).
    echo "cc: $(first_line "${CC:-cc}" --version)"
    echo "ar: $(first_line "${AR:-ar}" --version)"
    # Environment that can change how anything is compiled: every variable
    # whose name matches a build-related pattern (not a fixed list, so a new
    # variable of a known family is covered automatically).
    # scripts/test-cache-key.sh checks that every variable a vendored build
    # script declares with rerun-if-env-changed matches.
    env | grep -E "^($ENV_PATTERN)=" | sort | sed 's/^/env /' || true
    # Files: lock file, manifests, cargo config, toolchain and the
    # checksums cargo itself verifies for every vendored crate.
    {
        printf '%s\n' Cargo.lock Cargo.toml .cargo/config.toml rust-toolchain.toml
        find crates -mindepth 2 -maxdepth 2 -name Cargo.toml
        find vendor -name .cargo-checksum.json
    } | sort | while read -r file; do
        echo "file $file: $(sha256sum "$file" | cut -d' ' -f1)"
    done
}

if [[ "${1:-}" == "--explain" ]]; then
    inputs
    exit 0
fi
echo "deps-$(uname -s)-$(inputs | sha256sum | cut -c1-32)"
