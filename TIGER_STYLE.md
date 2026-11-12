# TigerStyle

How nanocode applies [TigerBeetle's TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md), as tempoxyz/zempo does, so that bugs surface early, in a simulator, and performance follows from the design. AGENTS.md wins where the two differ. A change that departs from a rule says so in its DECISIONS.md entry, with why and what bounds it.

## Safety

- Assert what the code relies on: arguments, results and the invariants of each lifecycle move. A broken invariant of our own is a panic, never a value that flows on.
- Pair assertions across a boundary: check a property where it is written and again where it is read back, such as a journal record encoded and decoded, a page committed and loaded, a fence taken and checked.
- Assert the positive and the negative space: what must hold, and what must not happen, such as a stale owner's commit.
- Split compound assertions so a failure names the one condition that broke.
- Three kinds of failure, never mixed: our invariant broken (panic); external input that is wrong, from a model, a peer, a file, a store or a user (a typed error naming what was wrong); local exhaustion (a typed error naming the bound that was reached, or backpressure). Never assert on external input, and never hide any of the three behind truncation, clamping or a fallback.
- `assert!` by default. `debug_assert!` only where a measurement shows the check costs on a hot path.

## Limits

- Every queue, buffer, batch, retry, concurrency level, depth and loop has a bound. Its constant is named for what it bounds, its unit last (`READ_PART_KEYS`, `latency_ms_max`), and its value comes from a protocol, a measurement or caller policy, as AGENTS.md requires.
- Reaching a bound is an error or backpressure the caller sees, never a silent drop.
- Walk input-shaped data (JSON, schemas, agent trees, shell syntax) iteratively, or with an explicit depth bound that returns an error.

## Memory

- Size buffers from known bounds up front and reuse them on hot paths; no allocation per token, per line or per record where a reused buffer does.
- Copy-on-write state (`Atom`) is sized so an update copies what it must and no more; shard what grows with the number of sessions or agents.
- Back every memory or allocation change with before and after measurements in DECISIONS.md.

## Control flow

- Model each lifecycle as one enum whose variants carry only what that phase has (AGENTS.md).
- Push `if`s up and `for`s down: decide once at the top, then run straight-line loops over the batch.
- Handle every `Result`. Dropping one is a decision with a reason in DECISIONS.md.
- One owner per piece of state, changed through its mailbox or atomics (AGENTS.md); no hidden work in `Drop` beyond releasing what the value owns.
- Keep functions short enough to read at once, around 70 lines; split a longer one when touching it.

## Determinism

The simulator replays a seed exactly only if the code under it is deterministic. In the durability, runtime and store crates:

- Read time, randomness and identifiers only through nanocode-platform (`system_time`, `unix_millis`, `Instant`, `sleep`, `timeout`, `fill_random`, `uuid_v4`, `uuid_v7`), never `std::time::SystemTime::now`, `std::time::Instant::now`, `tokio::time`, `getrandom` or environment variables directly.
- Reach storage only through the durability traits (`StateStore`, `KeyValue`, `Sql`, `BlobStore`), so a simulated store with faults can stand in.
- Iterate in an order that does not depend on a random hasher wherever the order reaches persisted bytes, the wire or a decision: `BTreeMap`, a sorted `Vec`, or a fixed-seed hasher.
- Spawn tasks through the runtime and keep blocking I/O on owning workers behind channels (AGENTS.md), so the simulator can run them on one thread in virtual time.
- An error carries no addresses, timestamps or other values that differ between two runs of the same seed.

## Dependencies

- Prefer std, then a workspace crate (nanocode-sync, nanocode-zstd, nanocode-http, nanocode-platform), then a small crate without dependencies. No C build scripts in the SDK crates.
- `default-features = false`, enabling only what is used.
- A dependency is added or kept with its cost in its DECISIONS.md entry: the packages only it brings (`cargo tree`), its build time (`cargo build --timings`) and its share of the binary (`cargo bloat --crates`), and why std or an existing crate does not do.
- One version of each crate. A change that adds a duplicate to `cargo tree --duplicates` says why.
- DEPENDENCIES.md ranks what to trim or replace next; take from it when touching those crates.

## Naming

- Whole words, no abbreviations except established ones (`id`, `url`, `http`). Units and qualifiers last, so related names line up: `bytes_max`, `bytes_read`.
- Name a constant for the bound or protocol value it is; a bare number in code is a constant waiting for a name.
