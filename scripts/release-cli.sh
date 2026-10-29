#!/bin/sh
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/target/release-cli"
mkdir -p "$out"
cd "$root"
cargo build --locked --release -p nanocode-cli --bin nanocode --bin nanocode-cua-driver
cp target/release/nanocode "$out/nanocode-aarch64-apple-darwin"
cp target/release/nanocode-cua-driver "$out/nanocode-cua-driver-aarch64-apple-darwin"
cargo build --locked --release --target x86_64-apple-darwin -p nanocode-cli --bin nanocode --bin nanocode-cua-driver
cp target/x86_64-apple-darwin/release/nanocode "$out/nanocode-x86_64-apple-darwin"
cp target/x86_64-apple-darwin/release/nanocode-cua-driver "$out/nanocode-cua-driver-x86_64-apple-darwin"
for arch in arm64 amd64; do
  case $arch in arm64) triple=aarch64-unknown-linux-gnu ;; amd64) triple=x86_64-unknown-linux-gnu ;; esac
  docker run --rm --platform "linux/$arch" -v "$root:/src" -v "nanocode-cargo-$arch:/usr/local/cargo/registry" -w /src \
    -e CARGO_TARGET_DIR="/src/target/linux-$arch" -e RUSTC_WRAPPER= -e RUSTC_WORKSPACE_WRAPPER= -e CARGO_INCREMENTAL=0 \
    rust:1.97-bullseye cargo build --locked --release -p nanocode-cli --bin nanocode
  cp "target/linux-$arch/release/nanocode" "$out/nanocode-$triple"
done
cd "$out"
rm -f ./*.gz SHA256SUMS
for binary in nanocode-*; do gzip -9 -c "$binary" > "$binary.gz"; done
shasum -a 256 ./*.gz | sed 's# \./# #' > SHA256SUMS
if [ "${1:-}" = "--publish" ]; then
  git -C "$root" push -f origin "$(git -C "$root" rev-parse HEAD):refs/tags/nanocode-cli"
  cp "$root/install" install.sh
  gh release upload nanocode-cli --repo gakonst/nanocode --clobber ./*.gz SHA256SUMS install.sh
fi
