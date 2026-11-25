#!/usr/bin/env bash
# Puts the PDFium library the PDF reader renders with, and its licenses,
# in a folder:
#   packaging/fetch-pdfium.sh <win|linux|mac-arm64|mac-x64> <folder>
# Pinned and checked, like nfpm. The version is the one the pdfium-render
# crate binds to: its `pdfium_latest` feature names it, so move the two
# together.
set -euo pipefail

platform=$1
archive=$platform-x64
folder=$2

version=7881
case "$platform" in
  win)
    sha256=73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08
    library=bin/pdfium.dll
    ;;
  linux)
    sha256=1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d
    library=lib/libpdfium.so
    ;;
  mac-arm64)
    sha256=52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40
    library=lib/libpdfium.dylib
    archive=mac-arm64
    ;;
  mac-x64)
    sha256=6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b
    library=lib/libpdfium.dylib
    archive=mac-x64
    ;;
  *)
    echo "fetch-pdfium: no such platform: $platform" >&2
    exit 2
    ;;
esac

work=$(mktemp -d)
curl -fsSL -o "$work/pdfium.tgz" "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/${version}/pdfium-${archive}.tgz"
if command -v sha256sum > /dev/null; then
  echo "$sha256  $work/pdfium.tgz" | sha256sum -c -
else
  echo "$sha256  $work/pdfium.tgz" | shasum -a 256 -c -
fi
tar -xzf "$work/pdfium.tgz" -C "$work"

mkdir -p "$folder/licenses/pdfium"
cp "$work/$library" "$folder/"
cp "$work"/licenses/* "$folder/licenses/pdfium/"
cp "$work/LICENSE" "$folder/licenses/pdfium/pdfium-binaries.txt"
