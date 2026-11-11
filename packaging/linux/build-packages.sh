#!/usr/bin/env bash
# Builds fenix_<version>_amd64.deb and fenix-<version>-1.x86_64.rpm from a
# release's unpacked Linux files, each with a .sha256 beside it:
#   packaging/linux/build-packages.sh <version> <unpacked dir> <out dir> [<name>]
# Run from the repository's root. nfpm is fetched, pinned and checked.
#
# A pre-release version (the nightly's 1.6.0-nightly.20261112) becomes
# 1.6.0~nightly.20261112 in both packages -- nfpm's doing -- so dpkg and
# rpm sort it after 1.5.x and before the 1.6.0 release, and either
# upgrades the other in place: it's the same package, `fenix`. With
# <name>, the files are called <name>_amd64.deb and <name>.x86_64.rpm
# instead (the nightly's fenix-nightly, the same every night), since a
# ~ in a file name isn't kept by GitHub's downloads.
set -euo pipefail

version=$1
src=$(cd "$2" && pwd)
out=$3
name=${4-}
mkdir -p "$out"

nfpm_version=2.48.0
nfpm_sha256=6d7f9a28b2f397934403950fcbba83f598b08d02edbe79a0815b3b4897341750
tool=$(mktemp -d)
curl -fsSL -o "$tool/nfpm.tar.gz" "https://github.com/goreleaser/nfpm/releases/download/v${nfpm_version}/nfpm_${nfpm_version}_Linux_x86_64.tar.gz"
echo "$nfpm_sha256  $tool/nfpm.tar.gz" | sha256sum -c -
tar -xzf "$tool/nfpm.tar.gz" -C "$tool" nfpm

# nfpm reads ${VERSION} itself, but not ${SRC} in a file's path: both are
# filled in here.
sed -e "s|\${SRC}|$src|g" -e "s|\${VERSION}|$version|g" packaging/linux/nfpm.yaml > "$tool/nfpm.yaml"
for packager in deb rpm; do
  "$tool/nfpm" package --config "$tool/nfpm.yaml" --packager "$packager" --target "$out/"
done
cd "$out"
if [ -n "$name" ]; then
  mv ./*.deb "${name}_amd64.deb"
  mv ./*.rpm "${name}.x86_64.rpm"
fi
for package in *.deb *.rpm; do
  sha256sum "$package" > "$package.sha256"
done
ls -l
