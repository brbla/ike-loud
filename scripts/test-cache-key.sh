#!/usr/bin/env bash
# Tests scripts/ci-cache-key.sh: every input of the dependency cache must
# change the key, and changes that do not affect the cached artifacts (our
# own code, docs) must not. Runs on a copy, never touches the repository.
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Copy only what the key reads (plus a source file and a doc for the
# "must not change" cases). vendor/ contributes only its checksum files.
cd "$ROOT"
mkdir -p "$WORK/repo"
{
    printf '%s\n' scripts/ci-cache-key.sh scripts/mutants.sh scripts/kani.sh scripts/kani-setup.sh \
        Cargo.lock Cargo.toml \
        .cargo/config.toml rust-toolchain.toml README.md crates/ikeloud-sync/src/lib.rs
    find crates -mindepth 2 -maxdepth 2 -name Cargo.toml
    find crates/ikeloud-safety -type f
    find vendor -name .cargo-checksum.json
} | while read -r f; do
    mkdir -p "$WORK/repo/$(dirname "$f")"
    cp "$f" "$WORK/repo/$f"
done
REPO="$WORK/repo"
KEY="$REPO/scripts/ci-cache-key.sh"

# Fake tools that report a different version, for the toolchain cases.
FAKE="$WORK/fake"
mkdir -p "$FAKE"
fake_tool() { # name, text printed by --version / -vV
    printf '#!/bin/sh\necho "%s"\n' "$2" > "$FAKE/$1"
    chmod +x "$FAKE/$1"
}

# Mode of the key under test (empty: dependency cache; --mutants).
MODE=()
baseline="$(cd "$REPO" && "$KEY")"
# The key must be deterministic: compute it many times.
for _ in $(seq 1 20); do
    if [[ "$(cd "$REPO" && "$KEY")" != "$baseline" ]]; then
        echo "FAIL the key is not deterministic" >&2
        exit 1
    fi
done
echo "ok   deterministic (21 runs)"
failures=0

# check NAME same|different COMMAND...  (COMMAND runs in a subshell in $REPO)
check() {
    local name="$1" expect="$2"
    shift 2
    local snapshot="$WORK/snapshot"
    rm -rf "$snapshot" && cp -r "$REPO" "$snapshot"
    local key
    key="$(cd "$REPO" && "$@" && "$KEY" "${MODE[@]}")" || key="error"
    rm -rf "$REPO" && mv "$snapshot" "$REPO"
    local got=same
    [[ "$key" != "$baseline" ]] && got=different
    if [[ "$got" == "$expect" && "$key" != "error" ]]; then
        echo "ok   $name ($got)"
    else
        echo "FAIL $name: expected $expect, got $got ($key)"
        failures=$((failures + 1))
    fi
}

edit() { printf '\n# changed\n' >> "$1"; }
first_vendor_checksum="$(cd "$REPO" && find vendor -name .cargo-checksum.json | sort | head -n1)"

check "repeated run" same true
check "unrelated env var" same export SOME_UNRELATED_VARIABLE=x
check "GitHub run id" same export GITHUB_RUN_ID=123
check "runner image build number" same export ImageVersion=20261111.999.1
check "our source code" same edit crates/ikeloud-sync/src/lib.rs
check "documentation" same edit README.md
check "Cargo.lock" different edit Cargo.lock
check "workspace manifest" different edit Cargo.toml
check "crate manifest" different edit crates/ikeloud-safety/Cargo.toml
check "cargo config" different edit .cargo/config.toml
check "rust-toolchain.toml" different edit rust-toolchain.toml
check "vendored crate changed" different sed -i 's/"package"/"package" /' "$first_vendor_checksum"
check "vendored crate added" different sh -c 'mkdir -p vendor/new-1.0.0 && echo "{}" > vendor/new-1.0.0/.cargo-checksum.json'
check "new workspace crate" different sh -c 'mkdir -p crates/new && echo "[package]" > crates/new/Cargo.toml'
for var in RUSTFLAGS RUSTC_WRAPPER CARGO_INCREMENTAL CARGO_PROFILE_DEV_OPT_LEVEL CC CC_x86_64_unknown_linux_gnu \
    CFLAGS CFLAGS_x86_64_unknown_linux_gnu TARGET_CC HOST_CFLAGS CRATE_CC_NO_DEFAULTS AR LDFLAGS \
    PKG_CONFIG_PATH LIBSQLITE3_FLAGS SQLITE3_LIB_DIR ImageOS; do
    check "env $var" different export "$var=changed"
done

# Every variable a vendored build script watches must change the key.
# (Templates like "{}_LIB_DIR" are covered by the patterns tested above.)
watched="$(cd "$ROOT" && grep -rhoE 'rerun-if-env-changed=[A-Za-z0-9_]+' vendor/*/build.rs vendor/*/build/*.rs 2>/dev/null \
    | cut -d= -f2 | sort -u)"
for var in $watched; do
    check "build script env $var" different export "$var=changed"
done
fake_tool rustc "rustc 0.0.0-fake"
check "rustc version" different export PATH="$FAKE:$PATH"
rm "$FAKE/rustc"; fake_tool cargo "cargo 0.0.0-fake"
check "cargo version" different export PATH="$FAKE:$PATH"
rm "$FAKE/cargo"; fake_tool cc "cc 0.0.0-fake"
check "C compiler version" different export PATH="$FAKE:$PATH"
rm "$FAKE/cc"; fake_tool ar "ar 0.0.0-fake"
check "archiver version" different export PATH="$FAKE:$PATH"
rm "$FAKE/ar"; fake_tool ld "ld 0.0.0-fake"
check "linker version" different export PATH="$FAKE:$PATH"
rm "$FAKE/ld"; fake_tool ldd "ldd (fake libc) 0.0"
check "C library version" different export PATH="$FAKE:$PATH"
rm "$FAKE/ldd"

# The key of a passed mutation run (--mutants): every input of the run
# changes it; code the safety layer does not use does not.
MODE=(--mutants)
baseline="$(cd "$REPO" && "$KEY" --mutants)"
for _ in $(seq 1 5); do
    [[ "$(cd "$REPO" && "$KEY" --mutants)" == "$baseline" ]] || {
        echo "FAIL the mutants key is not deterministic" >&2
        exit 1
    }
done
check "mutants: repeated run" same true
check "mutants: another crate's code" same edit crates/ikeloud-sync/src/lib.rs
check "mutants: documentation" same edit README.md
check "mutants: vendored crate (safety has none)" same sed -i 's/"package"/"package" /' "$first_vendor_checksum"
check "mutants: safety source" different edit crates/ikeloud-safety/src/lib.rs
check "mutants: safety test" different edit crates/ikeloud-safety/tests/assessment.rs
check "mutants: new safety test" different sh -c 'echo "// x" > crates/ikeloud-safety/tests/new.rs'
check "mutants: safety manifest" different edit crates/ikeloud-safety/Cargo.toml
check "mutants: workspace manifest (lints)" different edit Cargo.toml
check "mutants: Cargo.lock" different edit Cargo.lock
check "mutants: cargo config" different edit .cargo/config.toml
check "mutants: rust-toolchain.toml" different edit rust-toolchain.toml
check "mutants: the script" different edit scripts/mutants.sh
check "mutants: cargo-mutants pin" different export CARGO_MUTANTS_VERSION=changed
check "mutants: RUSTFLAGS" different export RUSTFLAGS=changed
fake_tool rustc "rustc 0.0.0-fake"
check "mutants: rustc version" different export PATH="$FAKE:$PATH"
rm "$FAKE/rustc"
check "mutants: the Kani scripts" same edit scripts/kani.sh

# The key of passed Kani proofs (--kani): the same inputs, its own scripts.
MODE=(--kani)
baseline="$(cd "$REPO" && "$KEY" --kani)"
[[ "$baseline" != "$(cd "$REPO" && "$KEY" --mutants)" ]] || {
    echo "FAIL kani and mutants share a key" >&2
    exit 1
}
check "kani: repeated run" same true
check "kani: another crate's code" same edit crates/ikeloud-sync/src/lib.rs
check "kani: safety source" different edit crates/ikeloud-safety/src/lib.rs
check "kani: the proofs" different edit crates/ikeloud-safety/src/proofs.rs
check "kani: workspace manifest" different edit Cargo.toml
check "kani: rust-toolchain.toml" different edit rust-toolchain.toml
check "kani: the script" different edit scripts/kani.sh
check "kani: version and bundle checksum" different edit scripts/kani-setup.sh
check "kani: the mutants script" same edit scripts/mutants.sh
MODE=()

# The tool cache key (--tools): pins, toolchain and platform change it.
tools_baseline="$(cd "$WORK" && CARGO_DENY_VERSION=1 CARGO_AUDIT_VERSION=1 CARGO_VET_VERSION=1 "$KEY" --tools set-a)"
check_tools() { # name, env assignment, expected[, tool set]
    local key
    key="$(cd "$WORK" && env CARGO_DENY_VERSION=1 CARGO_AUDIT_VERSION=1 CARGO_VET_VERSION=1 "$2" "$KEY" --tools "${4:-set-a}")"
    local got=same
    [[ "$key" != "$tools_baseline" ]] && got=different
    if [[ "$got" == "$3" ]]; then echo "ok   tools: $1 ($got)"; else
        echo "FAIL tools: $1: expected $3, got $got"; failures=$((failures + 1)); fi
}
check_tools "repeated run" "X_UNRELATED=1" same
check_tools "runner image build number" "ImageVersion=20261111.999.1" same
check_tools "cargo-deny pin" "CARGO_DENY_VERSION=2" different
check_tools "cargo-audit pin" "CARGO_AUDIT_VERSION=2" different
check_tools "cargo-vet pin" "CARGO_VET_VERSION=2" different
check_tools "a new pin, not listed anywhere" "CARGO_NEW_TOOL_VERSION=1" different
check_tools "another tool set (another CI job)" "X_UNRELATED=1" different set-b
check_tools "runner OS" "ImageOS=ubuntu99" different
fake_tool rustc "rustc 0.0.0-fake"
check_tools "rustc version" "PATH=$FAKE:$PATH" different
rm "$FAKE/rustc"; fake_tool ldd "ldd (fake libc) 0.0"
check_tools "C library version" "PATH=$FAKE:$PATH" different
rm "$FAKE/ldd"

if [[ $failures -gt 0 ]]; then
    echo "$failures cache key check(s) failed" >&2
    exit 1
fi
echo "cache key covers all inputs"
