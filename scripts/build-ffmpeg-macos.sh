#!/usr/bin/env bash
# Builds the FFmpeg that ships inside the Mac app, from official source, so it can be shared:
#   - no --enable-nonfree and no --enable-gpl: the result is LGPL 2.1+ (redistributable)
#   - H.264 encoding via Apple's VideoToolbox (part of macOS), AAC via FFmpeg's own encoder,
#     MP3 via LAME (LGPL), AV1 decoding via dav1d (BSD); everything else is FFmpeg's own code
#   - static: depends only on macOS system libraries/frameworks
#
# Usage: scripts/build-ffmpeg-macos.sh <aarch64-apple-darwin|x86_64-apple-darwin> <output-binary> <licenses-dir>
# Run by `pnpm engines` on macOS; needs Xcode command-line tools, make, meson, ninja, pkg-config.
# Every source archive is checked against a pinned SHA-256. Only the work folder is written to.
set -euo pipefail

FFMPEG_VERSION=9.0.2
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e # ffmpeg.org tarball, GPG-verified against the FFmpeg release key
LAME_VERSION=3.100
LAME_SHA256=ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e
DAV1D_VERSION=1.5.4
DAV1D_SHA256=686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5 # matches videolan's .sha256
MACOS_MIN=11.0

target=$1
output=$2
licenses=$3
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "unsupported target $target" >&2; exit 1 ;;
esac

work=$(mktemp -d "${TMPDIR:-/tmp}/ffmpeg-build.XXXXXX")
trap 'rm -rf "$work"' EXIT
prefix=$work/prefix
jobs=$(sysctl -n hw.ncpu)
cflags="-arch $arch -mmacosx-version-min=$MACOS_MIN -O2"
export PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig # only our own libraries, never Homebrew's
export PKG_CONFIG_PATH=$PKG_CONFIG_LIBDIR

# Runs a build step quietly; on failure shows the end of its output.
step() {
  "$@" >"$work/step.log" 2>&1 || { echo "build step failed: $*" >&2; tail -n 40 "$work/step.log" >&2; exit 1; }
}

fetch() { # url sha256 file
  echo "  fetching $1"
  curl -fsSL --retry 3 --max-time 1800 -o "$work/$3" "$1"
  echo "$2  $work/$3" | shasum -a 256 -c --quiet - || { echo "checksum mismatch for $1" >&2; exit 1; }
}

fetch "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" "$FFMPEG_SHA256" ffmpeg.tar.xz
fetch "https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz" "$LAME_SHA256" lame.tar.gz
fetch "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz" "$DAV1D_SHA256" dav1d.tar.xz
for a in ffmpeg.tar.xz lame.tar.gz dav1d.tar.xz; do tar -xf "$work/$a" -C "$work"; done

echo "  building LAME $LAME_VERSION"
(
  cd "$work/lame-$LAME_VERSION"
  # LAME 3.100 lists a symbol that no longer exists; the static library doesn't need the list.
  sed -i '' '/lame_init_old/d' include/libmp3lame.sym
  step ./configure --prefix="$prefix" --host="$target" --enable-static --disable-shared \
    --disable-frontend --disable-decoder CFLAGS="$cflags"
  step make -j"$jobs"
  step make install
)

echo "  building dav1d $DAV1D_VERSION"
(
  cd "$work/dav1d-$DAV1D_VERSION"
  # Always state the target CPU: meson may itself be an Intel build running under Rosetta,
  # which would otherwise compile dav1d for the wrong architecture.
  printf "[binaries]\nc = ['clang', '-arch', '%s']\n[host_machine]\nsystem = 'darwin'\ncpu_family = '%s'\ncpu = '%s'\nendian = 'little'\n" \
    "$arch" "$( [ "$arch" = arm64 ] && echo aarch64 || echo x86_64 )" "$arch" > cross.ini
  asm=true
  [ "$arch" = x86_64 ] && ! command -v nasm >/dev/null && asm=false # x86 assembly needs nasm
  step meson setup build --cross-file cross.ini --prefix="$prefix" --libdir=lib --buildtype=release --default-library=static \
    -Denable_tools=false -Denable_tests=false -Denable_asm=$asm -Dc_args="-mmacosx-version-min=$MACOS_MIN"
  step ninja -C build install
)

echo "  building FFmpeg $FFMPEG_VERSION (LGPL)"
(
  cd "$work/ffmpeg-$FFMPEG_VERSION"
  extra=() # ${extra[@]+"${extra[@]}"} below: safe empty expansion under `set -u` in macOS's bash 3.2
  [ "$arch" != "$(uname -m)" ] && extra+=(--enable-cross-compile)
  [ "$arch" = x86_64 ] && ! command -v nasm >/dev/null && extra+=(--disable-x86asm)
  # --disable-autodetect: never pick up libraries installed on the build machine (e.g. Homebrew);
  # only macOS's own libraries and the ones built above are used.
  step ./configure --prefix="$prefix" --arch="$arch" --target-os=darwin --cc="clang -arch $arch" ${extra[@]+"${extra[@]}"} \
    --pkg-config-flags=--static \
    --extra-cflags="-I$prefix/include -mmacosx-version-min=$MACOS_MIN" \
    --extra-ldflags="-L$prefix/lib -mmacosx-version-min=$MACOS_MIN" \
    --disable-autodetect --enable-videotoolbox --enable-audiotoolbox --enable-zlib --enable-bzlib --disable-iconv \
    --enable-libmp3lame --enable-libdav1d \
    --disable-ffplay --disable-ffprobe --disable-doc --disable-debug --disable-shared --enable-static
  # The whole point: neither GPL nor nonfree may be switched on.
  grep -q "#define CONFIG_NONFREE 0" config.h && grep -q "#define CONFIG_GPL 0" config.h ||
    { echo "FFmpeg is not configured as LGPL-only" >&2; exit 1; }
  step make -j"$jobs"
  cp ffmpeg "$work/ffmpeg-out"
  mkdir -p "$licenses"
  cp COPYING.LGPLv2.1 "$licenses/FFmpeg-LICENSE.txt"
  cp "$work/lame-$LAME_VERSION/COPYING" "$licenses/LAME-LICENSE.txt"
  cp "$work/dav1d-$DAV1D_VERSION/COPYING" "$licenses/dav1d-LICENSE.txt"
)

# Only macOS system libraries may be linked; anything else wouldn't exist on other Macs.
if otool -L "$work/ffmpeg-out" | tail -n +2 | grep -vE "^\s+(/usr/lib/|/System/Library/)"; then
  echo "ffmpeg links non-system libraries (above)" >&2
  exit 1
fi
mv "$work/ffmpeg-out" "$output"
chmod 755 "$output"
echo "  FFmpeg $FFMPEG_VERSION built for $target (LGPL, $(du -h "$output" | cut -f1))"
