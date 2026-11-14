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
digest() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
remap="--remap-path-prefix=$(pwd -P)=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)/lib/rustlib/src/rust=/rustc/$(rustc -vV | sed -n 's/^commit-hash: //p')"
case $target in
  *-linux-gnu.*)
    if [ "$package" = nanocode-cli ]; then
      RUSTFLAGS="$remap" cargo zigbuild --locked --profile "$profile" --target "$target" -p nanocode-python-cli --bin nanocode-python >&2
      cp "$dir/${target%%.*}/$profile/nanocode-python" "$out/nanocode-python"
      NANOCODE_PYTHON_SHA256="$(digest "$out/nanocode-python")"
      export NANOCODE_PYTHON_SHA256
    fi
    RUSTFLAGS="$remap" cargo zigbuild --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    cp "$dir/${target%%.*}/$profile/$bin" "$out/$bin"
    ;;
  *-linux-*)
    bolt="$(realpath "${LLVM_BOLT:-$(command -v llvm-bolt)}")" || { echo "build-cli.sh: needs llvm-bolt on PATH or LLVM_BOLT" >&2; exit 1; }
    flags="-Crelocation-model=static -Clink-arg=-Wl,--emit-relocs -Clink-arg=-Wl,-T,$root/scripts/cli-eh-frame.ld $remap"
    icf=all
    "$bolt" --help-hidden | grep -q -- '--icf=<value>' || icf=1
    case $target in x86_64-unknown-linux-gnu) flags="$flags -Clink-arg=-Wl,--icf=all" ;; esac
    nostrip="CARGO_PROFILE_$(printf '%s' "$profile" | tr a-z- A-Z_)_STRIP=none"
    if [ "$package" = nanocode-cli ]; then
      env "$nostrip" RUSTFLAGS="$flags" cargo build --locked --profile "$profile" --target "$target" -p nanocode-python-cli --bin nanocode-python >&2
      strip -o "$out/nanocode-python" "$dir/$target/$profile/nanocode-python"
      NANOCODE_PYTHON_SHA256="$(digest "$out/nanocode-python")"
      export NANOCODE_PYTHON_SHA256
    fi
    env "$nostrip" RUSTFLAGS="$flags" \
      cargo build --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    built="$dir/$target/$profile/$bin"
    train="$dir/bolt/$target/$bin"
    rm -rf "$train"
    mkdir -p "$train"
    "$bolt" "$built" -instrument -instrumentation-file="$train/profile" -instrumentation-file-append-pid -o "$train/instrumented" >&2
    python3 "$root/scripts/train-cli.py" "$train/instrumented" startup >&2
    mkdir "$train/startup"
    mv "$train"/profile.*.fdata "$train/startup/"
    python3 "$root/scripts/train-cli.py" "$train/instrumented" >&2
    "$(dirname "$bolt")/merge-fdata" "$train"/profile.*.fdata "$train"/startup/*.fdata > "$train/profile.fdata"
    layout="-reorder-blocks=ext-tsp -split-functions -split-all-cold -split-eh -icf=$icf -use-old-text -no-huge-pages"
    "$bolt" "$built" -o "$train/cdsort" -data="$train/profile.fdata" $layout -reorder-functions=cdsort -generate-function-order="$train/cdsort.order" >&2
    rm "$train/cdsort"
    python3 "$root/scripts/order-cli.py" "$train/cdsort.order" "$train"/startup/*.fdata > "$train/startup.order"
    "$bolt" "$built" -o "$train/optimized" -data="$train/profile.fdata" $layout -reorder-functions=user -function-order="$train/startup.order" >&2
    "$(dirname "$bolt")/llvm-strip" --remove-section=.note.bolt_info -o "$out/$bin" "$train/optimized"
    python3 "$root/scripts/trim-cli.py" "$out/$bin"
    ;;
  *)
    if [ "$package" = nanocode-cli ]; then
      RUSTFLAGS="$remap" cargo build --locked --profile "$profile" --target "$target" -p nanocode-python-cli --bin nanocode-python >&2
      cp "$dir/$target/$profile/nanocode-python" "$out/nanocode-python"
      NANOCODE_PYTHON_SHA256="$(digest "$out/nanocode-python")"
      export NANOCODE_PYTHON_SHA256
    fi
    RUSTFLAGS="$remap" cargo build --locked --profile "$profile" --target "$target" -p "$package" --bin "$bin" >&2
    cp "$dir/$target/$profile/$bin" "$out/$bin"
    ;;
esac
echo "$out/$bin"
