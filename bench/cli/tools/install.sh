#!/bin/sh
# Installs the latest release of each CLI under tools/install/<lane>/ (nothing global), then writes
# tools/install/env.sh (the variables the harness reads) and tools/install/VERSIONS.md.
# sh tools/install.sh            all six lanes
# LANES=nc,fx sh tools/install.sh
# LANES=nclite builds nc-lite (bin/nc-lite in the nanocode repository) with its scripts/build-cli.sh at NCLITE_REV (llvm-bolt on PATH or LLVM_BOLT on Linux)
set -eu
cd "$(dirname "$0")/.."
dir="$PWD/tools/install"
lanes="$(printf '%s' "${LANES:-nc,fx,pi,opencode,claude,codex}" | tr , ' ')"
nc_release="${NANOCODE_RELEASE:-https://github.com/nanocodexyz/nanocode/releases/download/nanocode-cli}"
fx_cdn="${FX_CDN:-https://releases.fx.sh}"
nclite_repo="${NCLITE_REPO:-https://github.com/nanocodexyz/nanocode}"
nclite_rev="${NCLITE_REV:-7837aae16afe1e46fbf72471c902f5cf2e683270}"
mkdir -p "$dir"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) nc_target=aarch64-apple-darwin fx_target=macos-aarch64 ;;
  Darwin-x86_64) nc_target=x86_64-apple-darwin fx_target=macos-x86_64 ;;
  Linux-x86_64 | Linux-amd64) nc_target=x86_64-unknown-linux-gnu fx_target=linux-x86_64 ;;
  Linux-aarch64 | Linux-arm64) nc_target=aarch64-unknown-linux-gnu fx_target=linux-aarch64 ;;
  *) echo "install.sh: no builds for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
digest() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
npm_lane() { # lane package
  mkdir -p "$dir/$1"
  [ -f "$dir/$1/package.json" ] || echo '{"private":true}' > "$dir/$1/package.json"
  npm install --prefix "$dir/$1" --no-audit --no-fund --loglevel=error "$2@latest" >&2
}

for lane in $lanes; do
  echo "install.sh: $lane" >&2
  case "$lane" in
    nc)
      mkdir -p "$dir/nc"
      curl -fsSL "$nc_release/SHA256SUMS" -o "$tmp/SHA256SUMS"
      sum="$(grep " nanocode-$nc_target.gz$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
      [ -n "$sum" ] || { echo "install.sh: the nanocode-cli release has no build for $nc_target" >&2; exit 1; }
      if [ ! -x "$dir/nc/nanocode" ] || [ "$(cat "$dir/nc/.sha256" 2>/dev/null)" != "$sum" ]; then
        curl -fsSL "$nc_release/nanocode-$nc_target.gz" -o "$tmp/nc.gz"
        [ "$(digest "$tmp/nc.gz")" = "$sum" ] || { echo "install.sh: checksum mismatch for nanocode-$nc_target.gz" >&2; exit 1; }
        gunzip -c "$tmp/nc.gz" > "$dir/nc/nanocode"
        chmod +x "$dir/nc/nanocode"
        echo "$sum" > "$dir/nc/.sha256"
      fi
      ;;
    fx)
      mkdir -p "$dir/fx"
      version="$(curl -fsSL "$fx_cdn/latest.txt" | tr -d '[:space:]')"
      [ -n "$version" ] || { echo "install.sh: no fx version at $fx_cdn/latest.txt" >&2; exit 1; }
      if [ ! -x "$dir/fx/fx" ] || [ "$(cat "$dir/fx/.version" 2>/dev/null)" != "$version" ]; then
        curl -fsSL "$fx_cdn/$version/fx-$fx_target.tar.gz" -o "$tmp/fx.tar.gz"
        tar -xzf "$tmp/fx.tar.gz" -C "$tmp" fx
        mv "$tmp/fx" "$dir/fx/fx"
        chmod +x "$dir/fx/fx"
        echo "$version" > "$dir/fx/.version"
      fi
      ;;
    nclite)
      mkdir -p "$dir/nclite"
      if [ ! -x "$dir/nclite/nc-lite" ] || [ "$(cat "$dir/nclite/.rev" 2>/dev/null)" != "$nclite_rev" ]; then
        src="$dir/.build/nanocode"
        [ -d "$src/.git" ] || git clone --quiet --filter=blob:none "$nclite_repo" "$src"
        git -C "$src" fetch --quiet origin "$nclite_rev"
        git -C "$src" checkout --quiet --detach "$nclite_rev"
        cp "$(cd "$src" && PROFILE=lite scripts/build-cli.sh nc-lite nc-lite)" "$dir/nclite/nc-lite"
        echo "$nclite_rev" > "$dir/nclite/.rev"
      fi
      ;;
    pi) npm_lane pi @earendil-works/pi-coding-agent ;;
    opencode) npm_lane opencode opencode-ai ;;
    claude) npm_lane claude @anthropic-ai/claude-code ;;
    codex) npm_lane codex @openai/codex ;;
    *) echo "install.sh: unknown lane $lane" >&2; exit 1 ;;
  esac
done

# Each lane's main binary, how it was installed, and the runtime it needs that the size does not count.
info() {
  m="$dir/$1/node_modules"
  case "$1" in
    nc) bin="$dir/nc/nanocode" var=NC_BIN how="curl $nc_release/nanocode-$nc_target.gz (the asset https://nanocode.xyz/install fetches; its optional macOS computer-use driver is left out)" runtime=none ;;
    nclite) bin="$dir/nclite/nc-lite" var=NCLITE_BIN how="PROFILE=lite scripts/build-cli.sh nc-lite nc-lite at $nclite_repo $nclite_rev" runtime=none ;;
    fx) bin="$dir/fx/fx" var=FX_BIN how="curl $fx_cdn/\$(latest.txt)/fx-$fx_target.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only)" runtime=none ;;
    pi) bin="$m/@earendil-works/pi-coding-agent/dist/bundle/cli.js" var=PI_CLI how="npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest" runtime="Node $(node --version)" ;;
    opencode) bin="$m/opencode-ai/bin/opencode.exe" var=OPENCODE_BIN how="npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package)" runtime=none ;;
    claude) bin="$m/@anthropic-ai/claude-code/bin/claude.exe" var=CLAUDE_BIN how="npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package)" runtime=none ;;
    codex) bin="$(find "$m" -path '*/vendor/*/bin/codex' -type f 2>/dev/null | head -n 1)" var=CODEX_BIN how="npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it)" runtime="none (the npm launcher needs Node)" ;;
  esac
}
versions="$dir/VERSIONS.md"
rows="$tmp/rows"
: > "$rows"
: > "$tmp/env.sh"
for lane in nc nclite fx pi opencode claude codex; do
  [ -d "$dir/$lane" ] || continue
  info "$lane"
  [ -n "$bin" ] && [ -f "$bin" ] || { echo "install.sh: $lane has no binary at '$bin'" >&2; exit 1; }
  upper="$(printf '%s' "$lane" | tr '[:lower:]' '[:upper:]')"
  printf "export %s='%s'\nexport BENCH_INSTALL_%s='%s'\n" "$var" "$bin" "$upper" "$dir/$lane" >> "$tmp/env.sh"
  if printf ' %s ' "$lanes" | grep -q " $lane " || ! grep -q "^| $lane |" "$versions" 2>/dev/null; then
    mkdir -p "$tmp/home"
    if [ "$var" = PI_CLI ]; then version="$(HOME="$tmp/home" node "$bin" --version 2>/dev/null | head -n 1)"; else version="$(HOME="$tmp/home" "$bin" --version 2>/dev/null | head -n 1)"; fi
    size="$(wc -c < "$bin" | awk '{ printf "%.1f MiB", $1 / 1048576 }')"
    printf '| %s | %s | %s | %s | `%s` (%s) | %s |\n' "$lane" "$version" "$how" "$(date -u +%Y-%m-%dT%H:%MZ)" "${bin#"$PWD/"}" "$size" "$runtime" >> "$rows"
  else
    grep "^| $lane |" "$versions" >> "$rows"
  fi
done
mv "$tmp/env.sh" "$dir/env.sh"
{
  echo "# Installed CLIs"
  echo
  echo "Written by tools/install.sh on $(uname -s) $(uname -m). The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory."
  echo
  echo "| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |"
  echo "| --- | --- | --- | --- | --- | --- |"
  cat "$rows"
} > "$versions"
echo "install.sh: wrote $dir/env.sh and $versions; load them with . tools/install/env.sh" >&2
