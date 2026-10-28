#!/bin/sh
set -eu
release="${NANOCODE_RELEASE:-https://github.com/gakonst/nanocode/releases/download/nanocode-cli}"
bin="${NANOCODE_BIN:-$HOME/.nanocode/bin}"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64 | Linux-amd64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "nanocode has no build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
digest() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$release/SHA256SUMS" -o "$tmp/SHA256SUMS"
expected="$(grep " nanocode-$target.gz$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
if [ -z "$expected" ]; then echo "nanocode: the release has no build for $target" >&2; exit 1; fi
mkdir -p "$bin"
if [ ! -x "$bin/nanocode" ] || [ "$(cat "$bin/.nanocode.sha256" 2>/dev/null)" != "$expected" ]; then
  echo "nanocode: downloading $target" >&2
  curl -fsSL "$release/nanocode-$target.gz" -o "$tmp/nanocode.gz"
  if [ "$(digest "$tmp/nanocode.gz")" != "$expected" ]; then echo "nanocode: checksum mismatch for nanocode-$target.gz" >&2; exit 1; fi
  gunzip -c "$tmp/nanocode.gz" > "$tmp/nanocode"
  chmod +x "$tmp/nanocode"
  mv "$tmp/nanocode" "$bin/nanocode"
  ln -sf nanocode "$bin/nc"
  echo "$expected" > "$bin/.nanocode.sha256"
  echo "nanocode: installed $bin/nc" >&2
fi
case ":$PATH:" in
  *":$bin:"*) ;;
  *)
    case "$(basename "${SHELL:-sh}")" in
      zsh) profile="$HOME/.zshrc"; line="export PATH=\"$bin:\$PATH\"" ;;
      bash) profile="$HOME/.bashrc"; line="export PATH=\"$bin:\$PATH\"" ;;
      fish) profile="$HOME/.config/fish/conf.d/nanocode.fish"; line="fish_add_path $bin" ;;
      *) profile="" ;;
    esac
    if [ -n "$profile" ] && [ -z "${NANOCODE_NO_MODIFY_PATH:-}" ]; then
      mkdir -p "$(dirname "$profile")"
      if grep -qsF "$line" "$profile"; then
        echo "nanocode: $profile puts $bin on your PATH; open a new shell to run nc" >&2
      else
        printf '\n%s\n' "$line" >> "$profile"
        echo "nanocode: added $bin to your PATH in $profile; open a new shell to run nc" >&2
      fi
    else
      echo "nanocode: add $bin to your PATH to run nc from anywhere" >&2
    fi
    ;;
esac
rm -rf "$tmp"
trap - EXIT
if [ "$#" -gt 0 ]; then exec "$bin/nanocode" "$@"; fi
echo "nanocode: run $bin/nc to start" >&2
