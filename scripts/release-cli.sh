#!/bin/sh
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/target/release-cli"
mkdir -p "$out"
cd "$root"
for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --locked --profile dist --target "$triple" -p nanocode-cli --bin nanocode
  cargo build --locked --profile dist --target "$triple" -p nanocode-cua-driver
  cp "target/$triple/dist/nanocode" "$out/nanocode-$triple"
  cp "target/$triple/dist/nanocode-cua-driver" "$out/nanocode-cua-driver-$triple"
done
for arch in arm64 amd64; do
  case $arch in arm64) triple=aarch64-unknown-linux-gnu ;; amd64) triple=x86_64-unknown-linux-gnu ;; esac
  docker run --rm --platform "linux/$arch" -v "$root:/src" -v "nanocode-cargo-$arch:/usr/local/cargo/registry" -w /src \
    -e CARGO_TARGET_DIR="/src/target/linux-$arch" -e RUSTC_WRAPPER= -e RUSTC_WORKSPACE_WRAPPER= -e CARGO_INCREMENTAL=0 \
    rust:1.97-bullseye cargo build --locked --profile dist -p nanocode-cli --bin nanocode
  cp "target/linux-$arch/dist/nanocode" "$out/nanocode-$triple"
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
