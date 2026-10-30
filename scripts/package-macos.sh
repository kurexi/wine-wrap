#!/usr/bin/env bash
# Build WineWrap for macOS and package it for distribution.
#
# Produces, in ./dist:
#   WineWrap-<version>-macos-<arch>.dmg      the WineWrap.app desktop app
#   winewrap-<version>-macos-<arch>.tar.gz   the `winewrap` command-line tool
#
# Usage: scripts/package-macos.sh [universal|native]   (default: universal)
#
# Optional environment:
#   VERSION                  version string (default: from Cargo.toml)
#   BUILD_NUMBER             CFBundleVersion (default: VERSION)
#   MACOS_SIGNING_IDENTITY   "Developer ID Application: …" identity in the
#                            keychain; ad-hoc signing is used when unset
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                            notarize and staple the .dmg when all are set
#                            (requires MACOS_SIGNING_IDENTITY)
set -euo pipefail

mode="${1:-universal}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: macOS packaging must run on macOS" >&2
    exit 1
fi

version="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)}"
build_number="${BUILD_NUMBER:-$version}"
export MACOSX_DEPLOYMENT_TARGET=13.0

case "$mode" in
    universal)
        arch_label=universal
        targets=(aarch64-apple-darwin x86_64-apple-darwin)
        ;;
    native)
        arch_label="$(uname -m)"
        [[ "$arch_label" == arm64 ]] && arch_label=aarch64
        targets=("${arch_label}-apple-darwin")
        ;;
    *)
        echo "usage: $0 [universal|native]" >&2
        exit 2
        ;;
esac

echo "==> Building WineWrap $version ($arch_label)"
rustup target add "${targets[@]}" >/dev/null
for target in "${targets[@]}"; do
    cargo build --release --locked --target "$target" -p winewrap-cli -p winewrap-gui
done

dist="$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$dist" "$stage/bin"

# Merge per-architecture binaries into one (a no-op copy for native builds).
# Binary names must differ by more than case: macOS file systems are
# case-insensitive, so `winewrap` and `WineWrap` would be the same file.
for bin in winewrap winewrap-gui; do
    inputs=()
    for target in "${targets[@]}"; do
        inputs+=("target/$target/release/$bin")
    done
    lipo -create "${inputs[@]}" -output "$stage/bin/$bin"
done
lipo -info "$stage/bin/winewrap" "$stage/bin/winewrap-gui"

identity="${MACOS_SIGNING_IDENTITY:--}"
sign() {
    if [[ "$identity" == "-" ]]; then
        codesign --force --sign - "$@"
    else
        codesign --force --sign "$identity" --options runtime --timestamp "$@"
    fi
}

echo "==> Assembling WineWrap.app"
app="$stage/dmg/WineWrap.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources"
cp "$stage/bin/winewrap-gui" "$app/Contents/MacOS/WineWrap"
# Ship the CLI inside the app too, so users of the .dmg can symlink it. It
# lives in Helpers/ because MacOS/winewrap would collide with MacOS/WineWrap.
cp "$stage/bin/winewrap" "$app/Contents/Helpers/winewrap"
sed -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build_number/" \
    packaging/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sign "$app/Contents/Helpers/winewrap"
sign "$app"
codesign --verify --deep --strict "$app"

echo "==> Creating disk image"
ln -s /Applications "$stage/dmg/Applications"
dmg="$dist/WineWrap-$version-macos-$arch_label.dmg"
rm -f "$dmg"
hdiutil create -volname "WineWrap $version" -srcfolder "$stage/dmg" \
    -fs HFS+ -format UDZO -ov "$dmg"
if [[ "$identity" != "-" ]]; then
    sign "$dmg"
fi

if [[ "$identity" != "-" && -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
    echo "==> Notarizing"
    xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$dmg"
fi

echo "==> Packaging CLI"
sign "$stage/bin/winewrap"
cli_dir="$stage/winewrap-$version-macos-$arch_label"
mkdir -p "$cli_dir"
cp "$stage/bin/winewrap" README.md LICENSE "$cli_dir/"
tar -C "$stage" -czf "$dist/winewrap-$version-macos-$arch_label.tar.gz" "$(basename "$cli_dir")"

echo "==> Done"
ls -lh "$dist"
