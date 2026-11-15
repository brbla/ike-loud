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
    printf '%s\n' scripts/ci-cache-key.sh Cargo.lock Cargo.toml .cargo/config.toml \
        rust-toolchain.toml README.md crates/ikeloud-safety/src/lib.rs
    find crates -mindepth 2 -maxdepth 2 -name Cargo.toml
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

baseline="$(cd "$REPO" && "$KEY")"
failures=0

# check NAME same|different COMMAND...  (COMMAND runs in a subshell in $REPO)
check() {
    local name="$1" expect="$2"
    shift 2
    local snapshot="$WORK/snapshot"
    rm -rf "$snapshot" && cp -r "$REPO" "$snapshot"
    local key
    key="$(cd "$REPO" && "$@" && "$KEY")" || key="error"
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
check "our source code" same edit crates/ikeloud-safety/src/lib.rs
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
    PKG_CONFIG_PATH LIBSQLITE3_FLAGS SQLITE3_LIB_DIR ImageVersion; do
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

if [[ $failures -gt 0 ]]; then
    echo "$failures cache key check(s) failed" >&2
    exit 1
fi
echo "cache key covers all inputs"
