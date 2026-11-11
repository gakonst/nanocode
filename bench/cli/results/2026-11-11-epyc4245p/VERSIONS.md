# Installed CLIs

Linux x86_64. nc and nclite are release-profile builds (lto off, as cargo build --release) of nanocodexyz/nanocode master 9f373a4b9 on Ubuntu 24.04; the published nanocode-cli release uses the dist profile (fat LTO). Harness: cli-bench ab0f0fc73.

| Lane | Version | Install | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 (9f373a4b9) | cargo build --locked --release --manifest-path bin/nanocode/Cargo.toml --bin nanocode | tools/install/ncm/nanocode | none |
| nclite | nc-lite 0.7.0 (9f373a4b9) | cargo build --locked --release -p nanocode --example nc_lite | tools/install/nclite/nc-lite | none |
| fx | 0.0.16 | curl https://releases.fx.sh/0.0.16/fx-linux-x86_64.tar.gz | tools/install/fx/fx | none |
