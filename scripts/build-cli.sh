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
remap="--remap-path-prefix=$(pwd -P)=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)/lib/rustlib/src/rust=/rustc/$(rustc -vV | sed -n 's/^commit-hash: //p')"
case $target in
  *-linux-gnu.*)
    RUSTFLAGS="$remap" cargo zigbuild --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    cp "$dir/${target%%.*}/$profile/$bin" "$out/$bin"
    ;;
  *-linux-*)
    bolt="$(realpath "${LLVM_BOLT:-$(command -v llvm-bolt)}")" || { echo "build-cli.sh: needs llvm-bolt on PATH or LLVM_BOLT" >&2; exit 1; }
    flags="-Crelocation-model=static -Clink-arg=-Wl,--emit-relocs $remap"
    icf=all
    "$bolt" --help-hidden | grep -q -- '--icf=<value>' || icf=1
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
      -split-functions -split-all-cold -split-eh -icf=$icf -use-old-text -use-gnu-stack -no-huge-pages >&2
    strip --remove-section=.note.bolt_info -o "$out/$bin" "$train/optimized"
    ;;
  *)
    RUSTFLAGS="$remap" cargo build --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    cp "$dir/$target/$profile/$bin" "$out/$bin"
    ;;
esac
echo "$out/$bin"
