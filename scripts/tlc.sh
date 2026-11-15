#!/usr/bin/env bash
# Runs the TLC model checker on a TLA+ model in the tla/ directory.
#
# Usage: scripts/tlc.sh <Model.cfg> [more TLC options]
# Example: scripts/tlc.sh SyncEtag.cfg
# The model comes from a "\* SPEC: File.tla" line in the config, otherwise Sync.tla.
#
# Java and tla2tools are downloaded on first run into .tools/ (not in git),
# in pinned versions with a SHA-256 check. No system Java is needed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLS="$ROOT/.tools"

TLA_VERSION="1.7.4"
TLA_URL="https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
# GitHub lists no checksum for this release; recorded on first download 2026-11-14.
TLA_SHA256="936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"

JRE_VERSION="21.0.13+9"
JRE_URL="https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz"
# Checksum from the Adoptium API.
JRE_SHA256="a535a1df9c6284d9618872984e026ae0352d1f7e9e0605425c0e6114f47351dd"

fetch() { # url destination sha256
    local url="$1" dest="$2" sum="$3"
    if [[ ! -f "$dest" ]]; then
        curl --proto '=https' --tlsv1.2 -fsSL -o "$dest.part" "$url"
        mv "$dest.part" "$dest"
    fi
    if ! echo "$sum  $dest" | sha256sum -c --quiet -; then
        echo "error: checksum mismatch for $dest" >&2
        rm -f "$dest"
        exit 1
    fi
}

mkdir -p "$TOOLS"
JAR="$TOOLS/tla2tools-$TLA_VERSION.jar"
fetch "$TLA_URL" "$JAR" "$TLA_SHA256"

JAVA="$TOOLS/jre-$JRE_VERSION/bin/java"
if [[ ! -x "$JAVA" ]]; then
    fetch "$JRE_URL" "$TOOLS/jre.tar.gz" "$JRE_SHA256"
    mkdir -p "$TOOLS/jre-$JRE_VERSION"
    tar -xzf "$TOOLS/jre.tar.gz" -C "$TOOLS/jre-$JRE_VERSION" --strip-components=1
fi

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <Model.cfg> [TLC options]" >&2
    exit 2
fi
CFG="$1"
shift

cd "$ROOT/tla"
SPEC="$(sed -n 's/^\\\* SPEC: *\([A-Za-z0-9_]*\.tla\).*/\1/p' "$CFG" | head -n1)"
SPEC="${SPEC:-Sync.tla}"
exec "$JAVA" -XX:+UseParallelGC -cp "$JAR" tlc2.TLC \
    -config "$CFG" -workers auto -deadlock \
    -metadir "$TOOLS/states/$(basename "$CFG" .cfg)-$$" "$@" "$SPEC"
