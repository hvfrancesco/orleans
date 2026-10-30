#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
target="$(rustc -vV | sed -n 's/^host: //p')"
dist="$root/dist"
appdir="$dist/orleans-$version.AppDir"
artifact="$dist/orleans-$version-$target.AppImage"

case "$target" in
    x86_64-*) appimage_arch=x86_64 ;;
    aarch64-*) appimage_arch=aarch64 ;;
    *) appimage_arch="$(uname -m)" ;;
esac

for tool in linuxdeploy appimagetool; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done

cargo build --release --locked

rm -rf "$appdir"
linuxdeploy \
    --appdir "$appdir" \
    --executable target/release/orleans \
    --desktop-file packaging/orleans.desktop \
    --icon-file packaging/orleans.png \
    --custom-apprun packaging/AppRun \
    --exclude-library "libudev.so*"

ARCH="$appimage_arch" appimagetool --no-appstream "$appdir" "$artifact"
(
    cd "$dist"
    sha256sum "$(basename "$artifact")" > "$(basename "$artifact").sha256"
)

echo "created $artifact"
echo "created $artifact.sha256"
