#!/usr/bin/env bash
# Prints the cache key of compiled third-party dependencies (target/).
#
# The key is a fingerprint of EVERY input the cached artifacts depend on.
# If any input changes, the key changes and the cache is not used.
# Our own crates are not an input: they are never cached
# (scripts/ci-clean-workspace.sh) and always rebuilt.
#
# Usage: scripts/ci-cache-key.sh                  -> key of the dependency cache
#        scripts/ci-cache-key.sh --explain        -> its fingerprinted inputs
#        scripts/ci-cache-key.sh --tools SET      -> key of the CI tool cache SET
#        scripts/ci-cache-key.sh --tools SET --explain
#        scripts/ci-cache-key.sh --mutants        -> key of a passed mutation run
#        scripts/ci-cache-key.sh --kani           -> key of passed Kani proofs
#        scripts/ci-cache-key.sh --mutants|--kani --explain
# --tools fingerprints the name of the tool set (each CI job caches its own
# tools), the platform, the Rust toolchain active in the current directory
# and every pinned tool version: all variables CARGO_*_VERSION, so a new pin
# is covered without touching this script.
#
# --mutants and --kani fingerprint everything a run of scripts/mutants.sh or
# scripts/kani.sh depends on: the safety crate (sources and tests),
# workspace manifests and lock, cargo config, toolchain, build environment
# (including tool pins) and the scripts themselves (Kani's version and
# bundle checksum live in scripts/kani-setup.sh). A passed run under the
# same key need not run again.
#
# Adding an input: add it below AND a case to scripts/test-cache-key.sh.
# Rules: .claude/skills/ci-cache/SKILL.md
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Bump when the meaning of the cache changes (layout, cleaning rules).
SCHEMA=4

# Names of environment variables that can affect compilation.
ENV_PATTERN='CARGO_[A-Z0-9_]+|RUST[A-Z0-9_]*|CC|CC_[A-Za-z0-9_]+|CXX|CXX_[A-Za-z0-9_]+|CFLAGS[A-Za-z0-9_]*|CXXFLAGS[A-Za-z0-9_]*|CPPFLAGS[A-Za-z0-9_]*|LDFLAGS[A-Za-z0-9_]*|AR|AR_[A-Za-z0-9_]+|ARFLAGS[A-Za-z0-9_]*|RANLIB[A-Za-z0-9_]*|TARGET_[A-Za-z0-9_]+|HOST_[A-Za-z0-9_]+|CRATE_CC_[A-Z0-9_]+|PKG_CONFIG[A-Za-z0-9_]*|LIBSQLITE3[A-Z0-9_]*|SQLITE[A-Z0-9_]*|VCPKG[A-Z0-9_]*|LIBC_[A-Z0-9_]+|DEP_[A-Z0-9_]+|DOCS_RS|NUM_JOBS|OPT_LEVEL|DEBUG|PROFILE'

# First line of a command's output. Reads all output first: piping into
# `head` would race with pipefail (SIGPIPE) and make the key nondeterministic.
first_line() {
    local out
    out="$("$@" 2>/dev/null)" || out="missing"
    printf '%s\n' "${out%%$'\n'*}"
}

# What the platform contributes to compiled artifacts. Deliberately not the
# runner image build number: it changes between runners of the same image
# without changing any of the tools below.
platform() {
    echo "os: $(uname -s) $(uname -m) ${ImageOS:-}"
    # Rust toolchain (rustup resolves rust-toolchain.toml in this directory).
    rustc -vV | sed 's/^/rustc: /'
    echo "cargo: $(cargo -V)"
    # C compiler (the bundled SQLite is C), archiver, linker, C library.
    echo "cc: $(first_line "${CC:-cc}" --version)"
    echo "ar: $(first_line "${AR:-ar}" --version)"
    echo "ld: $(first_line ld --version)"
    echo "libc: $(first_line ldd --version)"
}

tool_inputs() {
    echo "schema: tools-$SCHEMA"
    echo "tool set: $1"
    platform
    env | grep -E '^CARGO_[A-Z0-9_]+_VERSION=' | sort | sed 's/^/pin /' || true
}

inputs() {
    echo "schema: $SCHEMA"
    platform
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

result_inputs() { # name, scripts...
    echo "schema: $1-$SCHEMA"
    shift
    platform
    env | grep -E "^($ENV_PATTERN)=" | sort | sed 's/^/env /' || true
    {
        printf '%s\n' Cargo.lock Cargo.toml .cargo/config.toml rust-toolchain.toml "$@"
        find crates/ikeloud-safety -type f
    } | sort | while read -r file; do
        echo "file $file: $(sha256sum "$file" | cut -d' ' -f1)"
    done
}

if [[ "${1:-}" == "--mutants" || "${1:-}" == "--kani" ]]; then
    cd "$ROOT"
    case "$1" in
        --mutants) inputs_of() { result_inputs mutants scripts/mutants.sh; } ;;
        --kani) inputs_of() { result_inputs kani scripts/kani.sh scripts/kani-setup.sh; } ;;
    esac
    if [[ "${2:-}" == "--explain" ]]; then inputs_of; exit 0; fi
    echo "${1#--}-passed-$(uname -s)-$(inputs_of | sha256sum | cut -c1-32)"
    exit 0
fi

if [[ "${1:-}" == "--tools" ]]; then
    # Runs where the tools are built (outside the repository).
    set_name="${2:?usage: --tools SET [--explain]}"
    if [[ "${3:-}" == "--explain" ]]; then tool_inputs "$set_name"; exit 0; fi
    echo "tools-$set_name-$(uname -s)-$(tool_inputs "$set_name" | sha256sum | cut -c1-32)"
    exit 0
fi
cd "$ROOT"
if [[ "${1:-}" == "--explain" ]]; then
    inputs
    exit 0
fi
echo "deps-$(uname -s)-$(inputs | sha256sum | cut -c1-32)"
