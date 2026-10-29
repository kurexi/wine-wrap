#!/bin/bash
# Build an x86_64 macOS Wine from CodeWeavers' LGPL CrossOver source, with
# WineWrap's patches, packaged like Gcenx's builds:
#   "Wine WineWrap.app/Contents/Resources/wine/{bin,lib,share}"
# All dependencies are built from source into the work directory; nothing is
# installed system-wide and no package manager is needed. Requires Xcode and
# Rosetta on Apple Silicon.
#
# Usage: scripts/build-wine-macos.sh [work-dir]   (default: target/wine-build)
# Output: <work-dir>/wine-winewrap-<cx-version>-osx64.tar.xz
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$(mkdir -p "${1:-$ROOT/target/wine-build}" && cd "${1:-$ROOT/target/wine-build}" && pwd)"
JOBS="$(sysctl -n hw.ncpu)"

CX_VERSION=26.3.0
LLVM_MINGW=llvm-mingw-20261020-ucrt-macos-universal
DOWNLOADS=(
  "https://media.codeweavers.com/pub/crossover/source/crossover-sources-$CX_VERSION.tar.gz ac99c8ca4b3848f3e81784135f023df266b61c2345726ea55a50b3e030dd6872"
  "https://ftp.gnu.org/gnu/bison/bison-3.8.2.tar.xz 9bba0214ccf7f1079c5d59210045227bcf619519840ebfa80cd3849cff5a5bf2"
  "https://ftp.gnu.org/gnu/nettle/nettle-3.10.tar.gz b4c518adb174e484cb4acea54118f02380c7133771e7e9beb98a0787194ee47c"
  "https://download.savannah.gnu.org/releases/freetype/freetype-2.13.3.tar.xz 0550350666d427c74daeb85d5ac7bb353acba5f76956395995311a9c6f063289"
  "https://github.com/mstorsjo/llvm-mingw/releases/download/20261020/$LLVM_MINGW.tar.xz e9c13e4ede38e715003ffc674d4d00a1088df8235439d1f0c16188ff0704c17e"
  "https://github.com/KhronosGroup/MoltenVK/releases/download/v1.4.2/MoltenVK-macos.tar f95765a6229cb7b915990a2890ce12ebe36a730b021545d3d52ae69ce4c4024e"
  # Wine Mono and Gecko, the versions dlls/appwiz.cpl/addons.c asks for, so
  # new prefixes don't stop at the "install Mono?" dialog.
  "https://dl.winehq.org/wine/wine-mono/10.4.1/wine-mono-10.4.1-x86.tar.xz a16606ef0724202e6a6848ece6e0cbba64d11e2f11aefe744af1d93c6d9f99bb"
  "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86.tar.xz 2cfc8d5c948602e21eff8a78613e1826f2d033df9672cace87fed56e8310afb6"
  "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86_64.tar.xz fd88fc7e537d058d7a8abf0c1ebc90c574892a466de86706a26d254710a82814"
)

DL="$W/downloads" SRC="$W/src" TOOLS="$W/tools" DEPS="$W/deps" OBJ="$W/obj" INSTALL="$W/install"
mkdir -p "$DL" "$SRC" "$TOOLS" "$OBJ"
log() { echo "==> $*"; }

log "Downloading sources"
for entry in "${DOWNLOADS[@]}"; do
  read -r url sha <<<"$entry"
  f="$DL/$(basename "$url")"
  if [[ ! -f "$f" ]]; then
    curl -sSfL -o "$f.part" "$url"
    mv "$f.part" "$f"
  fi
  echo "$sha  $f" | shasum -a 256 -c --quiet
done

log "Unpacking"
rm -rf "$SRC" "$OBJ" "$INSTALL" "$DEPS"
mkdir -p "$SRC" "$OBJ"
tar xzf "$DL/crossover-sources-$CX_VERSION.tar.gz" -C "$SRC" sources/wine sources/gnutls
tar xzf "$DL/nettle-3.10.tar.gz" -C "$SRC"
tar xJf "$DL/freetype-2.13.3.tar.xz" -C "$SRC"
[[ -d "$TOOLS/$LLVM_MINGW" ]] || tar xJf "$DL/$LLVM_MINGW.tar.xz" -C "$TOOLS"
[[ -d "$TOOLS/moltenvk" ]] || { mkdir -p "$TOOLS/moltenvk" && tar xf "$DL/MoltenVK-macos.tar" -C "$TOOLS/moltenvk"; }
MVK="$TOOLS/moltenvk/MoltenVK/MoltenVK/dynamic/dylib/macOS"
for p in "$ROOT"/scripts/wine-patches/*.patch; do
  log "Applying $(basename "$p")"
  patch -s -p1 -d "$SRC/sources/wine" < "$p"
done

# macOS ships bison 2.3; Wine needs 3.x. It's a host tool, so build it natively.
if [[ ! -x "$TOOLS/host/bin/bison" ]]; then
  log "Building bison"
  tar xJf "$DL/bison-3.8.2.tar.xz" -C "$OBJ"
  (cd "$OBJ/bison-3.8.2" && ./configure --prefix="$TOOLS/host" >log 2>&1 &&
    make -j"$JOBS" >>log 2>&1 && make install >>log 2>&1)
fi

# Everything below runs as x86_64 (under Rosetta on Apple Silicon).
x86() { arch -x86_64 /usr/bin/env "$@"; }
export MACOSX_DEPLOYMENT_TARGET=13.0
# Apple's clang by full path: llvm-mingw's bin/ also has a `clang`.
export CC="/usr/bin/clang -arch x86_64" CXX="/usr/bin/clang++ -arch x86_64" OBJC="/usr/bin/clang -arch x86_64"
export CFLAGS="-O2" CROSSCFLAGS="-O2"
export CPPFLAGS="-I$DEPS/include" LDFLAGS="-L$DEPS/lib -L$MVK -Wl,-headerpad_max_install_names"
export PATH="$TOOLS/host/bin:$TOOLS/$LLVM_MINGW/bin:$PATH"

autotools() { # name srcdir configure-args...
  local name=$1 src=$2; shift 2
  log "Building $name"
  mkdir -p "$OBJ/$name"
  (cd "$OBJ/$name" &&
    x86 "$src/configure" --host=x86_64-apple-darwin --prefix="$DEPS" \
      --enable-shared --disable-static "$@" >log 2>&1 &&
    x86 make -j"$JOBS" >>log 2>&1 && make install >>log 2>&1) ||
    { tail -30 "$OBJ/$name/log"; exit 1; }
}
autotools gmp "$SRC/sources/gnutls/gmp" --with-pic
autotools nettle "$SRC/nettle-3.10" --libdir="$DEPS/lib" --disable-documentation --disable-openssl
# GnuTLS tests these with an unquoted `test`, so they must not contain spaces.
NETTLE_CFLAGS= NETTLE_LIBS=-lnettle HOGWEED_CFLAGS= HOGWEED_LIBS=-lhogweed GMP_CFLAGS= GMP_LIBS=-lgmp \
autotools gnutls "$SRC/sources/gnutls/gnutls" --disable-doc --disable-tests --disable-tools \
  --disable-cxx --disable-nls --disable-guile --disable-full-test-suite \
  --with-included-libtasn1 --with-included-unistring --without-p11-kit --without-idn \
  --without-brotli --without-zstd --without-zlib --without-tpm --without-tpm2 \
  --disable-hardware-acceleration
autotools freetype "$SRC/freetype-2.13.3" --without-png --without-harfbuzz \
  --without-brotli --without-bzip2 --with-zlib=yes

# Configure flags follow Gcenx's wine-stable Portfile. Wine 11's macOS OpenGL
# needs EGL, so OpenGL is off (as in Gcenx's builds).
log "Building Wine (this takes a while)"
mkdir -p "$OBJ/wine"
(cd "$OBJ/wine" &&
  FREETYPE_CFLAGS="-I$DEPS/include/freetype2" FREETYPE_LIBS="-lfreetype" \
  GNUTLS_CFLAGS="-I$DEPS/include" GNUTLS_LIBS="-lgnutls" \
  x86 "$SRC/sources/wine/configure" --prefix="$INSTALL" \
    --enable-archs=i386,x86_64 --with-mingw \
    --without-alsa --without-capi --with-coreaudio --with-cups --without-dbus \
    --without-ffmpeg --without-fontconfig --with-freetype --without-gettext \
    --without-gettextpo --without-gphoto --with-gnutls --without-gssapi \
    --without-gstreamer --without-inotify --without-krb5 --without-netapi \
    --with-opencl --without-opengl --without-oss --with-pcap --with-pthread \
    --without-pulse --without-sane --without-sdl --without-udev --with-unwind \
    --without-usb --without-v4l2 --with-vulkan --without-wayland --without-x \
    --disable-tests --disable-winemenubuilder >configure.log 2>&1 &&
  x86 make -j"$JOBS" >make.log 2>&1 && make install-lib >install.log 2>&1) ||
  { tail -30 "$OBJ/wine/"*.log; exit 1; }

log "Packaging"
APP="$W/pkg/Wine WineWrap.app"
P="$APP/Contents/Resources/wine"
rm -rf "$W/pkg"
mkdir -p "$P"
cp -R "$INSTALL/." "$P/"

# Bundled dylibs refer to each other via @loader_path; Wine's unix modules
# already have an @loader_path/../../ rpath, so they find them in lib/.
for l in libgmp.10.dylib libnettle.8.dylib libhogweed.6.dylib libgnutls.30.dylib libfreetype.6.dylib; do
  cp -L "$DEPS/lib/$l" "$P/lib/$l"
  chmod u+w "$P/lib/$l"
  install_name_tool -id "@rpath/$l" "$P/lib/$l"
  for dep in $(otool -L "$P/lib/$l" | awk 'NR>1{print $1}' | grep "^$DEPS/lib/" || true); do
    install_name_tool -change "$dep" "@loader_path/$(basename "$dep")" "$P/lib/$l"
  done
done
cp "$MVK/libMoltenVK.dylib" "$P/lib/"

mkdir -p "$P/share/wine/mono" "$P/share/wine/gecko"
tar xJf "$DL/wine-mono-10.4.1-x86.tar.xz" -C "$P/share/wine/mono"
tar xJf "$DL/wine-gecko-2.47.4-x86.tar.xz" -C "$P/share/wine/gecko"
tar xJf "$DL/wine-gecko-2.47.4-x86_64.tar.xz" -C "$P/share/wine/gecko"

# Rewriting load commands invalidates signatures; re-sign ad hoc.
find "$P/lib" -maxdepth 1 -name '*.dylib' -exec codesign -f -s - {} \; 2>/dev/null

OUT="$W/wine-winewrap-$CX_VERSION-osx64.tar.xz"
(cd "$W/pkg" && tar cJf "$OUT" "Wine WineWrap.app")
"$P/bin/wine" --version
log "Done: $OUT"
