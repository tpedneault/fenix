#!/usr/bin/env bash
# Makes Fenix.app, and a disk image of it, from a finished build:
#   packaging/macos/build-app.sh <version> <build folder> <out dir> <name>
# <build folder> holds fenix and, when they're there, fenix-pdfium,
# libpdfium.dylib, remote/ and licenses/ (target/<...>/release). The image
# is <out dir>/<name>.dmg. Run from the repository's root, on a Mac.
#
# Signed with MACOS_SIGN_IDENTITY (a "Developer ID Application" in the
# keychain, with the hardened runtime a notarisation needs) when it's set;
# else ad hoc, which is all Apple silicon needs to run it but leaves
# Gatekeeper to ask, the first time, about a download.
set -euo pipefail

version=$1
build=$2
out=$3
name=$4

app="$out/Fenix.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# 1.6.0-nightly.20261112: Apple's versions are numbers alone.
short=${version%%-*}
sed -e "s/@SHORT_VERSION@/$short/" -e "s/@MINIMUM@/${MACOSX_DEPLOYMENT_TARGET:-11.0}/" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint -s "$app/Contents/Info.plist"

# Programs and the library they load in MacOS; everything else in
# Resources, where a signature covers it as data.
cp "$build/fenix" "$app/Contents/MacOS/"
for program in fenix-pdfium libpdfium.dylib; do
  if [ -f "$build/$program" ]; then cp "$build/$program" "$app/Contents/MacOS/"; fi
done
for folder in remote licenses; do
  if [ -d "$build/$folder" ]; then cp -R "$build/$folder" "$app/Contents/Resources/"; fi
done
cp LICENSE-MIT LICENSE-APACHE THIRD-PARTY.md "$app/Contents/Resources/"

# The icon, in every size an .icns has, drawn from the brand's geometry.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cargo run --quiet --locked -p fenix-brand --example write-iconset --features ico -- "$work/Fenix.iconset"
iconutil -c icns "$work/Fenix.iconset" -o "$app/Contents/Resources/Fenix.icns"

identity=${MACOS_SIGN_IDENTITY:--}
sign=(codesign --force --sign "$identity")
if [ "$identity" != - ]; then sign+=(--options runtime --timestamp --entitlements packaging/macos/entitlements.plist); fi
# Inside out: what the application holds first.
for inner in libpdfium.dylib fenix-pdfium; do
  if [ -f "$app/Contents/MacOS/$inner" ]; then "${sign[@]}" "$app/Contents/MacOS/$inner"; fi
done
"${sign[@]}" "$app"
codesign --verify --deep --strict "$app"

mkdir "$work/image"
cp -R "$app" "$work/image/"
ln -s /Applications "$work/image/Applications"
hdiutil create -quiet -ov -volname Fenix -srcfolder "$work/image" -format UDZO "$out/$name.dmg"
echo "$out/$name.dmg"
