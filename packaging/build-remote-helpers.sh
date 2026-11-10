#!/usr/bin/env bash
# Builds fenix-remote, the helper Fenix puts on a remote machine, for
# every kind of machine it supports, into <out dir>/remote/:
#   packaging/build-remote-helpers.sh <out dir>
# Run from the repository's root, on any system: the helper is plain Rust,
# built static against musl and linked by the linker Rust ships (rust-lld),
# so no cross toolchain is needed. It ships inside Fenix, beside the
# program (`remote/`), and is copied to a machine at the first connection.
set -euo pipefail

out=$1
mkdir -p "$out/remote"

# The toolchain rustup manages has the targets; a stand-alone Rust first
# on PATH wouldn't.
if command -v rustup > /dev/null; then
  cargo=$(rustup which cargo)
  RUSTC=$(rustup which rustc)
  export RUSTC
else
  cargo=cargo
fi

for arch in x86_64 aarch64; do
  target="$arch-unknown-linux-musl"
  if command -v rustup > /dev/null; then
    rustup target add "$target" > /dev/null
  fi
  var=$(echo "$target" | tr 'a-z-' 'A-Z_')
  export "CARGO_TARGET_${var}_LINKER=rust-lld"
  export "CARGO_TARGET_${var}_RUSTFLAGS=-C linker-flavor=ld.lld -C link-self-contained=yes -C strip=symbols"
  # A target folder of its own: these are built by another compiler than
  # the rest when a stand-alone Rust is first on PATH.
  "$cargo" build --release --locked -p fenix-remote --bin fenix-remote --target "$target" --target-dir target/remote-helpers
  cp "target/remote-helpers/$target/release/fenix-remote" "$out/remote/fenix-remote-linux-$arch"
done
ls -l "$out/remote"
