#!/bin/sh
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/target/release-cli"
mkdir -p "$out"
cd "$root"
for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  cp "$(scripts/build-cli.sh nanocode-cli nanocode "$triple")" "$out/nanocode-$triple"
  cp "$(scripts/build-cli.sh nanocode-cua-driver nanocode-cua-driver "$triple")" "$out/nanocode-cua-driver-$triple"
done
for arch in arm64 amd64; do
  case $arch in arm64) triple=aarch64-unknown-linux-gnu ;; amd64) triple=x86_64-unknown-linux-gnu ;; esac
  docker run --rm --platform "linux/$arch" -v "$root:/src" -v "nanocode-cargo-$arch:/usr/local/cargo/registry" -w /src \
    -e CARGO_TARGET_DIR="/src/target/linux-$arch" -e RUSTC_WRAPPER= -e RUSTC_WORKSPACE_WRAPPER= -e CARGO_INCREMENTAL=0 \
    rust:1.97-bullseye sh -c 'apt-get update && apt-get install -y lsb-release software-properties-common gnupg python3 && wget -qO- https://apt.llvm.org/llvm.sh | bash -s 21 && apt-get install -y bolt-21 && LLVM_BOLT=/usr/lib/llvm-21/bin/llvm-bolt scripts/build-cli.sh nanocode-cli nanocode'
  cp "target/linux-$arch/cli/$triple/nanocode" "$out/nanocode-$triple"
done
cd "$out"
rm -f ./*.gz SHA256SUMS
for binary in nanocode-*; do gzip -9 -c "$binary" > "$binary.gz"; done
shasum -a 256 ./*.gz | sed 's# \./# #' > SHA256SUMS
if [ "${1:-}" = "--publish" ]; then
  git -C "$root" push -f origin "$(git -C "$root" rev-parse HEAD):refs/tags/nanocode-cli"
  cp "$root/install" install.sh
  gh release upload nanocode-cli --repo nanocodexyz/nanocode --clobber ./*.gz SHA256SUMS install.sh
fi
