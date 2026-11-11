//! What is in flight when a VM is idle (0172, docs/continuation-dumps.md, D2561, D2562,
//! D2610): open host calls, pending promises with their reactions, suspended
//! activations and the iterators of their loops, written as the calls that recreate
//! them.
//!
//! - An open call is `$dump.pending(id, name, args)`, by the name the host gives its
//!   function (`DumpOptions::host_fn_name`), with the host's JSON state of the call as a
//!   fourth argument when it gave one (`DumpOptions::call_states`, D2770).
//! - A pending promise is written by what made it: an open call, an activation (its
//!   result promise), a `then` on another promise (its derived promise), a combinator
//!   (`Promise.all` ...), or `new Promise` whose resolving functions the dump hands to
//!   whoever held them; else `new Promise(() => {})`, which never settles, as nothing can
//!   settle it.
//! - Its reactions are written in their order, each as the call that adds it: an await
//!   is the awaiting activation's `$dump.resume`, a `then`/`catch`/`finally` the call
//!   again, a combinator element the combinator called again with the elements that
//!   settled in place. A dump whose dependencies would add them in another order is
//!   refused.
//! - An activation is `$dump.resume(fn, {...})` with the slots its function's suspension
//!   map names at the site where it stopped (bindings, hidden locals, temporaries of the
//!   constructs in flight, D2731); the map comes from compiling the function's script
//!   again, with the options it was compiled with.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::builtins::regexp;
use crate::compiler::{FunctionMap, Holds, NamedSlot, Place, Site, SiteKind, Slot};
use crate::heap::cells::{HostCall, Script};
use crate::interp::realm::{Intrinsic, intrinsic_obj};
use crate::interp::run::{builtin_closure_kind, promise, suspend};

#[path = "dump_flight_helpers.rs"]
mod helpers;
#[path = "dump_flight_runtime.rs"]
mod runtime;

/// The resource Record of `using` declarations (builtins/disposable.ts `UsingCreate`):
/// its slot count, and its slots by name (slot 2 is the resources, an Array of `[value,
/// method, kind]` triples).
const USING_SLOTS: u32 = 11;
const USING_FIELDS: [(&str, u32); 8] = [
    ("error", 3),
    ("hasError", 4),
    ("needsAwait", 5),
    ("hasAwaited", 6),
    ("next", 7),
    ("value", 8),
    ("pending", 9),
    ("hasPending", 10),
];

/// The prototype an iterator helper is made with: %WrapForValidIteratorPrototype% for
/// `Iterator.from`'s wrapper (kind 0), else %IteratorHelperPrototype%.
fn helper_proto(h: &crate::heap::Heap, o: Obj) -> Value {
    if h.object_slot(o, 0).as_int() == Some(0) {
        Value::object(intrinsic_obj(h, Intrinsic::WrapForValidIteratorProto))
    } else {
        Value::object(intrinsic_obj(h, Intrinsic::IteratorHelperProto))
    }
}

/// Builtin closure kinds (`NewBuiltinClosure`, interp/run/intrinsics.rs).
const RESOLVE: usize = 0;
const REJECT: usize = 1;
const ALL_ELEMENT: usize = 3;
const SETTLED_FULFILLED: usize = 4;
const SETTLED_REJECTED: usize = 5;
const ANY_REJECT: usize = 6;
const THEN_FINALLY: usize = 7;
const CATCH_FINALLY: usize = 8;
const VALUE_THUNK: usize = 9;
const THROWER: usize = 10;
const FROM_SYNC_UNWRAP: usize = 13;
const FROM_SYNC_CLOSE: usize = 14;
/// Array.fromAsync's closure resumed after an await (builtins/fromasync.ts): its Record is
/// `[promise, mapfn, thisArg, iterator, next, A, k, phase, arrayLike, len, error]`.
const FROM_ASYNC_FULFILLED: usize = 16;
const FROM_ASYNC_REJECTED: usize = 17;
/// The slots of Array.fromAsync's Record.
const FROM_ASYNC_SLOTS: u32 = 11;

/// A for-in loop's key iterator (interp/run.rs `for_in_start`): its object (`undefined`
/// for a string's or a primitive's) and the keys it has yet to visit. `None`: a Proxy's
/// (a Record its traps walk).
fn for_in_parts(h: &Heap, v: Value) -> Option<(Value, Vec<Value>)> {
    if let Some(o) = v.as_object() {
        // A plain dump's `$dump.forIn(o, keys)` (a continuation's loop, origin
        // tracking): its `next` closes over the object, the keys and the index.
        let next = super::origin::own(h, o, "next")?.as_object()?;
        let target = captured_value(h, next, "o")?;
        let keys = super::origin::elements(h, captured_value(h, next, "keys")?)?;
        let i = captured_value(h, next, "i")?.as_number()?;
        let i = (i.max(0.0) as usize).min(keys.len());
        return Some((target, keys[i..].to_vec()));
    }
    let arr = v.as_internal()?;
    if h.cell_kind(arr) != crate::heap::CellKind::ValueArray {
        return None;
    }
    let len = h.value_array_len(arr);
    let next = h.value_array_get(arr, 1).as_int().unwrap_or(0).max(0) as u32;
    let keys = (2 + next..len).map(|i| h.value_array_get(arr, i)).collect();
    Some((h.value_array_get(arr, 0), keys))
}

pub(super) struct Flight<'a> {
    pub host_fn_name: &'a dyn Fn(u32) -> Option<String>,
    calls: Vec<Call>,
    handles: Vec<(String, Value)>,
    pends: Vec<Pend>,
    acts: Vec<Act>,
    combos: Vec<Combo>,
    clos: Vec<Clo>,
    /// Suspension maps by script cell.
    /// `(script, maps, partial)` (`Dumper::maps_entry`).
    #[allow(clippy::type_complexity)]
    maps: Vec<(
        u32,
        Option<alloc::sync::Arc<Vec<FunctionMap>>>,
        bool,
        super::ScriptGlobals,
    )>,
    /// The templates of lazy functions, for the maps a lazy load did not record (D2994):
    /// built at the first need.
    lazy: Option<LazyIndex>,
    /// Scripts the VM's cache has no entry for (`recorded_globals`).
    #[cfg_attr(not(feature = "lazy"), allow(dead_code))]
    uncached: Vec<u32>,
    /// Promises in the order the dump made them: the end writes their other reactions.
    made: Vec<usize>,
    /// The activations written, where they stopped (`Vm::dump_report`).
    sites: Vec<ResumedSite>,
    /// Activations whose function uses `super`: the prototype of its home object.
    supers: Vec<(usize, Value)>,
    /// Plain dumps: the `using` records written, by their text (`Act::usings`).
    usings: Vec<plain::Using>,
    /// Closures of the dump runtime written as the engine's builtin closures
    /// (dump_flight_runtime.rs): their cell, the closure kind and its data.
    runtime: Vec<(u32, usize, Value)>,
    /// The states of Array.fromAsync's in flight that the dump runtime's `fromAsync`
    /// holds (its closures' data is the index here): the Record's slots.
    from_async: Vec<[Value; FROM_ASYNC_SLOTS as usize]>,
}

impl<'a> Flight<'a> {
    pub fn new(host_fn_name: &'a dyn Fn(u32) -> Option<String>) -> Flight<'a> {
        Flight {
            host_fn_name,
            calls: Vec::new(),
            handles: Vec::new(),
            pends: Vec::new(),
            acts: Vec::new(),
            combos: Vec::new(),
            clos: Vec::new(),
            maps: Vec::new(),
            lazy: None,
            uncached: Vec::new(),
            made: Vec::new(),
            sites: Vec::new(),
            supers: Vec::new(),
            usings: Vec::new(),
            runtime: Vec::new(),
            from_async: Vec::new(),
        }
    }

    /// Open calls, activations, pending promises.
    /// The highest id of the open calls the dump reopens (0 if none).
    pub fn last_call_id(&self) -> u32 {
        self.calls.iter().map(|c| c.id).max().unwrap_or(0)
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.calls.len(), self.acts.len(), self.pends.len())
    }

    pub fn take_sites(&mut self) -> Vec<ResumedSite> {
        core::mem::take(&mut self.sites)
    }
}

struct Call {
    id: u32,
    name: String,
    args: Vec<Value>,
    promise: Value,
    /// The host's state of the call, JSON text checked by `dump_state::check`.
    state: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Creator {
    Unknown,
    Call(usize),
    Act(usize),
    /// The derived promise of reaction `j` of pending promise `p`.
    Then(usize, usize),
    Combo(usize),
    /// `new Promise`, handing out its resolving functions.
    Resolvers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Consumer {
    Then,
    /// An element of a combinator whose promise settled already: its function does
    /// nothing any more, a reaction with handlers that do nothing.
    Dead,
    Await(usize),
    Finally,
    /// Array.fromAsync's await: written as `$djs.fromAsync(…)`, which makes its promise.
    FromAsync,
    Element(usize),
}

#[derive(Clone, Copy)]
struct Reaction {
    cap: Value,
    f: Value,
    r: Value,
    consumer: Consumer,
    busy: bool,
}

struct Pend {
    node: usize,
    reactions: Vec<Reaction>,
    creator: Creator,
    /// The reactions the dump added so far.
    cursor: usize,
    made: bool,
    busy: bool,
    /// The name of its resolving functions (`Creator::Resolvers`).
    resolvers: String,
}

struct Act {
    node: usize,
    s: suspend::Suspended,
    site: Option<Site>,
    /// The syntax around its function's sites (plain dumps).
    syntax: Option<Arc<crate::compiler::Syntax>>,
    /// The awaited promise and the reaction's index there.
    awaiting: Option<(usize, usize)>,
    state: State,
}

struct Combo {
    /// The closure kind of its elements: ALL_ELEMENT, SETTLED_*, ANY_REJECT.
    kind: usize,
    cap: Value,
    values: Value,
    remaining: Value,
    /// Pending elements: index, promise, reaction.
    elements: Vec<(u32, usize, usize)>,
    state: State,
}

/// A builtin closure: its kind and data.
struct Clo {
    node: usize,
    kind: usize,
    data: Value,
}

impl Dumper<'_> {
    // ---------------------------------------------------------------- roots

    /// The roots of what is in flight: the open host calls and the host's handles. A
    /// dump needs an idle VM (D2561).
    pub(super) fn flight_roots(&mut self, opts: &DumpOptions<'_>) {
        let vm = self.vm;
        let h = self.h;
        use crate::interp::RunState;
        if !matches!(
            vm.state(),
            RunState::Idle | RunState::Done | RunState::Threw
        ) {
            self.refuse("the VM".into(), "is not idle (a call is running or parked)");
        } else if promise::has_jobs(h) {
            self.refuse("the VM".into(), "is not idle (jobs are queued)");
        } else if promise::has_inbox(h) {
            self.refuse(
                "the VM".into(),
                "is not idle (a host result was delivered and not applied yet)",
            );
        }
        let open = vm.pending_host_calls();
        for &id in &open {
            let Some(info) = vm.host_call(id) else {
                continue;
            };
            let path = cat!("<call ", id, ">");
            let Some(name) = (self.fl.host_fn_name)(info.fn_id) else {
                self.refuse(path, "a call of a host function the host does not name");
                continue;
            };
            let promise = crate::interp::run::host_call_cell(h, id)
                .map_or(Value::UNDEFINED, |c| h.arena().get(c, HostCall::PROMISE));
            for (i, &a) in info.args.iter().enumerate() {
                self.note(a, || cat!(path, ".args[", i, "]"));
            }
            self.note(promise, || path.clone());
            let mut state = None;
            if let Some(&(_, json)) = opts.call_states.iter().find(|(c, _)| *c == id) {
                match crate::vm::dump_state::check(json) {
                    Ok(()) => state = Some(String::from(json)),
                    Err(why) => self.refuse(cat!(path, ".state"), why),
                }
            }
            self.fl.calls.push_out(Call {
                id,
                name,
                args: info.args,
                promise,
                state,
            });
        }
        for &(id, _) in opts.call_states {
            if !open.contains(&id) {
                self.refuse(
                    cat!("<call ", id, ">"),
                    "a state for a call that is not open",
                );
            }
        }
        for &(name, v) in opts.handles {
            self.note(v, || cat!("<handle ", name, ">"));
            self.fl.handles.push_out((String::from(name), v));
        }
    }

    // ---------------------------------------------------------------- discovery

    pub(super) fn visit_pending(&mut self, n: usize, o: Obj) {
        let h = self.h;
        let path = self.nodes[n].path.clone();
        let mut reactions = Vec::new();
        if let Some(r) = h.object_slot(o, 2).as_internal() {
            for j in 0..h.value_array_len(r) / 3 {
                let get = |k| h.value_array_get(r, 3 * j + k);
                let (mut cap, f, mut rj) = (get(0), get(1), get(2));
                let cell = |v: Value| v.as_object().map_or(u32::MAX, |o| o.offset());
                if let Some(state) = self.runtime_from_async(f, rj)
                    && self.node_of(f).is_none()
                    && self.node_of(rj).is_none()
                {
                    // The dump runtime's `fromAsync` in flight: as the engine's.
                    let i = Value::int(self.fl.from_async.len() as i32);
                    self.fl.from_async.push(state);
                    self.fl.runtime.push_out((cell(f), FROM_ASYNC_FULFILLED, i));
                    self.fl.runtime.push_out((cell(rj), FROM_ASYNC_REJECTED, i));
                } else if let Some(step) = self.runtime_step(f, rj)
                    && self.node_of(f).is_none()
                    && self.node_of(rj).is_none()
                {
                    // A step of the dump runtime's `fromSync` in flight: the step's
                    // promise, made by a `then` with the engine's handlers.
                    cap = step.promise;
                    self.fl
                        .runtime
                        .push_out((cell(f), FROM_SYNC_UNWRAP, step.done));
                    match step.close {
                        Some(it) => self.fl.runtime.push_out((cell(rj), FROM_SYNC_CLOSE, it)),
                        None => rj = Value::UNDEFINED,
                    }
                }
                let p = cat!(path, "<reaction ", j, ">");
                self.edge(n, cap, || cat!(p, ".promise"));
                self.edge(n, f, || cat!(p, ".onFulfilled"));
                if rj != f {
                    self.edge(n, rj, || cat!(p, ".onRejected"));
                }
                reactions.push_out(Reaction {
                    cap,
                    f,
                    r: rj,
                    consumer: Consumer::Then,
                    busy: false,
                });
            }
        }
        self.nodes[n].fl = self.fl.pends.len();
        self.fl.pends.push_out(Pend {
            node: n,
            reactions,
            creator: Creator::Unknown,
            cursor: 0,
            made: false,
            busy: false,
            resolvers: String::new(),
        });
    }

    /// The suspension maps of the script of template `t`, compiled again in the scratch
    /// heap with the options it was compiled with.
    /// With a partial set (a plain dump loaded lazily, D2994) the map of `t`'s function
    /// is added if it is missing.
    pub(super) fn script_maps(&mut self, t: u32) -> Option<&[FunctionMap]> {
        let h = self.h;
        let info = template::debug_info(h, t);
        let script = info.script.get()?;
        let i = match self.fl.maps.iter().position(|m| m.0 == script) {
            Some(i) => i,
            None => {
                let (src, params, bits) = script_inputs(h, script);
                let (maps, partial, globals) = match src {
                    Some(src) => self.maps_entry(&src, &params, bits),
                    None => (None, false, Default::default()),
                };
                self.fl.maps.push_out((script, maps, partial, globals));
                self.fl.maps.len() - 1
            }
        };
        let (a, b) = (info.src_start, info.src_end);
        if self.fl.maps[i].2
            && !self.fl.maps[i]
                .1
                .as_ref()
                .is_some_and(|m| m.iter().any(|m| m.start == a && m.end == b))
        {
            self.complete_maps(i, a, b);
        }
        self.fl.maps[i].1.as_deref().map(Vec::as_slice)
    }

    /// Adds the maps of the function at `a..b` to the partial set `fl.maps[i]`: of the
    /// lazy template whose compile made its code (the innermost one around it) and of the
    /// functions inside it, compiled again on their own (D2994). Without one, all of the
    /// script's maps from an eager compile.
    fn complete_maps(&mut self, i: usize, a: u32, b: u32) {
        let h = self.h;
        let script = self.fl.maps[i].0;
        let (src, params, bits) = script_inputs(h, script);
        let Some(src) = src else { return };
        let root = self.lazy_index().root(script, a, b);
        #[cfg(feature = "lazy")]
        let more = root.and_then(|t| {
            let heap = self.scratch_heap()?;
            crate::compiler::lazy_function_maps(h, Ref(t), heap).ok()
        });
        #[cfg(not(feature = "lazy"))]
        let more: Option<Vec<FunctionMap>> = root.and(None);
        let (maps, partial) = match more {
            Some(more) => {
                let mut all: Vec<FunctionMap> =
                    self.fl.maps[i].1.as_deref().cloned().unwrap_or_default();
                for m in more {
                    if !all.iter().any(|x| x.start == m.start && x.end == m.end) {
                        all.push(m);
                    }
                }
                (Some(alloc::sync::Arc::new(all)), true)
            }
            None => (self.compile_maps(&src, &params, bits), false),
        };
        self.fl.maps[i].1 = maps.clone();
        self.fl.maps[i].2 = partial;
        let globals = self.fl.maps[i].3.clone();
        self.set_maps(&src, &params, bits, maps, partial, globals);
    }

    /// Makes sure the maps of the function that made closure template `t` (the one whose
    /// code evaluates its computed keys) are there, for a partial set.
    pub(super) fn creator_maps(&mut self, t: u32) {
        let h = self.h;
        let Some(script) = template::debug_info(h, t).script.get() else {
            return;
        };
        if !self
            .fl
            .maps
            .iter()
            .find(|m| m.0 == script)
            .is_none_or(|m| m.2)
        {
            return;
        }
        if let Some(p) = self.lazy_index().parent(t) {
            self.script_maps(p);
        }
    }

    /// The globals a lazy load recorded for the lazy function at `a..b` of `script`
    /// (D2993), if the VM's cache has that script's entry (looked up once per script).
    #[cfg(feature = "lazy")]
    pub(super) fn recorded_globals(&mut self, script: u32, a: u32, b: u32) -> Option<Vec<String>> {
        let i = match self.fl.maps.iter().position(|m| m.0 == script) {
            Some(i) => i,
            None => {
                if self.fl.uncached.contains(&script) {
                    return None;
                }
                let (src, params, bits) = script_inputs(self.h, script);
                let found = src.and_then(|src| {
                    let cache = self.vm.dump_cache.borrow();
                    cache
                        .maps
                        .iter()
                        .find(|m| m.0 == bits && m.1 == params && m.2 == src)
                        .map(|m| (m.3.clone(), m.4, m.5.clone()))
                });
                let Some((maps, partial, globals)) = found else {
                    self.fl.uncached.push_out(script);
                    return None;
                };
                self.fl.maps.push_out((script, maps, partial, globals));
                self.fl.maps.len() - 1
            }
        };
        self.fl.maps[i]
            .3
            .iter()
            .find(|g| g.0 == a && g.1 == b)
            .map(|g| g.2.clone())
    }

    fn lazy_index(&mut self) -> &LazyIndex {
        let h = self.h;
        self.fl.lazy.get_or_insert_with(|| LazyIndex::new(h))
    }

    pub(super) fn visit_activation(&mut self, n: usize, g: Obj) {
        let h = self.h;
        let path = self.nodes[n].path.clone();
        let mut s = suspend::read_activation(h, g);
        self.nodes[n].fl = self.fl.acts.len();
        let mut syntax = None;
        // The function the frame runs (a continuation's, not its origin's).
        let runs = s.frame.first().copied().unwrap_or(Value::UNDEFINED);
        let site = match s.state {
            suspend::AWAITING | suspend::SUSPENDED | suspend::SUSPENDED_RAW => {
                match self.activation_site(&path, &s) {
                    Some((site, sx)) => match self.origin_frame(&s, &site) {
                        Ok(Some((fs, fsite, fsx))) => {
                            s = fs;
                            syntax = Some(fsx);
                            Some(fsite)
                        }
                        Ok(None) => {
                            syntax = Some(sx);
                            Some(site)
                        }
                        Err(why) => {
                            self.refuse(path.clone(), &why);
                            None
                        }
                    },
                    None => None,
                }
            }
            suspend::COMPLETED => {
                // A finished generator of the same kind and prototype.
                let proto = h.get_prototype_of(g).map_or(Value::NULL, Value::object);
                self.edge(n, proto, || cat!(path, ".[[Prototype]]"));
                None
            }
            suspend::START => {
                // Called again: its parameters must run no code.
                let simple = s
                    .frame
                    .first()
                    .and_then(|f| f.as_object())
                    .and_then(|f| self.sources.fn_text(h, f))
                    .is_some_and(|src| simple_params(&src));
                if !simple {
                    self.refuse(
                        path.clone(),
                        "a generator that has not started or has finished, whose parameters run code",
                    );
                }
                let fp = |i: usize| s.frame.get(i).copied().unwrap_or(Value::UNDEFINED);
                let argc = s
                    .frame
                    .get(s.hp as usize + crate::interp::frame::ARGC as usize)
                    .and_then(|v| v.as_int())
                    .unwrap_or(0) as usize;
                self.edge(n, fp(0), || cat!(path, "<function>"));
                self.edge(n, fp(1), || cat!(path, "<this>"));
                for i in 0..argc {
                    self.edge(n, fp(2 + i), || cat!(path, "<args>[", i, "]"));
                }
                self.check_generator_proto(&path, g, fp(0));
                None
            }
            _ => {
                self.refuse(path.clone(), "a generator in the middle of a yield*");
                None
            }
        };
        if site.is_some() && s.kind != suspend::ASYNC {
            self.check_generator_proto(&path, g, runs);
            // Its prototype is its continuation's (`Act::proto`).
            let proto = h.get_prototype_of(g).map_or(Value::NULL, Value::object);
            self.edge(n, proto, || cat!(path, ".[[Prototype]]"));
        }
        // What `super[...]` read before the site: its continuation reads `super` again.
        let mut super_read: Vec<(bool, Value, Value)> = Vec::new();
        if let Some(site) = &site {
            let hp = s.hp as usize;
            let argc = s
                .frame
                .get(hp + crate::interp::frame::ARGC as usize)
                .and_then(|v| v.as_int())
                .unwrap_or(0) as usize;
            let lp = hp + crate::interp::frame::HEADER as usize;
            let fp = |i: usize| s.frame.get(i).copied().unwrap_or(Value::UNDEFINED);
            self.edge(n, fp(0), || cat!(path, "<function>"));
            self.edge(n, fp(1), || cat!(path, "<this>"));
            for i in 0..argc {
                self.edge(n, fp(2 + i), || cat!(path, "<args>[", i, "]"));
            }
            // Parameters: written as bindings too (and past the arguments given,
            // assigned since).
            for (i, slot) in site.args.iter().enumerate() {
                if let Slot::Binding { name, .. } = slot {
                    self.edge(n, fp(2 + i), || cat!(path, ".", name));
                }
            }
            for (i, slot) in site.locals.iter().enumerate() {
                let name = match slot {
                    Slot::Binding { name, .. } => name.as_str(),
                    Slot::Hidden(name) if !computed_hidden(name) && !class_hidden(name) => {
                        name.as_str()
                    }
                    // A `this` its arrows share (a box): the frame's, K's `this` (0215).
                    Slot::Hidden(name)
                        if matches!(name.as_str(), "this" | "new.target")
                            && fp(lp + i).as_internal().is_some() =>
                    {
                        name.as_str()
                    }
                    _ => continue,
                };
                let v = fp(lp + i);
                if v.is_uninitialized()
                    || syntax
                        .as_deref()
                        .is_some_and(|sx| undeclared_at(sx, site, slot, v))
                {
                    // A binding in its TDZ: written as `$dump.TDZ` (`slot_expr`).
                    continue;
                }
                // A box's path ends in `~name`: its factory's variable.
                let sep = if v.as_internal().is_some() { "~" } else { "." };
                if let Some(b) = self.edge(n, v, || cat!(path, sep, name))
                    && v.as_internal().is_some()
                {
                    self.nodes[b].frame_box = true;
                    // Its arrows read it as K's `$djs_arguments` (`fn_source`).
                    if name == "arguments"
                        && let Some(f) = fp(0).as_object()
                        && let Some(sx) = syntax.as_deref()
                    {
                        let d = template::debug_info(h, h.closure_template(f));
                        let offs = sx
                            .special
                            .iter()
                            .filter(|s| s.0 == crate::compiler::Special::Arguments)
                            .map(|s| s.1)
                            .collect();
                        self.arg_boxes.push_out((b, d.script, d.src_start, offs));
                    }
                }
            }
            // The construct temporaries, in locals of their own and on the stack.
            let named = site.named_slots();
            let at = |p: Place| match p {
                Place::Arg(i) => 2 + usize::from(i),
                Place::Local(i) => lp + usize::from(i),
                Place::Stack(k) => lp + site.locals.len() + usize::from(k),
            };
            for slot in named.iter().filter(|x| matches!(x.holds, Holds::Temp)) {
                let v = fp(at(slot.place));
                match slot.construct() {
                    c @ ("super.base" | "super.this") => {
                        super_read.push_out((c == "super.base", v, fp(1)));
                    }
                    "for-in.iterator" => {
                        // The key iterator: its object (its keys are strings).
                        match for_in_parts(h, v) {
                            Some((target, _)) => {
                                self.edge(n, target, || cat!(path, "<for-in.target>"));
                            }
                            None => self.refuse(path.clone(), "a for-in loop over a Proxy"),
                        }
                        continue;
                    }
                    "for-await.next" | "yield*.next"
                        if from_sync_partner(&named, slot)
                            .map(|p| fp(at(p)))
                            .and_then(|v| v.as_object())
                            .is_some_and(|o| self.is_from_sync(o)) =>
                    {
                        // The dump's own iterator brings its `next`.
                        continue;
                    }
                    _ => {}
                }
                let e = self.edge(n, v, || cat!(path, "<", slot.name, ">"));
                if slot.construct() == "using.record"
                    && let Some(e) = e
                {
                    let ok = v.as_object().is_some_and(|o| {
                        h.object_class(o) == ObjectClass::Record
                            && h.get_prototype_of(o).is_none()
                            && h.object_slot_count(o) == USING_SLOTS
                    });
                    if ok {
                        self.nodes[e].using_record = true;
                    } else {
                        self.refuse(
                            path.clone(),
                            "a using declaration's record of another shape",
                        );
                    }
                }
            }
            if s.kind == suspend::ASYNC {
                self.edge(n, s.promise, || cat!(path, "<promise>"));
            }
            for (k, &r) in s.queue.iter().enumerate() {
                if let Some(r) = r.as_object() {
                    self.edge(n, h.object_slot(r, 1), || {
                        cat!(path, "<queue ", k, ">.value")
                    });
                    self.edge(n, h.object_slot(r, 2), || cat!(path, "<queue ", k, ">"));
                }
            }
        }
        // A method's `super`: its home object's prototype, which its continuation's
        // home object gets.
        if syntax.as_ref().is_some_and(|sx| {
            sx.special
                .iter()
                .any(|x| x.0 == crate::compiler::Special::Super)
        }) && let Some(home) = runs
            .as_object()
            .and_then(|rf| home_object(h, rf))
            // (A continuation that uses no `super` (it resumes the call that did) has no
            // home: its origin's, the frame's function now, 0215.)
            .or_else(|| {
                s.frame
                    .first()
                    .and_then(|f| f.as_object())
                    .and_then(|f| home_object(h, f))
            })
        {
            let p = h.get_prototype_of(home).map_or(Value::NULL, Value::object);
            if super_read
                .iter()
                .any(|&(base, v, this)| v != if base { p } else { this })
            {
                self.refuse(
                    path.clone(),
                    "a super base or this the method no longer has",
                );
            }
            self.edge(n, p, || cat!(path, "<super>"));
            self.fl.supers.push_out((self.fl.acts.len(), p));
        }
        self.fl.acts.push(Act {
            node: n,
            s,
            site,
            syntax,
            awaiting: None,
            state: State::New,
        });
    }

    /// A generator object whose prototype is not its function's `prototype` (which
    /// `$dump.resume` and a new call give it) is refused.
    fn check_generator_proto(&mut self, path: &str, g: Obj, f: Value) {
        let h = self.h;
        let want = f
            .as_object()
            .and_then(|f| h.get_own_property(f, crate::heap::atoms::PROTOTYPE))
            .and_then(|p| p.value())
            .and_then(|v| v.as_object());
        if h.get_prototype_of(g) != want || !h.own_keys(g).is_empty() {
            self.refuse(
                path.into(),
                "a generator object with another prototype or properties of its own",
            );
        }
    }

    /// The site where activation `s` stopped, checked against its frame.
    fn activation_site(
        &mut self,
        path: &str,
        s: &suspend::Suspended,
    ) -> Option<(Site, Arc<crate::compiler::Syntax>)> {
        let h = self.h;
        let Some(f) = s.frame.first().and_then(|v| v.as_object()) else {
            self.refuse(path.into(), "an activation without its function");
            return None;
        };
        let t = h.closure_template(f);
        let info = template::debug_info(h, t);
        let (a, b) = (info.src_start, info.src_end);
        let params = usize::from(h.arena().get::<u16>(t, FuncTemplate::PARAMS));
        let Some(maps) = self.script_maps(t) else {
            self.refuse(
                path.into(),
                "an activation whose script cannot be compiled again",
            );
            return None;
        };
        let found = maps
            .iter()
            .find(|m| m.start == a && m.end == b)
            .and_then(|m| {
                m.sites
                    .iter()
                    .find(|st| st.resume_pc == s.pc)
                    .map(|st| (st.clone(), m.syntax.clone()))
            });
        let Some((site, syntax)) = found else {
            self.refuse(
                path.into(),
                "an activation stopped where its map has no site",
            );
            return None;
        };
        if let Some(u) = &site.unsupported {
            let why = cat!(
                "an activation at ",
                site.name,
                ", which dumps cannot resume yet: ",
                u
            );
            self.refuse(path.into(), &why);
            return None;
        }
        let kind = func_flags::kind(u32::from(h.arena().get::<u16>(t, FuncTemplate::FLAGS)));
        let (site, syntax) = if kind == func_flags::KIND_ASYNC_BODY {
            self.body_site(path, f, site)?
        } else {
            (site, syntax)
        };
        let at_yield = matches!(
            site.kind,
            SiteKind::Yield | SiteKind::AsyncYield | SiteKind::YieldStar
        );
        let hp = s.hp as usize;
        let argc = s
            .frame
            .get(hp + crate::interp::frame::ARGC as usize)
            .and_then(|v| v.as_int())
            .unwrap_or(-1);
        let fits = argc >= 0
            && hp == 2 + params.max(argc as usize)
            && site.args.len() == params
            && s.frame.len()
                == hp
                    + crate::interp::frame::HEADER as usize
                    + site.locals.len()
                    + site.stack.len()
            && at_yield == matches!(s.state, suspend::SUSPENDED | suspend::SUSPENDED_RAW);
        if !fits {
            self.refuse(
                path.into(),
                "an activation whose frame does not fit its map",
            );
            return None;
        }
        Some((site, Arc::new(syntax)))
    }

    /// A codemode body's site as the loader sees it: the dump writes the body as an
    /// async function (`fn_text`), whose map must have the same site with the same
    /// slots. Its bindings' scopes are the function's (they are offsets
    /// from its start); its position stays the body's, for the report.
    fn body_site(
        &mut self,
        path: &str,
        f: Obj,
        site: Site,
    ) -> Option<(Site, crate::compiler::Syntax)> {
        let h = self.h;
        let t = h.closure_template(f);
        let bits = template::debug_info(h, t)
            .script
            .get()
            .map_or(0, |s| h.arena().get::<u32>(s, Script::OPTIONS));
        let text = self.sources.fn_text(h, f).map(|s| cat!("(", s, ")"));
        // The dump's goal (a Script) with the body's TypeScript options.
        let maps = text.and_then(|text| {
            let len = text.len() as u32;
            self.maps_of(&text, "", bits & !3).map(|m| (m, len))
        });
        let wrapped = maps.and_then(|(maps, len)| {
            maps.iter()
                .find(|m| m.start == 1 && m.end == len - 1)
                .and_then(|m| {
                    m.sites
                        .iter()
                        .find(|s| s.name == site.name)
                        .map(|s| (s.clone(), m.syntax.clone()))
                })
        });
        let names = |slots: &[Slot]| -> Vec<Slot> {
            slots
                .iter()
                .map(|s| match s {
                    Slot::Binding { name, .. } => Slot::Binding {
                        name: name.clone(),
                        scope: 0,
                    },
                    // (Offsets are from the function's start, which differs.)
                    Slot::Temp(n) => Slot::Temp(n.split('@').next().unwrap_or("").into()),
                    s => s.clone(),
                })
                .collect()
        };
        let constructs = |stack: &[String]| -> Vec<String> {
            stack
                .iter()
                .map(|n| n.split('@').next().unwrap_or("").into())
                .collect()
        };
        match wrapped {
            Some((w, syntax))
                if w.kind == site.kind
                    && names(&w.args) == names(&site.args)
                    && names(&w.locals) == names(&site.locals)
                    && w.boxed == site.boxed
                    && constructs(&w.stack) == constructs(&site.stack)
                    && w.unsupported.is_none() =>
            {
                Some((Site { pos: site.pos, ..w }, syntax))
            }
            _ => {
                let why = cat!(
                    "a codemode body whose frame at ",
                    site.name,
                    " differs from the async function the dump writes for it"
                );
                self.refuse(path.into(), &why);
                None
            }
        }
    }

    /// An object the dump runtime made (dump_flight_runtime.rs), visited as what it
    /// stands for; false for any other.
    pub(super) fn visit_runtime_made(&mut self, n: usize, o: Obj) -> bool {
        let h = self.h;
        let path = self.nodes[n].path.clone();
        if h.object_class(o) == ObjectClass::Function {
            let v = Value::object(o);
            let clo = match self.fl.runtime.iter().find(|r| r.0 == o.offset()) {
                Some(&(_, kind, data)) => (kind, data),
                None => match self.runtime_close_throw(v) {
                    Some(it) => (FROM_SYNC_CLOSE, it),
                    None => return false,
                },
            };
            if matches!(clo.0, FROM_ASYNC_FULFILLED | FROM_ASYNC_REJECTED) {
                for v in self
                    .async_from_state(clo.1)
                    .unwrap_or([Value::UNDEFINED; FROM_ASYNC_SLOTS as usize])
                {
                    self.edge(n, v, || cat!(path, "<fromAsync>"));
                }
            } else {
                self.edge(n, clo.1, || cat!(path, "<data>"));
            }
            self.nodes[n].fl = self.fl.clos.len();
            self.fl.clos.push_out(Clo {
                node: n,
                kind: clo.0,
                data: clo.1,
            });
            return true;
        }
        let Some((it, next)) = self.runtime_from_sync(o) else {
            return false;
        };
        self.edge(n, it, || cat!(path, "<iterator>"));
        self.edge(n, next, || cat!(path, "<next>"));
        self.nodes[n].fl = 0;
        true
    }

    pub(super) fn visit_builtin_closure(&mut self, n: usize, o: Obj) {
        let h = self.h;
        let path = self.nodes[n].path.clone();
        let kind = builtin_closure_kind(h, o).unwrap_or(usize::MAX);
        let data = h.object_slot(o, 1);
        match kind {
            RESOLVE | REJECT | ALL_ELEMENT | SETTLED_FULFILLED | SETTLED_REJECTED | ANY_REJECT
            | THEN_FINALLY | CATCH_FINALLY | VALUE_THUNK | THROWER | FROM_SYNC_UNWRAP
            | FROM_SYNC_CLOSE => {
                self.edge(n, data, || cat!(path, "<data>"));
            }
            // Array.fromAsync waiting: its state, which `$djs.fromAsync` takes (0215).
            FROM_ASYNC_FULFILLED | FROM_ASYNC_REJECTED => match data.as_object() {
                Some(s) if h.object_class(s) == ObjectClass::Record => {
                    let st = self
                        .async_from_state(data)
                        .unwrap_or([Value::UNDEFINED; FROM_ASYNC_SLOTS as usize]);
                    // (An async-from-sync iterator brings its own `next`.)
                    let from_sync = st[3].as_object().is_some_and(|it| self.is_from_sync(it));
                    for (i, &v) in st.iter().enumerate() {
                        if i == 4 && from_sync {
                            continue;
                        }
                        self.edge(n, v, || cat!(path, "<fromAsync ", i, ">"));
                    }
                }
                _ => self.refuse(path, "an internal function of a builtin"),
            },
            _ => self.refuse(path, "an internal function of a builtin"),
        }
        self.nodes[n].fl = self.fl.clos.len();
        self.fl.clos.push_out(Clo {
            node: n,
            kind,
            data,
        });
    }

    pub(super) fn visit_iterator(&mut self, n: usize, o: Obj) {
        let h = self.h;
        let path = self.nodes[n].path.clone();
        if h.object_class(o) == ObjectClass::ArrayIterator {
            self.edge(n, h.object_slot(o, 0), || cat!(path, "<target>"));
        } else if h.object_class(o) == ObjectClass::IteratorHelper {
            for i in 0..h.object_slot_count(o) {
                self.edge(n, h.object_slot(o, i), || cat!(path, "<slot ", i, ">"));
            }
            if h.get_prototype_of(o).map(Value::object) != Some(helper_proto(h, o)) {
                self.refuse(path.clone(), "an iterator with another prototype");
            }
        } else if h.object_class(o) == ObjectClass::RegExpStringIterator {
            // (Its string is a primitive.)
            self.edge(n, h.object_slot(o, regexp::IT_REGEXP), || {
                cat!(path, "<regexp>")
            });
            if h.get_prototype_of(o).map(Value::object)
                != Some(intrinsic(h, Intrinsic::RegExpStringIteratorProto))
            {
                self.refuse(path.clone(), "an iterator with another prototype");
            }
        } else if let Some((entries, _)) = h.collection_iterator_position(o) {
            // (Written again if only the iterator holds the collection.)
            for (i, (k, v)) in h.collection_entries_of(entries).into_iter().enumerate() {
                self.edge(n, k, || cat!(path, "<entries>[", i, "]"));
                self.edge(n, v, || cat!(path, "<entries>[", i, "]"));
            }
        }
        let extra = self.h.own_keys(o).len();
        if extra > 0 {
            self.refuse(path, "an iterator with properties of its own");
        }
        self.nodes[n].fl = 0;
    }

    // ---------------------------------------------------------------- analysis

    fn pend_of(&self, v: Value) -> Option<usize> {
        let n = self.node_of(v)?;
        let o = self.nodes[n].v.as_object()?;
        (self.h.object_class(o) == ObjectClass::Promise && self.nodes[n].fl != NO_FLIGHT)
            .then_some(self.nodes[n].fl)
    }

    /// Array.fromAsync's state from its closures' data: the engine's Record, or the
    /// index of the state the dump runtime's `fromAsync` holds.
    fn async_from_state(&self, data: Value) -> Option<[Value; FROM_ASYNC_SLOTS as usize]> {
        if let Some(i) = data.as_int() {
            return self.fl.from_async.get(i as usize).copied();
        }
        let s = data.as_object()?;
        let mut out = [Value::UNDEFINED; FROM_ASYNC_SLOTS as usize];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.h.object_slot(s, i as u32);
        }
        Some(out)
    }

    fn clo_of(&self, v: Value) -> Option<&Clo> {
        let n = self.node_of(v)?;
        let o = self.nodes[n].v.as_object()?;
        // (A closure of the dump runtime may stand for one, dump_flight_runtime.rs.)
        let class = self.h.object_class(o);
        ((class == ObjectClass::BuiltinClosure || class == ObjectClass::Function)
            && self.nodes[n].fl != NO_FLIGHT)
            .then(|| &self.fl.clos[self.nodes[n].fl])
    }

    fn act_of(&self, v: Value) -> Option<usize> {
        let n = self.node_of(v)?;
        let o = self.nodes[n].v.as_object()?;
        (self.h.object_class(o) == ObjectClass::Generator && self.nodes[n].fl != NO_FLIGHT)
            .then_some(self.nodes[n].fl)
    }

    /// Who made each pending promise, and what each reaction is.
    fn analyze_flight(&mut self) {
        let h = self.h;
        for c in 0..self.fl.calls.len() {
            if let Some(p) = self.pend_of(self.fl.calls[c].promise) {
                self.fl.pends[p].creator = Creator::Call(c);
            }
        }
        for a in 0..self.fl.acts.len() {
            if self.fl.acts[a].s.kind == suspend::ASYNC
                && let Some(p) = self.pend_of(self.fl.acts[a].s.promise)
                && self.fl.pends[p].creator == Creator::Unknown
            {
                self.fl.pends[p].creator = Creator::Act(a);
            }
        }
        for p in 0..self.fl.pends.len() {
            for j in 0..self.fl.pends[p].reactions.len() {
                let Reaction { cap, f, r, .. } = self.fl.pends[p].reactions[j];
                let path = cat!(self.nodes[self.fl.pends[p].node].path, "<reaction ", j, ">");
                let kind = |d: &Self, v: Value| d.clo_of(v).map(|c| (c.kind, c.data));
                let consumer = if f == r
                    && let Some(a) = self.act_of(f)
                {
                    if self.fl.acts[a].s.state != suspend::AWAITING {
                        self.refuse(
                            path.clone(),
                            "an await of an activation that is not awaiting",
                        );
                    }
                    self.fl.acts[a].awaiting = Some((p, j));
                    Consumer::Await(a)
                } else if let Some((k, data)) = kind(self, f)
                    .filter(|c| matches!(c.0, ALL_ELEMENT | SETTLED_FULFILLED))
                    .or_else(|| kind(self, r).filter(|c| c.0 == ANY_REJECT))
                {
                    match self.combo_element(k, data, p, j) {
                        Some(c) => Consumer::Element(c),
                        None => {
                            self.refuse(path.clone(), "an element of a keyed promise combinator");
                            Consumer::Then
                        }
                    }
                } else if kind(self, f).map(|c| c.0) == Some(THEN_FINALLY)
                    && kind(self, r).map(|c| c.0) == Some(CATCH_FINALLY)
                {
                    Consumer::Finally
                } else if let (Some((FROM_ASYNC_FULFILLED, s)), Some((FROM_ASYNC_REJECTED, s2))) =
                    (kind(self, f), kind(self, r))
                    && s == s2
                {
                    // Its promise is made by the reaction: `$djs.fromAsync` again.
                    if let Some(d) = self.async_from_state(s).and_then(|st| self.pend_of(st[0]))
                        && self.fl.pends[d].creator == Creator::Unknown
                    {
                        self.fl.pends[d].creator = Creator::Then(p, j);
                    }
                    Consumer::FromAsync
                } else {
                    for v in [f, r] {
                        if let Some((k, _)) = kind(self, v)
                            && !matches!(
                                k,
                                RESOLVE
                                    | REJECT
                                    | VALUE_THUNK
                                    | THROWER
                                    | FROM_SYNC_UNWRAP
                                    | FROM_SYNC_CLOSE
                            )
                        {
                            self.refuse(path.clone(), "a reaction of a builtin in flight");
                        }
                    }
                    Consumer::Then
                };
                if !cap.is_undefined() && promise::as_promise(h, cap).is_none() {
                    self.refuse(path.clone(), "a then on a promise of another constructor");
                }
                self.fl.pends[p].reactions[j].consumer = consumer;
                if let Some(d) = self.pend_of(cap)
                    && self.fl.pends[d].creator == Creator::Unknown
                {
                    self.fl.pends[d].creator = Creator::Then(p, j);
                }
            }
        }
        for c in 0..self.fl.combos.len() {
            if let Some(p) = self.pend_of(self.fl.combos[c].cap)
                && self.fl.pends[p].creator == Creator::Unknown
            {
                self.fl.pends[p].creator = Creator::Combo(c);
            }
            // Every element that has not settled must be found (`remaining` counts them).
            let rem = self.fl.combos[c]
                .remaining
                .as_object()
                .and_then(|r| h.object_slot(r, 0).as_number())
                .unwrap_or(-1.0);
            if self.pend_of(self.fl.combos[c].cap).is_none() {
                for &(_, p, j) in &self.fl.combos[c].elements {
                    self.fl.pends[p].reactions[j].consumer = Consumer::Dead;
                }
                self.fl.combos[c].state = State::Done;
            } else if rem != self.fl.combos[c].elements.len() as f64 {
                let p = self.combo_path(c);
                self.refuse(
                    p,
                    "a promise combinator whose pending elements are not all reachable",
                );
            }
        }
        for k in 0..self.fl.clos.len() {
            let Clo { kind, data, .. } = self.fl.clos[k];
            if kind != RESOLVE && kind != REJECT {
                continue;
            }
            let target = match data.as_object() {
                Some(d) if h.object_class(d) == ObjectClass::Record => h.object_slot(d, 0),
                _ => data,
            };
            if let Some(p) = self.pend_of(target)
                && self.fl.pends[p].creator == Creator::Unknown
            {
                self.fl.pends[p].creator = Creator::Resolvers;
            }
        }
    }

    fn combo_path(&self, c: usize) -> String {
        match self.node_of(self.fl.combos[c].cap) {
            Some(n) => self.nodes[n].path.clone(),
            None => "<a promise combinator>".into(),
        }
    }

    /// Records element `data` (an element Record) of a combinator; its index.
    fn combo_element(&mut self, kind: usize, data: Value, p: usize, j: usize) -> Option<usize> {
        let h = self.h;
        let r = data.as_object()?;
        if !h.object_slot(r, 5).is_undefined() {
            return None;
        }
        let index = h.object_slot(r, 1).as_number()? as u32;
        let cap = h.object_slot(r, 3);
        let c = match self.fl.combos.iter().position(|c| c.cap == cap) {
            Some(c) => c,
            None => {
                self.fl.combos.push_out(Combo {
                    kind: if kind == SETTLED_REJECTED {
                        SETTLED_FULFILLED
                    } else {
                        kind
                    },
                    cap,
                    values: h.object_slot(r, 2),
                    remaining: h.object_slot(r, 4),
                    elements: Vec::new(),
                    state: State::New,
                });
                self.fl.combos.len() - 1
            }
        };
        self.fl.combos[c].elements.push_out((index, p, j));
        Some(c)
    }

    // ---------------------------------------------------------------- emission

    /// Writes the open calls, first and in their order.
    pub(super) fn flight_start(&mut self) {
        self.analyze_flight();
        for c in 0..self.fl.calls.len() {
            if let Some(p) = self.pend_of(self.fl.calls[c].promise) {
                self.make(p);
            }
        }
    }

    /// The rest of the reactions of every promise the dump made, then the handles.
    pub(super) fn flight_finish(&mut self) {
        loop {
            let mut progress = false;
            for p in 0..self.fl.pends.len() {
                let len = self.fl.pends[p].reactions.len();
                if len > 0 && self.fl.pends[p].cursor < len {
                    self.react_upto(p, len);
                    progress = true;
                }
            }
            for a in 0..self.fl.acts.len() {
                if self.fl.acts[a].state == State::New && self.fl.acts[a].s.kind != suspend::ASYNC {
                    self.emit_act(a);
                    progress = true;
                }
            }
            if !progress || !self.refusals.is_empty() {
                break;
            }
        }
        let handles = core::mem::take(&mut self.fl.handles);
        for (name, v) in handles {
            let before = self.fns_read.get();
            let e = self.expr(v, None, None);
            self.pl.handles.push_out((name.clone(), e.clone()));
            self.line(cat!("$dump.handle(", quote(&name), ", ", e, ");"), before);
        }
    }

    /// Pushes a line that writes no binding.
    fn line(&mut self, text: String, fns_before: u32) {
        self.out.push_out(text);
        self.skeleton
            .push_out((self.fns_read.get() == fns_before).then(String::new));
    }

    /// The expression of an in-flight node.
    pub(super) fn flight_expr(&mut self, n: usize) -> String {
        let h = self.h;
        let o = self.nodes[n].v.as_object().expect("an object");
        let i = self.nodes[n].fl;
        match h.object_class(o) {
            ObjectClass::Promise => self.make(i),
            ObjectClass::Generator => {
                if self.fl.acts[i].s.kind == suspend::ASYNC {
                    let p = self.nodes[n].path.clone();
                    self.refuse(p, "internal: an async function's state as a value");
                    return "undefined".into();
                }
                self.emit_act(i)
            }
            ObjectClass::BuiltinClosure | ObjectClass::Function => self.closure_expr(i),
            ObjectClass::ArrayIterator => self.iterator_expr(n, o),
            ObjectClass::CollectionIterator => self.collection_iterator_expr(n, o),
            ObjectClass::RegExpStringIterator => self.regexp_iterator_expr(n, o),
            ObjectClass::IteratorHelper => self.helper_expr(n, o),
            ObjectClass::Record | ObjectClass::Ordinary if self.is_from_sync(o) => {
                self.async_from_sync_expr(n, o)
            }
            ObjectClass::Record if self.nodes[n].using_record => self.using_expr(n, o),
            _ => {
                let p = self.nodes[n].path.clone();
                self.refuse(p, "internal: a promise builtin's record as a value");
                "undefined".into()
            }
        }
    }

    /// The iterator a `for await` over a sync iterable makes (CreateAsyncFromSyncIterator):
    /// a Record `[iterator, next]` with %AsyncFromSyncIteratorPrototype%.
    /// The dump runtime's `fromSync` iterator counts as one (dump_flight_runtime.rs).
    pub(super) fn is_from_sync(&self, o: Obj) -> bool {
        self.async_from_sync_parts(o).is_some()
    }

    /// An async-from-sync iterator's sync iterator and `next`.
    fn async_from_sync_parts(&self, o: Obj) -> Option<(Value, Value)> {
        let h = self.h;
        if h.object_class(o) == ObjectClass::Record
            && h.get_prototype_of(o).map(Value::object)
                == Some(intrinsic(h, Intrinsic::AsyncFromSyncIteratorProto))
        {
            return Some((h.object_slot(o, 0), h.object_slot(o, 1)));
        }
        self.runtime_from_sync(o)
    }

    /// An async-from-sync iterator: the dump's own, `$dump.fromSync(it, next)`, which steps
    /// the same way (the object is never visible to user code).
    fn async_from_sync_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        self.nodes[n].state = State::Busy;
        let before = self.fns_read.get();
        let (it, next) = self
            .async_from_sync_parts(o)
            .unwrap_or((Value::UNDEFINED, Value::UNDEFINED));
        let it = self.expr(it, None, None);
        let next = self.expr(next, None, None);
        if it.is_empty() || next.is_empty() {
            let p = self.nodes[n].path.clone();
            self.refuse(p, "a for-await over an iterator on a reference cycle");
        }
        let name = self.fresh("i");
        self.line(
            cat!(name, " = ", self.rt(), "fromSync(", it, ", ", next, ");"),
            before,
        );
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        name
    }

    /// The resource Record of `using` declarations: `$dump.using(resources, state)`, the
    /// state's slots by name where they hold something.
    fn using_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        if self.nodes[n].state == State::Busy {
            return String::new();
        }
        self.nodes[n].state = State::Busy;
        let h = self.h;
        // (The resources themselves are written by the continuation, not their array.)
        let mut fields = Vec::new();
        let mut cycle = false;
        for (k, slot) in USING_FIELDS {
            let v = h.object_slot(o, slot);
            if v.is_undefined() {
                continue;
            }
            let e = self.expr(v, None, None);
            cycle |= e.is_empty();
            fields.push_out(cat!(k, ": ", e));
        }
        if cycle {
            let p = self.nodes[n].path.clone();
            self.refuse(p, "a using declaration's record on a reference cycle");
        }
        // Plain dumps: no object; the continuation adds the resources again
        // (`Act::usings`). The text names what it needs.
        let Some(items) = super::origin::elements(h, h.object_slot(o, 2)) else {
            let p = self.nodes[n].path.clone();
            self.refuse(p, "a using declaration's record of another shape");
            return String::from("undefined");
        };
        let mut triples = Vec::new();
        for t in items.chunks(3) {
            let [v, m, k] = t else { continue };
            let (v, m) = unwrap_using(h, *v, *m);
            let ve = self.expr(v, None, None);
            let me = self.expr(m, None, None);
            triples.push_out((ve, me, k.as_int().unwrap_or(-1)));
        }
        let mut state = Vec::new();
        for f in fields {
            if let Some((k, e)) = f.split_once(": ") {
                state.push_out((String::from(k), String::from(e)));
            }
        }
        let parts: Vec<String> = triples
            .iter()
            .map(|(v, m, k)| cat!(v, ", ", m, ", ", *k))
            .collect();
        let fields: Vec<String> = state.iter().map(|(k, e)| cat!(k, ": ", e)).collect();
        let text = cat!(
            "$dump.using([",
            join(&parts, ", "),
            "], { ",
            join(&fields, ", "),
            " })"
        );
        self.fl.usings.push(plain::Using {
            slot: text.clone(),
            triples,
            state,
        });
        self.nodes[n].loc = text.clone();
        self.nodes[n].state = State::Done;
        text
    }

    fn closure_expr(&mut self, k: usize) -> String {
        let h = self.h;
        let Clo { node, kind, data } = self.fl.clos[k];
        if kind == FROM_SYNC_UNWRAP {
            // A for-await over a sync iterator: the result of a step (D2612).
            return cat!("(v) => ({ value: v, done: ", tf(data == Value::TRUE), " })");
        }
        if kind == FROM_SYNC_CLOSE {
            let it = self.expr(data, None, None);
            return cat!(self.rt(), "closeThrow(", it, ")");
        }
        if kind == VALUE_THUNK || kind == THROWER {
            // A finally's value thunk or thrower, or the handler that makes an
            // AsyncIteratorClose of a throw ignore `return()`'s outcome: the same as a
            // function of the dump's.
            let v = self.expr(data, None, None);
            if v.is_empty() {
                let p = self.nodes[node].path.clone();
                self.refuse(p, "a promise builtin's function on a reference cycle");
            }
            return if kind == VALUE_THUNK {
                cat!("() => (", v, ")")
            } else {
                cat!("() => { throw ", v, "; }")
            };
        }
        if kind != RESOLVE && kind != REJECT {
            let p = self.nodes[node].path.clone();
            self.refuse(p, "internal: a promise builtin's function as a value");
            return "undefined".into();
        }
        let target = match data.as_object() {
            Some(d) if h.object_class(d) == ObjectClass::Record => h.object_slot(d, 0),
            _ => data,
        };
        match self.pend_of(target) {
            Some(p) if self.fl.pends[p].creator == Creator::Resolvers => {
                self.make(p);
                let r = self.fl.pends[p].resolvers.clone();
                if r.is_empty() {
                    return r;
                }
                cat!(r, "[", kind, "]")
            }
            Some(_) => {
                let p = self.nodes[node].path.clone();
                self.refuse(p, "a resolving function of a promise made otherwise");
                "undefined".into()
            }
            // The promise settled: calling them does nothing.
            None => {
                cat!(self.rt(), "spent(", kind, ")")
            }
        }
    }

    fn iterator_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        if self.nodes[n].state == State::Busy {
            return String::new();
        }
        self.nodes[n].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let target = h.object_slot(o, 0);
        let index = h.object_slot(o, 1).as_int().unwrap_or(0);
        let kind = h.object_slot(o, 2).as_int();
        let method = match kind {
            Some(0) => "Array.prototype.keys",
            Some(2) => "Array.prototype.entries",
            // A String Iterator (D1346): it steps by code points.
            Some(3) => "String.prototype[Symbol.iterator]",
            _ => "Array.prototype.values",
        };
        // An array iterator over an array only it holds: a new one over what is left
        // (0216's rule 7), `["second"].values()` for `advance(…values.call(["first",
        // "second"]), 1)`.
        if kind.is_none_or(|k| k == 1)
            && let Some(rest) = self.lone_array_rest(target, index)
        {
            let els: Vec<String> = rest.into_iter().map(|v| self.expr(v, None, None)).collect();
            if !els.iter().any(String::is_empty) {
                let name = self.fresh("i");
                self.line(cat!(name, " = [", join(&els, ", "), "].values();"), before);
                self.nodes[n].loc = name.clone();
                self.nodes[n].state = State::Done;
                return name;
            }
        }
        let (t, steps) = if target.is_undefined() {
            (String::from(if kind == Some(3) { "\"\"" } else { "[]" }), 1)
        } else if kind == Some(3) {
            let units: Vec<u16> = target
                .as_string()
                .map(|s| h.str_units(s).take(index.max(0) as usize).collect())
                .unwrap_or_default();
            let low = |u: u16| (0xDC00..0xE000).contains(&u);
            let high = |u: u16| (0xD800..0xDC00).contains(&u);
            let cps = (0..units.len())
                .filter(|&i| !(low(units[i]) && i > 0 && high(units[i - 1])))
                .count();
            (self.expr(target, None, None), cps as i32)
        } else {
            (self.expr(target, None, None), index)
        };
        if t.is_empty() {
            let p = self.nodes[n].path.clone();
            self.refuse(p, "an iterator over an array on a reference cycle");
        }
        let mut e = cat!(method, ".call(", t, ")");
        if steps > 0 {
            e = cat!(self.rt(), "advance(", e, ", ", steps, ")");
        }
        let name = self.fresh("i");
        self.line(cat!(name, " = ", e, ";"), before);
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        name
    }

    /// The elements from `index` on of `target`, where it is an array nothing but its
    /// iterator holds: dense, ordinary (its prototype `Array.prototype`, extensible, no
    /// properties but its elements and `length`).
    fn lone_array_rest(&self, target: Value, index: i32) -> Option<Vec<Value>> {
        let h = self.h;
        let o = target.as_object()?;
        let tn = self.node_of(target)?;
        if self.nodes[tn].refs != 1
            || self.nodes[tn].cyclic
            || self.nodes[tn].root.is_some()
            || !h.is_extensible(o)
            || h.get_prototype_of(o).map(Value::object) != Some(intrinsic(h, Intrinsic::ArrayProto))
        {
            return None;
        }
        let els = super::origin::elements(h, target)?;
        let index = usize::try_from(index).ok()?;
        if els.iter().any(|v| v.is_hole()) || h.own_keys(o).len() != els.len() + 1 {
            return None;
        }
        Some(els.get(index..).map(<[Value]>::to_vec).unwrap_or_default())
    }

    /// An iterator helper (`map`, `take`, `Iterator.from`'s wrapper, `concat`, `zip`, …):
    /// `$dump.iteratorHelper([slots])`, its state machine's slots as they are
    /// (builtins/iterator.ts).
    fn helper_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        if self.nodes[n].state == State::Busy {
            return String::new();
        }
        self.nodes[n].state = State::Busy;
        let before = self.fns_read.get();
        self.plain_helper_expr(n, o, before)
    }

    /// A RegExp String Iterator (`matchAll`): `$dump.regexpIterator(re, s, flags)` over
    /// the regexp it made (with its `lastIndex`), the string, its global, unicode and
    /// done bits.
    fn regexp_iterator_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        if self.nodes[n].state == State::Busy {
            return String::new();
        }
        self.nodes[n].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let re = self.expr(h.object_slot(o, regexp::IT_REGEXP), None, None);
        let s = self.expr(h.object_slot(o, regexp::IT_STRING), None, None);
        let flags = h.object_slot(o, regexp::IT_FLAGS).as_int().unwrap_or(0);
        if re.is_empty() {
            let p = self.nodes[n].path.clone();
            self.refuse(p, "a RegExp String Iterator on a reference cycle");
        }
        let name = self.fresh("i");
        self.line(
            cat!(
                name,
                " = ",
                self.rt(),
                "regexpIterator(",
                re,
                ", ",
                s,
                ", ",
                flags,
                ");"
            ),
            before,
        );
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        name
    }

    /// A Map or Set iterator: made on the collection and advanced past the live entries
    /// it has passed (advancing runs no user code).
    fn collection_iterator_expr(&mut self, n: usize, o: Obj) -> String {
        if self.nodes[n].state == State::Done {
            return self.nodes[n].loc.clone();
        }
        self.nodes[n].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let set = h.object_slot(o, 3).as_int() == Some(1);
        let class = if set { "Set" } else { "Map" };
        let method = match (set, h.object_slot(o, 2).as_int()) {
            (false, Some(0)) => "keys",
            (_, Some(2)) => "entries",
            _ => "values",
        };
        let e = match h.collection_iterator_position(o) {
            None => {
                cat!(
                    self.rt(),
                    "advance(",
                    class,
                    ".prototype.",
                    method,
                    ".call(new ",
                    class,
                    "()), 1)"
                )
            }
            Some((entries, live)) => {
                // The collection: a node of the dump whose entries these are.
                let owner = self.nodes.iter().position(|d| {
                    d.v.as_object().is_some_and(|c| {
                        matches!(h.object_class(c), ObjectClass::Map | ObjectClass::Set)
                            && h.collection_entries_ref(c) == Some(entries)
                    })
                });
                let t = match owner {
                    Some(c) => self.expr(self.nodes[c].v, None, None),
                    // Only the iterator holds the collection: a new one with its entries.
                    None => {
                        let mut items = Vec::new();
                        for (k, v) in h.collection_entries_of(entries) {
                            let k = self.expr(k, None, None);
                            if set {
                                items.push_out(k);
                            } else {
                                let v = self.expr(v, None, None);
                                items.push_out(cat!("[", k, ", ", v, "]"));
                            }
                        }
                        cat!("new ", class, "([", join(&items, ", "), "])")
                    }
                };
                if t.is_empty() {
                    let p = self.nodes[n].path.clone();
                    self.refuse(p, "an iterator over a collection on a reference cycle");
                }
                let mut e = cat!(class, ".prototype.", method, ".call(", t, ")");
                if live > 0 {
                    e = cat!(self.rt(), "advance(", e, ", ", live, ")");
                }
                e
            }
        };
        let name = self.fresh("i");
        self.line(cat!(name, " = ", e, ";"), before);
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        name
    }

    /// Makes pending promise `p` (once); its location.
    fn make(&mut self, p: usize) -> String {
        let n = self.fl.pends[p].node;
        if self.fl.pends[p].made {
            return self.nodes[n].loc.clone();
        }
        if self.fl.pends[p].busy {
            return String::new();
        }
        if self.data_depth > 0
            && matches!(
                self.fl.pends[p].creator,
                Creator::Act(_) | Creator::Then(..) | Creator::Combo(_)
            )
        {
            // Data holds it: the activation or reaction that makes it may need that data
            // first. The data gets the promise by a patch at the end; the promise is made
            // with what makes it (at the latest when its awaited promise's reactions are
            // written).
            self.reserve(n, "p");
            return String::new();
        }
        self.fl.pends[p].busy = true;
        let before = self.fns_read.get();
        match self.fl.pends[p].creator {
            Creator::Call(c) => {
                let args = self.fl.calls[c].args.clone();
                let a: Vec<String> = args.iter().map(|&v| self.expr(v, None, None)).collect();
                if a.iter().any(String::is_empty) {
                    let path = self.nodes[n].path.clone();
                    self.refuse(
                        path,
                        "an open call whose arguments are on a reference cycle",
                    );
                }
                let name = self.reserve(n, "p");
                let Call {
                    id, name: f, state, ..
                } = &self.fl.calls[c];
                let state = state.as_deref().map_or(String::new(), |s| cat!(", ", s));
                // A plain dump asks the host for the call (D2832).
                let l = cat!(
                    name,
                    " = $djs.resume(",
                    *id,
                    ", ",
                    quote(f),
                    ", [",
                    join(&a, ", "),
                    "]",
                    state,
                    ");"
                );
                self.line(l, before);
                self.made(p, name);
            }
            Creator::Act(a) => {
                self.emit_act(a);
            }
            Creator::Then(q, j) => {
                self.react_upto(q, j + 1);
            }
            Creator::Combo(c) => {
                self.emit_combo(c);
            }
            Creator::Resolvers => {
                let name = self.reserve(n, "p");
                let r = self.fresh("r");
                self.line(
                    cat!(
                        name,
                        " = new Promise((res, rej) => { ",
                        r,
                        " = [res, rej]; });"
                    ),
                    before,
                );
                self.fl.pends[p].resolvers = r;
                self.made(p, name);
            }
            Creator::Unknown => {
                let name = self.reserve(n, "p");
                self.line(cat!(name, " = new Promise(() => {});"), before);
                self.made(p, name);
            }
        }
        self.fl.pends[p].busy = false;
        if !self.fl.pends[p].made {
            // Its maker is being written further up (a reference cycle): the caller
            // patches the reference in later, or refuses.
            return String::new();
        }
        self.nodes[n].loc.clone()
    }

    /// The location of node `n`, chosen now if it has none yet (so that patches can name
    /// a value that is made later).
    fn reserve(&mut self, n: usize, prefix: &str) -> String {
        if self.nodes[n].loc.is_empty() {
            self.nodes[n].loc = self.fresh(prefix);
        }
        self.nodes[n].loc.clone()
    }

    fn made(&mut self, p: usize, loc: String) {
        let n = self.fl.pends[p].node;
        self.nodes[n].loc = loc;
        self.nodes[n].state = State::Done;
        self.fl.pends[p].made = true;
        self.fl.made.push_out(p);
    }

    /// Adds the reactions of `q` before `k`, in their order.
    fn react_upto(&mut self, q: usize, k: usize) {
        if self.make(q).is_empty() {
            let path = self.nodes[self.fl.pends[q].node].path.clone();
            self.refuse(path, "a promise reacted to before the dump can make it");
            return;
        }
        while self.fl.pends[q].cursor < k && self.refusals.is_empty() {
            let j = self.fl.pends[q].cursor;
            if self.fl.pends[q].reactions[j].busy {
                // Being written further up: it adds itself.
                return;
            }
            self.fl.pends[q].reactions[j].busy = true;
            match self.fl.pends[q].reactions[j].consumer {
                Consumer::Await(a) => {
                    self.emit_act(a);
                }
                Consumer::Then | Consumer::Finally | Consumer::Dead | Consumer::FromAsync => {
                    self.emit_then(q, j)
                }
                Consumer::Element(c) => self.emit_combo(c),
            }
            self.fl.pends[q].reactions[j].busy = false;
            if self.fl.pends[q].cursor == j {
                let path = self.nodes[self.fl.pends[q].node].path.clone();
                self.refuse(path, "reactions the dump cannot add in their order");
                return;
            }
        }
    }

    /// The reaction `j` of `q` is added now.
    fn subscribed(&mut self, q: usize, j: usize) {
        if self.fl.pends[q].cursor != j {
            let path = self.nodes[self.fl.pends[q].node].path.clone();
            self.refuse(path, "reactions the dump cannot add in their order");
        }
        self.fl.pends[q].cursor = j + 1;
    }

    /// `p.then(f, r)` (or `p.finally(f)`) again: its derived promise.
    fn emit_then(&mut self, q: usize, j: usize) {
        let h = self.h;
        let before = self.fns_read.get();
        let Reaction {
            cap,
            f,
            r,
            consumer,
            ..
        } = self.fl.pends[q].reactions[j];
        let ql = self.make(q);
        // A promise of the builtin prototype without own methods is subscribed to by
        // its methods (`p.then(f)`, 0216's rule 9): while the dump loads, builtins are
        // the realm's own (their changes come at the end).
        let pv = self.nodes[self.fl.pends[q].node].v;
        let method = pv.as_object().is_some_and(|o| {
            h.get_prototype_of(o).map(Value::object) == Some(intrinsic(h, Intrinsic::PromiseProto))
                && !h
                    .own_keys(o)
                    .iter()
                    .any(|k| *k == atoms::THEN || *k == atoms::FINALLY)
        });
        let recv = |m: &str| {
            if !method {
                return cat!("Promise.prototype.", m, ".call(", ql, ", ");
            }
            let simple = ql
                .bytes()
                .all(|c| c == b'.' || c == b'$' || c == b'_' || c.is_ascii_alphanumeric());
            if simple {
                cat!(ql, ".", m, "(")
            } else {
                cat!("(", ql, ").", m, "(")
            }
        };
        // (Array.fromAsync's promise is the one its call makes.)
        let mut cap = cap;
        let call = if consumer == Consumer::FromAsync {
            let st = self
                .clo_of(f)
                .and_then(|c| self.async_from_state(c.data))
                .expect("Array.fromAsync's state");
            cap = st[0];
            let mut args = alloc::vec![ql.clone()];
            let from_sync = st[3].as_object().is_some_and(|it| self.is_from_sync(it));
            for (i, &v) in st.iter().enumerate().skip(1) {
                let e = if i == 4 && from_sync {
                    cat!(args[3], ".next")
                } else {
                    self.expr(v, None, None)
                };
                if e.is_empty() {
                    let path = self.nodes[self.fl.pends[q].node].path.clone();
                    self.refuse(
                        path,
                        "an Array.fromAsync whose state is on a reference cycle",
                    );
                }
                args.push_out(e);
            }
            cat!(self.rt(), "fromAsync(", join(&args, ", "), ")")
        } else if consumer == Consumer::Dead {
            cat!(recv("then"), "() => {}, () => {})")
        } else if consumer == Consumer::Finally {
            let on = self
                .clo_of(f)
                .and_then(|c| c.data.as_object())
                .map_or(Value::UNDEFINED, |d| h.object_slot(d, 0));
            let e = self.expr(on, None, None);
            cat!(recv("finally"), e, ")")
        } else {
            let fe = self.expr(f, None, None);
            let re = self.expr(r, None, None);
            if fe.is_empty() || re.is_empty() {
                let path = self.nodes[self.fl.pends[q].node].path.clone();
                self.refuse(path, "a reaction whose handler is on a reference cycle");
            }
            // (A missing argument is `undefined`: trailing ones left out.)
            let args = match (fe.as_str(), re.as_str()) {
                ("undefined", "undefined") if method => String::new(),
                (_, "undefined") if method => fe,
                _ => cat!(fe, ", ", re),
            };
            cat!(recv("then"), args, ")")
        };
        self.react_upto(q, j);
        match self.pend_of(cap) {
            Some(d) if !self.fl.pends[d].made => {
                let name = self.reserve(self.fl.pends[d].node, "p");
                self.line(cat!(name, " = ", call, ";"), before);
                self.made(d, name);
            }
            _ => self.line(cat!("void ", call, ";"), before),
        }
        self.subscribed(q, j);
    }

    /// A combinator called again: settled elements by their values, pending ones by
    /// their promises.
    fn emit_combo(&mut self, c: usize) {
        if self.fl.combos[c].state != State::New {
            return;
        }
        self.fl.combos[c].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let mut elements = self.fl.combos[c].elements.clone();
        crate::sort::sort_by(&mut elements, |a, b| a.0 < b.0);
        let values = self.fl.combos[c].values.as_object();
        let len = values.map_or(0, |v| h.array_length(v));
        let kind = self.fl.combos[c].kind;
        let mut items: Vec<String> = Vec::new();
        for i in 0..len {
            let e = match elements.iter().find(|e| e.0 == i) {
                Some(&(_, p, _)) => self.make(p),
                None => {
                    let v = values
                        .and_then(|v| h.get_element_fast(v, i))
                        .unwrap_or(Value::UNDEFINED);
                    self.settled_element(kind, v)
                }
            };
            items.push_out(e);
        }
        for &(_, p, j) in &elements {
            self.react_upto(p, j);
        }
        let f = match kind {
            ALL_ELEMENT => "all",
            SETTLED_FULFILLED => "allSettled",
            _ => "any",
        };
        let call = cat!("Promise.", f, "([", join(&items, ", "), "])");
        if items.iter().any(String::is_empty) {
            let p = self.combo_path(c);
            self.refuse(p, "a promise combinator on a reference cycle");
        }
        match self.pend_of(self.fl.combos[c].cap) {
            Some(d) if !self.fl.pends[d].made => {
                let name = self.reserve(self.fl.pends[d].node, "p");
                self.line(cat!(name, " = ", call, ";"), before);
                self.made(d, name);
            }
            _ => self.line(cat!("void ", call, ";"), before),
        }
        for &(_, p, j) in &elements {
            self.subscribed(p, j);
        }
        self.fl.combos[c].state = State::Done;
    }

    /// An element that settled, written so the combinator records the same: its value,
    /// or for a rejection a rejected promise.
    fn settled_element(&mut self, kind: usize, v: Value) -> String {
        let h = self.h;
        let get = |o: Obj, k: &str| {
            h.own_keys(o)
                .into_iter()
                .find(|a| super::atom_text(h, *a) == k)
                .and_then(|a| h.get_own_property(o, a))
                .and_then(|p| p.value())
        };
        let rejected = |d: &mut Self, e: Value| {
            let r = d.expr(e, None, None);
            cat!(d.rt(), "rejected(", r, ")")
        };
        match kind {
            SETTLED_FULFILLED => {
                let Some(o) = v.as_object() else {
                    return "undefined".into();
                };
                let status = get(o, "status").and_then(|s| s.as_string());
                let fulfilled = status.is_some_and(|s| super::text_of(h, s) == "fulfilled");
                if fulfilled {
                    let x = get(o, "value").unwrap_or(Value::UNDEFINED);
                    self.expr(x, None, None)
                } else {
                    let x = get(o, "reason").unwrap_or(Value::UNDEFINED);
                    rejected(self, x)
                }
            }
            ANY_REJECT => rejected(self, v),
            _ => self.expr(v, None, None),
        }
    }

    /// `$dump.resume(fn, {...})` for activation `a`: its location.
    fn emit_act(&mut self, a: usize) -> String {
        let n = self.fl.acts[a].node;
        match self.fl.acts[a].state {
            State::Done => return self.nodes[n].loc.clone(),
            State::Busy => return String::new(),
            State::New => {}
        }
        let Some(site) = self.fl.acts[a].site.clone() else {
            return self.emit_unstarted(a);
        };
        self.fl.acts[a].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let path = self.nodes[n].path.clone();
        let s = &self.fl.acts[a].s;
        let frame = s.frame.clone();
        let queue = s.queue.clone();
        let kind = s.kind;
        let hp = s.hp as usize;
        let lp = hp + crate::interp::frame::HEADER as usize;
        let argc = frame
            .get(hp + crate::interp::frame::ARGC as usize)
            .and_then(|v| v.as_int())
            .unwrap_or(0) as usize;
        let fv = |i: usize| frame.get(i).copied().unwrap_or(Value::UNDEFINED);
        let mut fields: Vec<String> = alloc::vec![cat!("at: ", quote(&site.name))];
        let awaiting = self.fl.acts[a].awaiting;
        let mut parts: Vec<(&str, String)> = Vec::new();
        // A codemode body: the host's script by reference (D2882), else its text.
        let fe = match fv(0).as_object().and_then(|f| self.source_ref(f)) {
            Some(r) => r,
            None => self.expr(fv(0), None, None),
        };
        parts.push_out(("function", fe.clone()));
        let mut awaited = None;
        if let Some((q, _)) = awaiting {
            let ql = self.make(q);
            parts.push_out(("awaited promise", ql.clone()));
            fields.push_out(cat!("awaiting: ", ql));
            awaited = Some(ql);
        } else if s_state_awaiting(&self.fl.acts[a].s) {
            fields.push_out("awaiting: new Promise(() => {})".into());
            awaited = Some("new Promise(() => {})".into());
        }
        let this = self.expr(fv(1), None, None);
        parts.push_out(("this", this.clone()));
        if this != "undefined" {
            fields.push_out(cat!("this: ", this));
        }
        let args: Vec<String> = (0..argc)
            .map(|i| self.expr(fv(2 + i), None, None))
            .collect();
        if !args.is_empty() {
            fields.push_out(cat!("args: [", join(&args, ", "), "]"));
        }
        parts.extend(args.into_iter().map(|a| ("argument", a)));
        // The frame's slots by name (D2731): bindings (`name@scope` where two of a name
        // are in scope), hidden locals, the temporaries of constructs in flight.
        let mut slots: Vec<String> = Vec::new();
        let mut pslots: Vec<plain::Slot> = Vec::new();
        let n_locals = site.locals.len();
        let index = |p: Place| match p {
            Place::Arg(i) => 2 + usize::from(i),
            Place::Local(i) => lp + usize::from(i),
            Place::Stack(k) => lp + n_locals + usize::from(k),
        };
        // The iterator below a loop's `next` (an async-from-sync one brings its own).
        let mut it = String::new();
        let named = site.named_slots();
        for slot in named.iter().cloned() {
            let v = fv(index(slot.place));
            let key = match slot.holds {
                Holds::Binding if is_ident(&slot.name) => slot.name.clone(),
                // (A `this` arrows share is the frame's: its scope goes into K, 0215.)
                Holds::Hidden
                    if matches!(slot.name.as_str(), "this" | "new.target") && slot.boxed =>
                {
                    quote(&slot.name)
                }
                Holds::Hidden if computed_hidden(&slot.name) => continue,
                _ => quote(&slot.name),
            };
            let mut cell = None;
            let (what, e) = match slot.holds {
                Holds::Hidden if class_hidden(&slot.name) => ("binding", "undefined".into()),
                Holds::Binding | Holds::Hidden if slot.boxed => {
                    let e = self.slot_expr(v, slot.boxed);
                    // A variable closures share: the factory's cell.
                    cell = self
                        .node_of(v)
                        .filter(|&bn| !e.is_empty() && self.nodes[bn].loc == e);
                    ("binding", e)
                }
                Holds::Binding
                    if let Place::Local(i) = slot.place
                        && let Some(sx) = self.fl.acts[a].syntax.clone()
                        && site
                            .locals
                            .get(usize::from(i))
                            .is_some_and(|l| undeclared_at(&sx, &site, l, v)) =>
                {
                    ("binding", String::from("$dump.TDZ"))
                }
                Holds::Binding | Holds::Hidden => ("binding", self.slot_expr(v, slot.boxed)),
                Holds::Temp => {
                    let e = match slot.construct() {
                        "for-await.next" | "yield*.next"
                            if from_sync_partner(&named, &slot)
                                .map(|p| fv(index(p)))
                                .and_then(|v| v.as_object())
                                .is_some_and(|o| self.is_from_sync(o)) =>
                        {
                            cat!(it, ".next")
                        }
                        "for-in.iterator" => match for_in_parts(h, v) {
                            Some((target, keys)) => {
                                let t = self.expr(target, None, None);
                                let ks: Vec<String> =
                                    keys.into_iter().map(|k| self.expr(k, None, None)).collect();
                                cat!(self.rt(), "forIn(", t, ", [", join(&ks, ", "), "])")
                            }
                            // (Refused by the visit.)
                            None => "undefined".into(),
                        },
                        _ => self.expr(v, None, None),
                    };
                    if matches!(slot.construct(), "for-await.iterator" | "yield*.iterator") {
                        it = e.clone();
                    }
                    ("slot", e)
                }
            };
            parts.push_out((what, e.clone()));
            pslots.push_out(plain::Slot {
                name: slot.name.clone(),
                value: e.clone(),
                cell,
            });
            slots.push_out(cat!(key, ": ", e));
        }
        if !slots.is_empty() {
            fields.push_out(cat!("slots: { ", join(&slots, ", "), " }"));
        }
        let mut reqs: Vec<String> = Vec::new();
        let mut preqs: Vec<(&'static str, String, String)> = Vec::new();
        for r in queue {
            let Some(r) = r.as_object() else { continue };
            let t = match h.object_slot(r, 0).as_number().map(|t| t as i32) {
                Some(1) => "throw",
                Some(2) => "return",
                _ => "next",
            };
            let v = self.expr(h.object_slot(r, 1), None, None);
            let p = match self.pend_of(h.object_slot(r, 2)) {
                Some(p) => self.make(p),
                None => String::new(),
            };
            parts.push_out(("queue", v.clone()));
            parts.push_out(("queue", p.clone()));
            preqs.push_out((t, v.clone(), p.clone()));
            reqs.push_out(cat!(
                "{ type: \"",
                t,
                "\", value: ",
                v,
                ", promise: ",
                p,
                " }"
            ));
        }
        if !reqs.is_empty() {
            fields.push_out(cat!("queue: [", join(&reqs, ", "), "]"));
        }
        if let Some((what, _)) = parts.iter().find(|p| p.1.is_empty()) {
            let why = cat!("an activation on a reference cycle (its ", *what, ")");
            self.refuse(path.clone(), &why);
        }
        if let Some((q, j)) = awaiting {
            self.react_upto(q, j);
        }
        let name = match self.pend_of(self.fl.acts[a].s.promise) {
            Some(p) if kind == suspend::ASYNC => self.reserve(self.fl.pends[p].node, "a"),
            _ => self.reserve(n, "a"),
        };
        if let Some(f) = fv(0).as_object() {
            let info = self.site_info(f, &site, &name);
            self.fl.sites.push(info);
        }
        let fnode = self.node_of(fv(0)).unwrap_or(usize::MAX);
        let ftext = if fnode == usize::MAX {
            String::new()
        } else {
            self.fn_source(fnode).unwrap_or_default()
        };
        // A generator's prototype: its continuation's (the origin's function made
        // from its text has a `prototype` of its own).
        let proto = (kind != suspend::ASYNC)
            .then(|| self.nodes[n].v.as_object())
            .flatten()
            .map(|g| {
                let p = h.get_prototype_of(g).map_or(Value::NULL, Value::object);
                self.expr(p, None, None)
            });
        let sp = self.fl.supers.iter().find(|x| x.0 == a).map(|x| x.1);
        let super_proto = sp.map(|p| self.expr(p, None, None));
        let peers = match fv(0).as_object() {
            Some(f)
                if matches!(
                    site.kind,
                    SiteKind::ForAwaitClose | SiteKind::YieldStarAwait
                ) =>
            {
                self.peer_sites(f, &site)
            }
            _ => Vec::new(),
        };
        let usings = pslots
            .iter()
            .filter(|s| s.name.starts_with("using.record@"))
            .filter_map(|s| {
                let u = self.fl.usings.iter().find(|u| u.slot == s.value)?;
                Some(plain::Using {
                    slot: s.name.clone(),
                    triples: u.triples.clone(),
                    state: u.state.clone(),
                })
            })
            .collect();
        let rereads = self.rereads(
            fv(0).as_object(),
            self.fl.acts[a]
                .syntax
                .as_deref()
                .unwrap_or(&Default::default()),
            &ftext,
            &named,
            &|x| fv(index(x.place)),
            &site.path,
        );
        let act = plain::Act {
            rereads,
            usings,
            peers,
            proto,
            super_proto,
            name: String::from(name.strip_prefix("$dump.").unwrap_or(&name)),
            fexpr: fe.clone(),
            fkind: if fnode == usize::MAX {
                0
            } else {
                self.fn_kind(fnode)
            },
            ftext,
            site: site.clone(),
            syntax: self.fl.acts[a].syntax.clone().unwrap_or_default(),
            awaiting: awaited,
            this: this.clone(),
            slots: pslots,
            queue: preqs,
            kind,
        };
        let at = self.out.len();
        let pl = &mut self.pl;
        pl.lines.push_out((at, plain::Rec::Act(pl.acts.len())));
        pl.acts.push(act);
        self.line(
            cat!(
                name,
                " = $dump.resume(",
                fe,
                ", { ",
                join(&fields, ", "),
                " });"
            ),
            before,
        );
        if let Some((q, j)) = awaiting {
            self.subscribed(q, j);
        }
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        self.fl.acts[a].state = State::Done;
        if kind == suspend::ASYNC
            && let Some(p) = self.pend_of(self.fl.acts[a].s.promise)
            && !self.fl.pends[p].made
        {
            self.made(p, name.clone());
        }
        name
    }

    /// A generator that has not started (called again) or has finished (a new one,
    /// finished at once, with its prototype).
    fn emit_unstarted(&mut self, a: usize) -> String {
        let n = self.fl.acts[a].node;
        self.fl.acts[a].state = State::Busy;
        let h = self.h;
        let before = self.fns_read.get();
        let s = &self.fl.acts[a].s;
        let (state, kind) = (s.state, s.kind);
        let frame = s.frame.clone();
        let hp = s.hp as usize;
        let e = if state == suspend::START {
            let argc = frame
                .get(hp + crate::interp::frame::ARGC as usize)
                .and_then(|v| v.as_int())
                .unwrap_or(0) as usize;
            let fv = |i: usize| frame.get(i).copied().unwrap_or(Value::UNDEFINED);
            let f = self.expr(fv(0), None, None);
            let this = self.expr(fv(1), None, None);
            let args: Vec<String> = (0..argc)
                .map(|i| self.expr(fv(2 + i), None, None))
                .collect();
            let call = cat!(
                "Reflect.apply(",
                f,
                ", ",
                this,
                ", [",
                join(&args, ", "),
                "])"
            );
            // Its prototype, where it is not the one its function's text makes (a
            // function's `prototype` replaced or changed, which the dump sets later).
            let g = self.nodes[n].v.as_object().expect("a generator");
            let proto = h.get_prototype_of(g).map_or(Value::NULL, Value::object);
            let default = fv(0).as_object().is_some_and(|f| {
                h.get_own_property(f, crate::heap::atoms::PROTOTYPE)
                    .and_then(|p| p.value())
                    == Some(proto)
                    && self.default_fn_prototype(f)
            });
            if default {
                call
            } else {
                let p = self.expr_or_loc(proto);
                cat!("Object.setPrototypeOf(", call, ", ", p, ")")
            }
        } else {
            let g = self.nodes[n].v.as_object().expect("a generator");
            let proto = h.get_prototype_of(g).map_or(Value::NULL, Value::object);
            // A generator function's own `prototype` is written with the function.
            let owner = self.nodes.iter().position(|d| {
                d.v.as_object().is_some_and(|f| {
                    h.object_class(f) == ObjectClass::Function
                        && h.get_own_property(f, crate::heap::atoms::PROTOTYPE)
                            .and_then(|p| p.value())
                            == Some(proto)
                })
            });
            let p = match owner {
                Some(f) => {
                    let e = self.expr(self.nodes[f].v, None, None);
                    cat!(e, ".prototype")
                }
                None => self.expr(proto, None, None),
            };
            let make = if kind == suspend::ASYNC_GENERATOR {
                "(async function* () {})()"
            } else {
                "(function* () {})()"
            };
            cat!(
                "Object.setPrototypeOf(((g) => (g.return(), g))(",
                make,
                "), ",
                p,
                ")"
            )
        };
        let name = self.reserve(n, "a");
        self.line(cat!(name, " = ", e, ";"), before);
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        self.fl.acts[a].state = State::Done;
        name
    }

    /// Where an activation stopped (`Vm::dump_report`): its function, the site, and the
    /// site's line, column and source line in its script.
    fn site_info(&self, f: Obj, site: &Site, loc: &str) -> ResumedSite {
        let h = self.h;
        let t = h.closure_template(f);
        let info = template::debug_info(h, t);
        let a = h.arena();
        let text = |r: Ref| r.get().map(|s| super::text_of(h, crate::value::Str(s)));
        let script = info.script.get();
        let src = script
            .and_then(|s| template::script_text(h, s))
            .unwrap_or_default();
        let file = script
            .and_then(|s| text(a.get::<Ref>(s, Script::NAME)))
            .unwrap_or_default();
        let atom = Atom::from_id(a.get::<u32>(t, FuncTemplate::NAME));
        let function = if atom.is_null() {
            String::new()
        } else {
            super::atom_text(h, atom)
        };
        let at = (info.src_start + site.pos) as usize;
        let start = src
            .get(..at)
            .and_then(|b| b.rfind('\n'))
            .map_or(0, |i| i + 1);
        let end = src
            .get(at..)
            .and_then(|r| r.find('\n'))
            .map_or(src.len(), |i| at + i);
        let line = src.get(..at).map_or(0, |b| b.matches('\n').count()) + 1;
        let col = src.get(start..at).map_or(0, |b| b.chars().count()) + 1;
        ResumedSite {
            loc: String::from(loc),
            function,
            site: site.name.clone(),
            file,
            line: line as u32,
            col: col as u32,
            source_line: String::from(src.get(start..end).unwrap_or_default()),
        }
    }

    /// A frame slot's value: `$dump.TDZ`, a Box's cell when closures share it, else the
    /// value.
    fn slot_expr(&mut self, v: Value, boxed: bool) -> String {
        let h = self.h;
        let inner = match (boxed, v.as_internal()) {
            (true, Some(b)) => {
                if let Some(bn) = self.node_of(v)
                    && let g = self.nodes[bn].group
                    && g != usize::MAX
                    && !self.groups[g].closures.is_empty()
                {
                    self.emit_group(g);
                    return self.nodes[bn].loc.clone();
                }
                h.arena().get(b, crate::heap::cells::BoxCell::VALUE)
            }
            _ => v,
        };
        if inner.is_uninitialized() {
            return "$dump.TDZ".into();
        }
        self.expr(inner, None, None)
    }
}

impl Dumper<'_> {
    /// The names of the sites of `site`'s kind at its offset in `f`, in order (a `for
    /// await`'s closes: of an exception, of a jump out, of a `break`, the last two where
    /// used; an async `yield*`'s awaits of the results of `next`, `return`, `throw`).
    fn peer_sites(&mut self, f: Obj, site: &Site) -> Vec<String> {
        let h = self.h;
        let t = h.closure_template(f);
        let info = template::debug_info(h, t);
        let Some(maps) = self.script_maps(t) else {
            return Vec::new();
        };
        let Some(m) = maps
            .iter()
            .find(|m| m.start == info.src_start && m.end == info.src_end)
        else {
            return Vec::new();
        };
        m.sites
            .iter()
            .filter(|x| x.kind == site.kind && x.pos == site.pos)
            .map(|x| x.name.clone())
            .collect()
    }
}

/// A `using` resource as the original added it: a continuation adds it again through
/// `$djs.U(value, method)` (its dispose method a closure of those two), which is read
/// back here so re-dumps add the original again.
fn unwrap_using(h: &Heap, v: Value, m: Value) -> (Value, Value) {
    let wrapped = || {
        let f = m.as_object()?;
        if h.object_class(f) != ObjectClass::Function {
            return None;
        }
        let ov = captured_value(h, f, "$djs_v")?;
        let om = captured_value(h, f, "$djs_m")?;
        Some((ov, om))
    };
    wrapped().unwrap_or((v, m))
}

/// The home object of method `f` (one that uses `super`).
pub(super) fn home_object(h: &Heap, f: Obj) -> Option<Obj> {
    if h.object_class(f) != ObjectClass::Function {
        return None;
    }
    let t = h.closure_template(f);
    let a = h.arena();
    if u32::from(a.get::<u16>(t, FuncTemplate::FLAGS)) & func_flags::HOME == 0 {
        return None;
    }
    let captures = u32::from(a.get::<u16>(t, FuncTemplate::CAPTURES));
    h.object_slot(f, captures).as_object()
}

fn s_state_awaiting(s: &suspend::Suspended) -> bool {
    s.state == suspend::AWAITING
}

/// Whether a function's parameter list (in its source text) runs no code when called:
/// no defaults, no destructuring.
fn simple_params(src: &str) -> bool {
    let Some(open) = src.find('(') else {
        return false;
    };
    let mut depth = 0;
    for c in src.get(open..).unwrap_or_default().chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            '=' | '{' | '[' | '`' | '"' | '\'' | '/' => return false,
            _ => {}
        }
    }
    false
}

/// A class's hidden bindings in the frame that defines it: its instance fields'
/// initializer, computed field keys, private methods' closures, decorator state. The
/// class statement writes them and only the class's own closures read them, through
/// their capture of the box; the dump writes the class from its text, which makes its
/// own. So after the statement the frame's slot is dead and is written as undefined
/// (a statement run again, in a loop, writes it again).
pub(super) fn class_hidden(name: &str) -> bool {
    [
        "<fields ",
        "<key ",
        "<#",
        "#",
        "<get #",
        "<set #",
        "<class decs>",
        "<dec>",
        "<arg ",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// Where the iterator of a loop's or `yield*`'s `next` slot is (an async-from-sync
/// iterator brings its own `next`).
fn from_sync_partner(named: &[NamedSlot], next: &NamedSlot) -> Option<Place> {
    let want = next.partner("iterator")?;
    named.iter().find(|x| x.name == want).map(|x| x.place)
}

/// Hidden locals the loader makes itself.
pub(super) fn computed_hidden(name: &str) -> bool {
    matches!(
        name,
        "<state>" | "this" | "new.target" | "<active>" | "<home>"
    )
}

/// Whether lexical binding `slot` of `site`'s frame has not been declared in its scope's
/// current entry: its declarator holds the site or comes after it. Its local can still
/// hold what an earlier entry left there (a loop's earlier iteration) when the compiler
/// saw no read that needs the TDZ marker: that value is not the binding's (0215, a
/// first iteration's object made its class's scope ambiguous).
fn undeclared_at(sx: &crate::compiler::Syntax, site: &Site, slot: &Slot, v: Value) -> bool {
    let Slot::Binding { name, scope } = slot else {
        return false;
    };
    // (A boxed binding's box is made at its scope's entry: uninitialized until then.)
    if v.as_internal().is_some() {
        return false;
    }
    use crate::compiler::{DeclKind as K, NodeKind, Role};
    for (i, step) in site.path.iter().enumerate() {
        let Some(nd) = sx.nodes.get(step.node as usize) else {
            continue;
        };
        let Some(d) = nd
            .decls
            .iter()
            .find(|d| d.name == *name && d.scope == *scope)
        else {
            continue;
        };
        if !matches!(
            d.kind,
            K::Let | K::Const | K::Class | K::Using | K::AwaitUsing
        ) {
            return false;
        }
        let (Some(dp), Some(sp)) = (d.part, step.part) else {
            return false;
        };
        if dp != sp {
            return dp > sp;
        }
        // Declared by the statement that holds the site: by a later declarator, or by
        // the one whose initializer holds it. (A pattern the site is in has run part of
        // its declarator: its bindings are left as they are.)
        let Some(next) = site.path.get(i + 1) else {
            return false;
        };
        let Some(decl) = sx.nodes.get(next.node as usize) else {
            return false;
        };
        let (NodeKind::Decl(_), Some(k)) = (decl.kind, next.part) else {
            return false;
        };
        let k = usize::from(k);
        let Some(tp) = decl
            .parts
            .iter()
            .position(|p| p.role == Role::Target && p.start <= d.at && d.at < p.end)
        else {
            return false;
        };
        return tp > k || (tp + 1 == k && decl.parts.get(k).is_some_and(|p| p.role == Role::Init));
    }
    false
}

/// The source, parameters and option bits of Script `script`.
fn script_inputs(h: &Heap, script: u32) -> (Option<String>, String, u32) {
    let a = h.arena();
    let params: Ref = a.get(script, Script::PARAMS);
    let params = params
        .get()
        .map(|s| super::text_of(h, crate::value::Str(s)))
        .unwrap_or_default();
    (
        template::script_text(h, script),
        params,
        a.get::<u32>(script, Script::OPTIONS),
    )
}

/// The heap's lazy templates and who made each template (D2994), from one walk.
pub(super) struct LazyIndex {
    /// `(script, start, end, template)` of every lazy template.
    lazy: Vec<(u32, u32, u32, u32)>,
    /// `(child, parent)`, by child: the template whose children list holds it, or for a
    /// lazily compiled template, its lazy template's.
    parents: Vec<(u32, u32)>,
}

impl LazyIndex {
    fn new(h: &Heap) -> LazyIndex {
        use crate::heap::CellKind;
        use crate::heap::cells::FuncTemplate;
        let lazy_code = [crate::bytecode::Op::Lazy as u8];
        let mut lazy = Vec::new();
        let mut parents = Vec::new();
        let mut compiled = Vec::new();
        for c in h.cells().filter(|c| c.kind == CellKind::FuncTemplate) {
            let t = c.offset;
            let info = template::debug_info(h, t);
            if template::code(h, t) == lazy_code {
                if let Some(s) = info.script.get() {
                    lazy.push_out((s, info.src_start, info.src_end, t));
                }
                if let Some(x) = h.arena().get::<Ref>(t, FuncTemplate::IC).get() {
                    compiled.push_out((x, t));
                }
            }
            for k in 0..template::child_count(h, t) {
                if let Some(ch) = template::child(h, t, k).and_then(|r| r.get()) {
                    parents.push_out((ch, t));
                }
            }
        }
        crate::sort::sort_by(&mut parents, |a, b| a.0 < b.0);
        let mut index = LazyIndex { lazy, parents };
        // A compiled template's parent is its lazy template's.
        let more: Vec<(u32, u32)> = compiled
            .iter()
            .filter_map(|&(x, t)| index.parent(t).map(|p| (x, p)))
            .collect();
        if !more.is_empty() {
            index.parents.extend(more);
            crate::sort::sort_by(&mut index.parents, |a, b| a.0 < b.0);
        }
        index
    }

    /// The innermost lazy template of `script` whose span holds `a..b`.
    fn root(&self, script: u32, a: u32, b: u32) -> Option<u32> {
        self.lazy
            .iter()
            .filter(|l| l.0 == script && l.1 <= a && b <= l.2)
            .min_by_key(|l| l.2 - l.1)
            .map(|l| l.3)
    }

    fn parent(&self, t: u32) -> Option<u32> {
        let i = self.parents.partition_point(|e| e.0 < t);
        self.parents.get(i).filter(|e| e.0 == t).map(|e| e.1)
    }
}
