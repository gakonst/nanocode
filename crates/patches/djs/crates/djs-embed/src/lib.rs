//! Embed djs, a small JavaScript engine, in a Rust program.
//!
//! The host compiles its setup once into a [`Snapshot`]: the host functions the scripts may
//! call, plain-data globals, and setup sources (a shim, a policy script) that run to
//! completion. Each [`Instance`] is a VM in that state, made by copying the snapshot's
//! image; it calls one JS function at a time, and [`Instance::reset`] puts it back into the
//! snapshot's state. Values cross the boundary as typed arguments and results ([`Value`]
//! in, [`ValueRef`] out) or as plain data ([`Data`], or its byte form, the wire format),
//! never as JSON text. Every call has budgets a script cannot catch: fuel, memory and a
//! wall-clock deadline ([`Limits`]).
//!
//! ```ignore
//! use djs_embed::{Data, HostFn, Limits, Snapshot, Source, Step, Throw, Value};
//!
//! const LOG: u32 = 1;
//! const FETCH: u32 = 2;
//!
//! let snapshot = Snapshot::builder()
//!     .host_fn(HostFn::sync(LOG, "log"))
//!     .host_fn(HostFn::asynchronous(FETCH, "fetchText"))
//!     .global("config", Data::object([("greeting", Data::from("hello"))]))
//!     .source(Source::script("policy.js", r#"
//!         async function onRequest(req) {
//!             log(config.greeting + " " + req.url);
//!             const body = await fetchText(req.url);
//!             return { allow: !body.includes("secret"), length: body.length };
//!         }
//!     "#))
//!     .build()?;
//!
//! let mut vm = snapshot.instantiate(Limits::default())?;
//! let req = Data::object([("url", Data::from("https://example.com/"))]);
//! vm.start("onRequest", &[Value::Data(&req)])?;
//! let decision = loop {
//!     match vm.step()? {
//!         Step::Done => break vm.result()?.to_data()?,
//!         Step::Yield => {} // a slice of fuel is used up: yield the thread if it is shared
//!         Step::HostCall(id) => {
//!             // A sync host function: answer before the script goes on.
//!             let call = vm.host_call(id)?;
//!             println!("log: {}", call.arg(0).as_str().unwrap_or_default());
//!             call.reply(Ok(Value::Undefined))?;
//!         }
//!         Step::Wait(started) => {
//!             // Async host functions: start the work, answer when it is done.
//!             for id in started {
//!                 let url = vm.host_call(id)?.arg(0).as_str().map(|s| s.into_owned());
//!                 match url {
//!                     Some(url) => vm.resolve(id, Ok(Value::Str(&format!("body of {url}"))))?,
//!                     None => vm.resolve(id, Err(Throw::type_error("fetchText: no URL")))?,
//!                 }
//!             }
//!         }
//!     }
//! };
//! assert_eq!(decision.get("allow"), Some(&Data::Bool(true)));
//! vm.reset()?; // ready for the next call, in the snapshot's state
//! ```
//!
//! [`Instance::call`] is the same loop for hosts that answer every host call at once.
//!
//! # Snapshots
//!
//! [`Snapshot::builder`] defines the host functions ([`HostFn::sync`],
//! [`HostFn::asynchronous`]) and data globals ([`Builder::global`]) as non-enumerable
//! globals, then runs each [`Source`] in order with a fuel budget of its own. A syntax
//! error (or a module's link error) is [`Error::Syntax`], a module the loader cannot
//! supply [`Error::Load`], a top-level throw [`Error::Exception`]; setup code must not
//! call host functions. The builtins are frozen by default ([`Builder::frozen_builtins`]),
//! so a script cannot patch `Array.prototype` for the next call. The result is an image of
//! the VM (a few hundred KB for a shim and a policy), immutable, cheap to clone and
//! `Send + Sync`. Images exist only in memory: rebuilding from the sources is how a host
//! gets one again.
//!
//! [`Snapshot::instantiate`] copies the image into a new VM: no compile, no setup code.
//! [`Instance::reset`] copies it into the instance's existing heap allocation instead,
//! which saves the allocation and the page faults of a fresh one. A fresh or reset
//! instance shares nothing with earlier calls; an instance used for many calls without a
//! reset keeps its globals (and whatever a call left in them) from one call to the next.
//!
//! # Calls
//!
//! [`Instance::start`] looks up a function by its dotted global path (`"onRequest"`,
//! `"hooks.onRequest"`: own data properties only, no getter runs) and starts it with the
//! arguments; `this` is the object the function was found on. [`Instance::step`] then runs
//! one slice of at most [`Limits::slice`] fuel and returns what stopped it ([`Step`]):
//!
//! - [`Step::Done`]: the call settled. If the function returned a promise (an `async`
//!   function), that is when the promise settled; the value is kept for the host
//!   ([`Instance::result`]) until the next call starts. Whatever the call left behind
//!   (async host calls no one awaits, queued jobs) is dropped.
//! - [`Step::HostCall`]: a sync host function was called; the VM waits for
//!   [`HostCall::reply`] or [`Instance::resolve`].
//! - [`Step::Wait`]: the script waits for async host functions. The step lists the calls
//!   made since the last `Wait`; answers may come in any order and at any time (before
//!   the next step, or later), and the script sees each as a separate macrotask in the
//!   order the host delivered them.
//! - [`Step::Yield`]: the slice ended. Stepping again continues.
//!
//! Host function arguments are read with [`HostCall::arg`]: `as_str` (borrowed from the
//! heap when the string is ASCII), `as_bytes` (a `Uint8Array`'s bytes, borrowed),
//! `as_number`, `as_bool`, `to_data`, `to_wire`. A [`ValueRef`] borrows the instance, so
//! copy out what must outlive the next step. Answers are a [`Value`] (strings and bytes are
//! copied into the heap once) or a [`Throw`], an Error of a [`ErrorKind`] with a message.
//!
//! # Modules
//!
//! A setup source can be an ES module ([`Source::module`], [`Source::typescript_module`]):
//! setup loads it and everything it imports, links and evaluates the graph, so the
//! snapshot holds the evaluated modules and instances start with them, no module code run
//! again. Imports resolve through the snapshot's [`Loader`] ([`Builder::loader`]; a
//! [`ModuleMap`] is the in-memory one, with `./` and `../` relative to the importer's key)
//! and its [`SyntheticModule`]s ([`Builder::module`]): modules without source whose exports
//! are host functions and plain data, imported by their exact key. Specifiers and keys are
//! opaque strings; djs does no I/O.
//!
//! ```ignore
//! use djs_embed::{Data, HostFn, Limits, ModuleMap, Snapshot, Source, SyntheticModule, Value};
//!
//! const ALLOW: u32 = 1;
//!
//! let snapshot = Snapshot::builder()
//!     .module(
//!         SyntheticModule::new("@sdk/net")
//!             .host_fn(HostFn::sync(ALLOW, "allowHosts"))
//!             .data("secret", Data::from("t0k")),
//!     )
//!     .loader(ModuleMap::new().module("lib/hosts.js", "export const lower = (h) => h.toLowerCase();"))
//!     .source(Source::module("policy.js", r#"
//!         import { allowHosts, secret } from "@sdk/net";
//!         import { lower } from "./lib/hosts.js";
//!         export async function onRequest(req) {
//!             return { allow: allowHosts(lower(req.host)), auth: secret };
//!         }
//!     "#))
//!     .build()?;
//!
//! let mut vm = snapshot.instantiate(Limits::default())?;
//! let req = Data::object([("host", Data::from("Example.com"))]);
//! let decision = vm.call_export("policy.js", "onRequest", &[Value::Data(&req)], |call| {
//!     let ok = call.arg(0).as_str().is_some_and(|h| h == "example.com");
//!     call.reply(Ok(Value::Bool(ok)))
//! })?;
//! assert_eq!(decision.get("allow"), Some(&Data::Bool(true)));
//! ```
//!
//! [`Instance::start_export`] and [`Instance::call_export`] call a module's exported
//! function (`this` undefined), as [`Instance::start`] and [`Instance::call`] do a global's;
//! [`Instance::export`] reads an export's current value. An `import()`, in a module or a
//! script, goes through the same loader: [`Instance::step`] answers it itself between two
//! slices (the load and compile are not counted as fuel), evaluates the graph and settles
//! the promise with the module namespace; a module that cannot be found, does not compile
//! or throws rejects it. Modules an `import()` loaded stay in the instance until
//! [`Instance::reset`].
//!
//! JSON modules: a [`ModuleSource::json`] (or [`ModuleMap::json`]) is imported `with {
//! type: "json" }` and has the parsed text as its default export; an import with that
//! attribute loads nothing else, and one without it cannot load JSON (a `TypeError`). Other
//! import attributes are a `SyntaxError`. Top-level `await` works: setup waits for the
//! graph's evaluation to settle (no host function may be called during setup, so it can
//! wait only on promises and `import()`); one that never settles is
//! [`Error::NeverSettles`].
//!
//! # Plain data
//!
//! [`Data`] is the owned tree: undefined, null, booleans, numbers, strings, bytes
//! (`Uint8Array`), arrays (with holes) and objects (keys in order). Out of the VM,
//! [`ValueRef::to_data`] and [`ValueRef::to_wire`] read plain data without running JS:
//! own enumerable data properties of plain objects (prototype `Object.prototype` or
//! `null`), arrays, `Uint8Array`s and primitives. Anything else is refused with
//! [`Error::Data`] naming the path (`cannot encode a function at $.headers[2]`): accessors,
//! proxies, class instances, `Map`, `Date`, symbols, BigInts, cycles. The wire format is
//! djs's binary encoding of the same values (docs/wire.md in the repository):
//! [`Value::Wire`] passes bytes a host already has, and [`Data::to_wire`] /
//! [`Data::from_wire`] convert.
//!
//! Into the VM, [`Value::Data`] builds the tree in the heap directly, and
//! [`Value::Array`] / [`Value::Object`] do the same from borrowed parts: a host passes a
//! request head made of the strings it already has, without building a [`Data`] first.
//! [`Value::Utf8`] makes a string from bytes (a body), replacing invalid UTF-8.
//!
//! Bytes are always copied into the VM: the VM's heap is one arena it owns, so a
//! `Uint8Array` cannot point at host memory.
//!
//! # Failures
//!
//! Every outcome that is not a settled value is an [`Error`], and the call is then over:
//! the VM is idle and the instance usable for the next call (reset it if the failed call
//! may have left half-done state in its globals).
//!
//! | outcome | error |
//! | --- | --- |
//! | the function throws, or its promise rejects | [`Error::Exception`] |
//! | the call's fuel is used up | [`Error::OutOfFuel`] |
//! | the deadline passed (checked before each slice) | [`Error::Deadline`] |
//! | the heap reached [`Limits::memory`] | [`Error::OutOfMemory`] |
//! | nothing left to run, no host call pending, the promise still pending | [`Error::NeverSettles`] |
//! | a result that is not plain data, corrupt wire bytes | [`Error::Data`] |
//! | API misuse (unknown function or call id, a call during a call) | [`Error::Usage`] |
//! | a panic inside djs | [`Error::Panicked`], then [`Error::Poisoned`] |
//!
//! Scripts cannot catch the budgets: running out of fuel or past the deadline stops the
//! VM between instructions, and reaching the memory cap ends the run before any `catch`
//! or `finally` block runs. Aborting a call drops its frames, its pending host calls, its
//! queued jobs and its delivered but unapplied results; the realm and the globals stay as
//! the call left them.
//!
//! Every entry into djs runs under `catch_unwind`. A panic is a bug in djs; it poisons
//! only that instance (every later use returns [`Error::Poisoned`]: drop it), never the
//! snapshot or other instances: djs keeps no global mutable state, everything a VM has
//! lives in its own heap. This needs the default `panic = "unwind"`; with
//! `panic = "abort"` a djs bug ends the process. The panic hook still runs (and prints,
//! with the default hook).
//!
//! # Threads
//!
//! [`Snapshot`] is `Send + Sync`: share one across threads. An [`Instance`] is `Send`:
//! it can move between threads (an async task that awaits host work between steps) but
//! serves one call at a time. Parallel calls take one instance each.
//!
//! # Depending on djs-embed
//!
//! djs publishes no crates yet: depend on the repository, by path while developing both
//! and by a pinned git revision otherwise.
//!
//! ```toml
//! [dependencies]
//! # local development, next to a djs checkout:
//! # djs-embed = { path = "../djs/crates/djs-embed" }
//! djs-embed = { git = "https://github.com/earendil-works/djs", rev = "<commit>" }
//!
//! # djs-core at opt-level 1 in debug builds: at 0 its calls run several times slower
//! [profile.dev.package.djs-core]
//! opt-level = 1
//! ```
//!
//! - **Features.** The only one is `unicode` (default): `\p{..}` in regular expressions and
//!   `String.prototype.normalize`, about 60 KB of native code;
//!   `default-features = false` leaves it out. djs-embed picks djs-core's features itself
//!   (every builtin, nothing a host does not use: no dumps, lazy compilation or compile
//!   cache) and turns djs-prelude's builtin registry off.
//! - **Runtime crates.** `cargo tree -e normal` gives djs-embed, djs-core, djs-prelude
//!   and djs-libm (djs's copy of libm's math functions): no third-party crate.
//! - **Git checkouts.** The crates read files elsewhere in the repository at build time
//!   (the builtins' compiled JavaScript in `builtins/gen/`); cargo's git checkout is the
//!   whole repository, so they resolve. The build script of djs-prelude makes the base
//!   image (the realm every snapshot starts from) on the build machine.
//! - **Cross builds.** djs-embed builds for any target djs-core does, with nothing but a
//!   linker for it: no C code, no target-specific build step. The base image a cross build
//!   embeds is byte for byte the one the target makes itself (heap cells are little-endian
//!   with 32-bit offsets on every target). `scripts/cross-check.sh` in the repository
//!   builds and runs an example for aarch64 and x86_64 macOS and linux-musl (with `zig cc`
//!   as the musl linker) and compares their output.
//! - **Panics** must unwind (the default profile setting), see Failures.

mod data;
mod error;
mod instance;
mod module;
mod park;
mod snapshot;
mod value;

#[cfg(test)]
mod tests;

pub use data::Data;
pub use error::{Error, ErrorKind, Exception, Throw};
pub use instance::{CallId, HostCall, Instance, Limits, Step};
pub use module::{Loader, ModuleMap, ModuleSource, SyntheticModule};
pub use park::Parked;
pub use snapshot::{Builder, HostFn, Snapshot, Source, SourceKind};
pub use value::{Value, ValueRef};

const _: () = {
    const fn send<T: Send>() {}
    send::<Instance>();
};
