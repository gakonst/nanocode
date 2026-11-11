# grep-printer 0.3.1, patched for psh

This is grep-printer 0.3.1 as published on crates.io (ripgrep's `crates/printer`, by
Andrew Gallant, dual-licensed MIT or Unlicense; see `LICENSE-MIT`, `UNLICENSE` and psh's
`NOTICE`), used through `[patch.crates-io]` in psh's `Cargo.toml` (D1135). The first
commit that adds this directory is the crate verbatim; the changes are only these:

1. `src/instant.rs` (new): `Instant` is `std::time::Instant`, except on
   `wasm32-unknown-unknown`, where it is a clock that reads zero elapsed time.
2. `src/lib.rs`: `mod instant;`.
3. `src/standard.rs`, `src/summary.rs`, `src/json.rs`: `Instant` comes from
   `crate::instant` instead of `std::time` (the `use` lines only).

Why: every sink calls `Instant::now()` when it is created and when a search begins (for
the elapsed time in `Stats`), and on `wasm32-unknown-unknown` `std::time::Instant::now()`
panics ("time not implemented on this platform"), so no printer can be used in a
`wasm32-unknown-unknown` build. With the patch, statistics there report zero elapsed
time; nothing else changes.

Dropping the patch: once an upstream grep-printer no longer reads the clock
unconditionally (or takes it from a caller), delete this directory, the
`[patch.crates-io]` entry and the workspace `exclude`, and run `cargo update -p
grep-printer`.
