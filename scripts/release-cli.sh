#!/bin/sh
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/target/release-cli"
mkdir -p "$out"
cd "$root"
for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  built="$(scripts/build-cli.sh nanocode-cli nanocode "$triple")"
  cp "$built" "$out/nanocode-$triple"
  python="$(dirname "$built")/nanocode-python"
  cp "$python" "$out/nanocode-python-$(shasum -a 256 "$python" | cut -d' ' -f1)"
  cp "$(scripts/build-cli.sh nanocode-cua-driver nanocode-cua-driver "$triple")" "$out/nanocode-cua-driver-$triple"
done
for arch in arm64 amd64; do
  case $arch in arm64) triple=aarch64-unknown-linux-gnu ;; amd64) triple=x86_64-unknown-linux-gnu ;; esac
  docker run --rm --platform "linux/$arch" -v "$root:/src" -v "nanocode-cargo-$arch:/usr/local/cargo/registry" -w /src \
    -e CARGO_TARGET_DIR="/src/target/linux-$arch" -e RUSTC_WRAPPER= -e RUSTC_WORKSPACE_WRAPPER= -e CARGO_INCREMENTAL=0 \
    rust:1.97-bullseye sh -c 'sed -i /debian-security/d /etc/apt/sources.list && wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor > /usr/share/keyrings/llvm.gpg && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/bullseye/ llvm-toolchain-bullseye-21 main" > /etc/apt/sources.list.d/llvm.list && apt-get update && apt-get install -y --no-install-recommends bolt-21 llvm-21 && LLVM_BOLT=/usr/lib/llvm-21/bin/llvm-bolt scripts/build-cli.sh nanocode-cli nanocode'
  cp "target/linux-$arch/cli/$triple/nanocode" "$out/nanocode-$triple"
  python="target/linux-$arch/cli/$triple/nanocode-python"
  cp "$python" "$out/nanocode-python-$(shasum -a 256 "$python" | cut -d' ' -f1)"
done
cd "$out"
rm -f ./*.gz ./*.xz SHA256SUMS
for binary in nanocode-*; do
  case $binary in nanocode-python-*) ;; *) gzip -9 -c "$binary" > "$binary.gz" ;; esac
  case $binary in nanocode-cua-driver-*) ;; *) xz -9e -c "$binary" > "$binary.xz" ;; esac
done
shasum -a 256 ./*.gz ./*.xz | sed 's# \./# #' > SHA256SUMS
if [ "${1:-}" = "--publish" ]; then
  git -C "$root" push -f origin "$(git -C "$root" rev-parse HEAD):refs/tags/nanocode-cli"
  cp "$root/install" install.sh
  gh release upload nanocode-cli --repo nanocodexyz/nanocode --clobber ./*.gz ./*.xz SHA256SUMS install.sh
fi
