#!/usr/bin/env bash
# Regenerate the app icons from packaging/icon/winewrap.svg (macOS only):
#   packaging/macos/AppIcon.icns      the .app bundle icon
#   packaging/icon/winewrap.png       512 px, for the README
#   crates/winewrap-gui/assets/icon.png  256 px window icon for unbundled runs
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
svg=packaging/icon/winewrap.svg
render() { swift scripts/render-icon.swift "$svg" "$1" "$2"; }

iconset="$(mktemp -d)/AppIcon.iconset"
trap 'rm -rf "$(dirname "$iconset")"' EXIT
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    render "$iconset/icon_${size}x${size}.png" "$size"
    render "$iconset/icon_${size}x${size}@2x.png" $((size * 2))
done
iconutil -c icns "$iconset" -o packaging/macos/AppIcon.icns
render packaging/icon/winewrap.png 512
render crates/winewrap-gui/assets/icon.png 256
