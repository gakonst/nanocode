# Dependencies

What to trim or replace next, ranked by what it costs against the effort to remove. Measured on 2026-11-12 at 7b0586574 on a 5-core Linux Hand: `cargo metadata` resolves 1,176 packages (1,175 after the first batch below); `cargo build --release -p nanocode-cli --timings` takes 7m07s, 1,623 s of summed unit time; the release `nanocode` is 108 MB stripped with 66.5 MiB of `.text` (`cargo bloat --crates`). "Only it" counts the packages that leave the graph when the dependency goes (every target, normal and build edges).

| Rank | Dependency | Where | Only it | Cost | Replace with | Effort |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | mpp, tempo-alloy | nanocode-cli (`tempo.rs`; tempo-alloy for one call) | 216 together | aws-lc-sys 106.8 s (the largest C build, through commonware-cryptography), bundled SQLite 33.5 s (mpp `sqlite` for `SqliteChannelStore`), tempo-contracts 25.1 s, mpp 16.5 s; mpp 1.5 MiB of text plus alloy; brings rand 0.8 and 0.9, digest 0.10, k256, dashmap | a separate executable spawned by the CLI, as nanocode-cua-driver was split out, or MPP's client side written directly over the workspace HTTP and a small signer | large |
| 2 | syntect `regex-onig` | nanocode-tui | onig, onig_sys | 11.5 s of C | syntect `regex-fancy`, after measuring highlighting speed | small |
| 3 | tokio-postgres, tokio-postgres-rustls, webpki-roots beside sqlx | nanocode-store-sql (`PostgresSql`, used only by tempoxyz/pyramid-core for one-round-trip locked writes) | 11 (sqlx alone 9) | two Postgres drivers | `PostgresSql` over sqlx's simple protocol (`EXECUTE` of prepared statements, literal parameters), or pyramid-core keeping its lanes; both change pyramid-core (2026-11-12 entry) | medium, with pyramid-core |
| 4 | portable-pty | nanocode-process | 8 | nix 0.28 beside 0.31 | rustix's pty, already a dependency | small |
| 5 | clap | nanocode-cli, nanocode-observability | 12 | clap_builder 4.7 s, 275 KiB | a parser over `std::env::args_os` | medium |
| 6 | schemars 0.8 | tools, tool-host, both backends | 7 | nothing else builds schemars since gakonst/cua went (0.9 and 1.x stay in Cargo.lock as optional dependencies of alloy's serde_with) | schemars 1, or the schemas written as the pinned upstream JSON they are | medium |
| 7 | chrono, iana-time-zone | both backends, code-mode-engine | 8 | 1.75 s; used for today's local date and the UTC offset | a TZif reader on native and the host's offset on WASM | small |
| 8 | opentelemetry, -otlp, -proto, -sdk, tracing-opentelemetry | nanocode-observability, nanocode-cli | 10 plus prost | 378 KiB for the SDK | OTLP/HTTP export written over the workspace HTTP client | medium |
| 9 | image `=0.25.6` | nanocode-image and its users | 8 | pins png 0.17, which brings bitflags 1 | a newer image and png 0.18, once the reason for the pin is checked | small |
| 10 | tracing-appender | nanocode-observability | 2 | crossbeam-channel, symlink | an owning writer thread behind a bounded channel | small |
| 11 | base64 with base64-simd | audio, image, codex, code-mode-engine, computer-repl | 1 | two encoders in five crates | one of them, keeping the measured fast paths | small |

Kept on purpose: rustpython (103 packages, 110.1 s for rustpython-vm and 38.6 s for malachite; embedded Python stays in the CLI by decision), ratatui (15), djs and psh (patched in tree). Not worth it alone: tower (1, the Codex client is built as services), async-trait (1, a proc macro in 15 crates), httpdate, unicode-normalization, xxhash-rust (an upstream Claude Code hash), subtle, futures-channel.

Also from `cargo shear`: the `nanocode` facade's `serde_json`, `nanocode-home`, `tokio` (with `fs`, `net` and `macros`) and Linux `nix` are used only by its examples, so every host that links the facade takes those tokio features; `crates/runtime/code-mode/src/code_mode/schema_types.rs` is not part of any module; nanocode-cli's `nanocode-agent-host` is there to turn on `cua`.

Removed: `imbl` from nanocode-agent (never used) and `opentelemetry-semantic-conventions` (two of its 21,000 lines of constants were used) on 2026-11-12; gakonst/cua's cua-driver-sdk, cua-driver-core and cursor-overlay on 2026-11-12, 200 packages with zstd-sys, the last C zstd (`cargo metadata` resolves 974), the built-in macOS driver now being nanocode-computer's own engine.
