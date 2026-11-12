# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-12T03:11Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-12T03:11Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v24.22.3 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-12T03:11Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.322 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-12T03:11Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (326.1 MiB) | none |
| codex | codex-cli 0.167.2 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-12T03:11Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (305.4 MiB) | none (the npm launcher needs Node) |
| nc | nanocode 0.7.0 (3c16e6c8e) | cargo build --locked --profile dist -p nanocode-cli --bin nanocode at nanocodexyz/nanocode 3c16e6c8e | | `tools/install/nc/nanocode` (71.4 MiB) | none |
| nclite | nc-lite 0.7.0 (3c16e6c8e) | cargo build --locked --profile lite -p nc-lite at nanocodexyz/nanocode 3c16e6c8e | | `tools/install/nclite/nc-lite` (14.1 MiB) | none |
