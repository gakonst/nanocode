#!/bin/sh
set -eu
package=$1
bin=$2
root="$(cd "$(dirname "$0")/.." && pwd)"
target="${3:-$(rustc -vV | sed -n 's/^host: //p')}"
profile="${PROFILE:-dist}"
dir="${CARGO_TARGET_DIR:-$root/target}"
out="$dir/cli/$target"
mkdir -p "$out"
cd "$root"
case $target in
  *-linux-*)
    bolt="$(realpath "${LLVM_BOLT:-$(command -v llvm-bolt)}")" || { echo "build-cli.sh: needs llvm-bolt on PATH or LLVM_BOLT" >&2; exit 1; }
    flags="-Crelocation-model=static -Clink-arg=-Wl,--emit-relocs"
    case $target in x86_64-unknown-linux-gnu) flags="$flags -Clink-arg=-Wl,--icf=all" ;; esac
    env "CARGO_PROFILE_$(printf '%s' "$profile" | tr a-z- A-Z_)_STRIP=none" RUSTFLAGS="$flags" \
      cargo build --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    built="$dir/$target/$profile/$bin"
    train="$dir/bolt/$target/$bin"
    rm -rf "$train"
    mkdir -p "$train"
    "$bolt" "$built" -instrument -instrumentation-file="$train/profile" -instrumentation-file-append-pid -o "$train/instrumented" >&2
    python3 "$root/scripts/train-cli.py" "$train/instrumented" >&2
    "$(dirname "$bolt")/merge-fdata" "$train"/profile.*.fdata > "$train/profile.fdata"
    "$bolt" "$built" -o "$train/optimized" -data="$train/profile.fdata" -reorder-blocks=ext-tsp -reorder-functions=cdsort \
      -split-functions -split-all-cold -split-eh -icf=all -use-gnu-stack -no-huge-pages >&2
    strip -o "$out/$bin" "$train/optimized"
    ;;
  *)
    cargo build --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    cp "$dir/$target/$profile/$bin" "$out/$bin"
    ;;
esac
echo "$out/$bin"
