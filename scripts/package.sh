#!/bin/sh
# Stage a release folder; native macOS builds also produce an app and DMG.
# The build is locked to Cargo.lock. SOURCE_DATE_EPOCH and remapped paths keep the
# binary from embedding this checkout's clock and home directory.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"
TARGET=${1:-}
if [ ! -f Cargo.lock ]; then
    echo "Cargo.lock is required" >&2
    exit 1
fi
if [ -n "$TARGET" ]; then
    if ! rustup target list --installed | grep -qx "$TARGET"; then
        echo "Install the target first: rustup target add $TARGET" >&2
        exit 1
    fi
    TRIPLE=$TARGET
else
    TRIPLE=$(rustc -vV | awk '/^host:/ { print $2 }')
fi
if git rev-parse HEAD >/dev/null 2>&1; then
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
else
    SOURCE_DATE_EPOCH=0
fi
export SOURCE_DATE_EPOCH
CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$ROOT=/freeshow --remap-path-prefix=$CARGO_HOME=/cargo"
if command -v brew >/dev/null 2>&1; then
    export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi
if [ -n "$TARGET" ]; then
    cargo build --locked --release -p freeshow --target "$TARGET"
    BIN_DIR="$ROOT/target/$TARGET/release"
else
    cargo build --locked --release -p freeshow
    BIN_DIR="$ROOT/target/release"
fi
NAME=freeshow
case $TRIPLE in
    *windows*) NAME=freeshow.exe ;;
esac
STAGE="$ROOT/dist/freeshow-$TRIPLE"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$BIN_DIR/$NAME" "$STAGE/"
cp LICENSE README.md "$STAGE/"
if command -v shasum >/dev/null 2>&1; then
    LOCK_HASH=$(shasum -a 256 Cargo.lock | awk '{ print $1 }')
else
    LOCK_HASH=$(sha256sum Cargo.lock | awk '{ print $1 }')
fi
{
    echo "name freeshow-rust"
    echo "version 0.1.0"
    echo "target $TRIPLE"
    echo "source_date_epoch $SOURCE_DATE_EPOCH"
    echo "cargo_lock_sha256 $LOCK_HASH"
    rustc -vV
} > "$STAGE/BUILD.txt"
case $TRIPLE in
    *apple-darwin*)
        if [ "$(uname -s)" != Darwin ]; then
            echo "macOS app packaging needs a native macOS host" >&2
            exit 1
        fi
        python3 scripts/package-macos.py --binary "$STAGE/$NAME" --output "$STAGE" --dmg
        ;;
esac
echo "$STAGE"
