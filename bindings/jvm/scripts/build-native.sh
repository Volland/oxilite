#!/usr/bin/env bash
# Builds the oxilite-jvm cdylib for the host platform and copies it into
# java/src/main/resources/native/<os>-<arch>/, where NativeLoader expects to find it.
#
# Usage: scripts/build-native.sh [--release]

set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(cd ../.. && pwd)"

PROFILE=dev
PROFILE_DIR=debug
CARGO_FLAGS=()
if [[ "${1:-}" == "--release" ]]; then
    PROFILE=release
    PROFILE_DIR=release
    CARGO_FLAGS+=(--release)
fi

case "$(uname -s)" in
    Darwin) OS=darwin; EXT=dylib; LIB_PREFIX=lib ;;
    Linux) OS=linux; EXT=so; LIB_PREFIX=lib ;;
    MINGW*|MSYS*|CYGWIN*) OS=windows; EXT=dll; LIB_PREFIX= ;;
    *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    arm64|aarch64) ARCH=aarch64 ;;
    x86_64|amd64) ARCH=x86_64 ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

echo "Building oxilite-jvm ($PROFILE) for $OS-$ARCH..."
(cd "$ROOT" && cargo build -p oxilite-jvm ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"})

SRC="$ROOT/target/$PROFILE_DIR/${LIB_PREFIX}oxilite_jvm.$EXT"
DEST_DIR="java/src/main/resources/native/$OS-$ARCH"
mkdir -p "$DEST_DIR"
cp "$SRC" "$DEST_DIR/${LIB_PREFIX}oxilite_jvm.$EXT"
echo "Copied $SRC -> $DEST_DIR/${LIB_PREFIX}oxilite_jvm.$EXT"
