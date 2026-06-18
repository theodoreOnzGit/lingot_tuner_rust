#!/usr/bin/env bash
#
# Build a standalone Linux AppImage for the lingot-tuner GUI.
#
# Produces dist/lingot-tuner-<arch>.AppImage, a self-contained, portable
# executable that bundles the non-driver shared libraries the tuner needs
# (ALSA, X11/Wayland, etc.). Requires network access the first time, to fetch
# linuxdeploy.
#
# Usage:  ./scripts/build-appimage.sh
#
set -euo pipefail

ARCH="${ARCH:-x86_64}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
TOOLS="$DIST/tools"
APPDIR="$DIST/AppDir"

# AppImages normally need FUSE; extract-and-run avoids that requirement so this
# works in containers/CI too.
export APPIMAGE_EXTRACT_AND_RUN=1

mkdir -p "$TOOLS"

# 1. Fetch linuxdeploy (gathers dependencies and builds the AppImage).
LD="$TOOLS/linuxdeploy-$ARCH.AppImage"
if [ ! -x "$LD" ]; then
    echo "==> downloading linuxdeploy"
    curl -fSL -o "$LD" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage"
    chmod +x "$LD"
fi

# 2. Build the release binary (the GUI binary requires the `gui` feature).
echo "==> building lingot-tuner (release, gui)"
cargo build --release --bin lingot-tuner --features gui --manifest-path "$ROOT/Cargo.toml"

# 3. Assemble a fresh AppDir with the binary.
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
cp "$ROOT/target/release/lingot-tuner" "$APPDIR/usr/bin/"

# 4. Bundle dependencies and emit the AppImage into dist/.
echo "==> packaging AppImage"
cd "$DIST"
OUTPUT="lingot-tuner-$ARCH.AppImage" \
"$LD" --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/lingot-tuner" \
    --desktop-file "$ROOT/packaging/lingot-tuner.desktop" \
    --icon-file "$ROOT/packaging/lingot-tuner.svg" \
    --output appimage

echo "==> done: $DIST/lingot-tuner-$ARCH.AppImage"
