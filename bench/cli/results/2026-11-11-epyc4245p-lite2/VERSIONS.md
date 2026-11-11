# Installed CLIs

Linux x86_64. nc and nclite are release-profile builds (cargo build --release, lto off) of nanocodexyz/nanocode master 1da9dd20c on Ubuntu 24.04; the published nanocode-cli release uses the dist profile (fat LTO), and nc-lite's dist build is 26.9 MB.

| Lane | Version | Install | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 (1da9dd20c) | cargo build --locked --release --manifest-path bin/nanocode/Cargo.toml --bin nanocode | tools/install/ncm/nanocode | none |
| nclite | nc-lite 0.7.0 (1da9dd20c) | cargo build --locked --release -p nc-lite | tools/install/nclite/nc-lite | none |
| fx | 0.0.16 | curl https://releases.fx.sh/0.0.16/fx-linux-x86_64.tar.gz | tools/install/fx/fx | none |
