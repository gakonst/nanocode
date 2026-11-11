//! The embedding API (ARCH "Execution API").
//!
//! ```text
//! Vm::new(opts)                      a fresh VM: heap, realm, interpreter state
//! vm.instantiate(template)           a closure for a compiled top-level template
//! vm.call(f, this, args)             pushes the entry frame; does not run it
//! vm.run(fuel) -> Status             Done | Threw | OutOfFuel | HostCall | Idle
//!                                    (the job queue runs after the entry call, D1370)
//! vm.host_call(id)                   the fn id, `this` and arguments of a parked call
//! vm.resolve_host_call(id, result)   completes it; then `run` again
//! vm.bytes() / Vm::from_bytes(..)    the whole VM as a position-independent image
//! ```
//!
//! Values returned by the API (`Status::Done(v)`, `host_call` arguments) are raw heap
//! references: valid until the next `run` or collection. Keep them in a
//! [`Handle`](crate::heap::Handle) to hold them longer.

#[cfg(feature = "dump")]
mod call_site;
pub mod compat;
#[cfg(feature = "compile-cache")]
pub(crate) mod compile_cache;
#[cfg(feature = "dump")]
pub mod dump;
#[cfg(feature = "dump")]
mod dump_classes;
pub mod dump_runtime;
#[cfg(feature = "dump")]
mod dump_state;
mod format;
mod image;
mod module;
pub mod registry;

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::cells::{HostCall, Interp, ValueArray};
use crate::heap::{
    Heap, HeapError, LoadError, ObjectClass, PropFlags, PropertyDescriptor, Ref, atoms,
};
use crate::interp::realm::{self, Intrinsic, define_hidden, intrinsic_obj};
use crate::interp::run::{self, Exec};
use crate::interp::{Continuation, state};
pub use crate::interp::{ErrorKind, ExecOptions, RunState, Status};
use crate::value::{Obj, Str, Value};
#[cfg(feature = "dump")]
pub use call_site::CallSite;
pub use compat::{COMPAT_VERSION, FEATURES, IMAGE_DEPS, ImageDep, ImageDeps, RECORDS_HASH};
pub use module::{IMPORT_FN_ID, ModuleKind, ModuleLoader, ModuleSource};

/// `HostCall::FLAGS`: returned by `Vm::new_host_calls` already.
const HOST_CALL_REPORTED: u32 = 1;

/// The generated prelude (`builtins/gen/prelude.js`, D1172); the slim build's leaves out
/// the families it doesn't have (D2411), and full builds without `embed` leave out the
/// builtins tagged `// @embed-only` (D3175).
#[cfg(all(feature = "full", feature = "embed"))]
const PRELUDE: &str = include_str!("../../../../builtins/gen/prelude.js");
#[cfg(all(feature = "full", not(feature = "embed")))]
const PRELUDE: &str = include_str!("../../../../builtins/gen/prelude-noembed.js");
#[cfg(not(feature = "full"))]
const PRELUDE: &str = include_str!("../../../../builtins/gen/prelude-slim.js");

// `SOURCE_HASH`: the library's non-test sources, djs-libm's and the prelude (build.rs,
// D1330, D3174) and the enabled cargo features (`BUILD_INPUTS`, D1750).
include!(concat!(env!("OUT_DIR"), "/source_hash.rs"));

/// The engine build hash (D1320, D1330): the heap layout (with the well-known atoms),
/// the instruction table, the intrinsic ids and the source hash. It identifies the build
/// (`djs_build_hash`; Sandbox journals replay only on it). Images are not keyed on it
/// since D026: they carry their dependency descriptor ([`ImageDeps`], D2265) instead.
pub const BUILD_HASH: u32 = {
    use crate::bytecode::intrinsics::INTRINSICS_HASH;
    use crate::bytecode::{OPS_HASH, build_hash};
    build_hash(
        build_hash(
            build_hash(crate::heap::cells::LAYOUT_HASH, OPS_HASH),
            INTRINSICS_HASH,
        ),
        SOURCE_HASH,
    )
};

/// What WeakRef and FinalizationRegistry do (D1680). Collection timing depends on the heap
/// cap and the GC policy, so a host that needs runs to repeat exactly pins them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeakRefs {
    /// Weak, as the spec intends: targets die in collections, and FinalizationRegistry
    /// callbacks run afterwards (D1513).
    #[default]
    Collected,
    /// Strong: `deref` always returns the target and cleanup callbacks never run, whenever
    /// collections run (the spec allows an engine that never collects). Applies to the
    /// WeakRefs and registry cells created afterwards.
    Strong,
    /// The globals `WeakRef` and `FinalizationRegistry` are deleted (as pi's quickjs VM
    /// leaves them out in deterministic runs). Not reversible.
    Absent,
}

/// VM configuration.
#[derive(Clone, Copy, Debug)]
pub struct VmOptions {
    /// The heap's memory cap in bytes.
    pub memory_limit: usize,
    pub exec: ExecOptions,
    /// Install the builtins of `builtins/gen/prelude.js` at creation (default). Off gives
    /// a bare realm (interpreter tests, measuring the prelude's cost).
    pub prelude: bool,
    /// The seed of `Math.random` (ARCH "Nondeterminism": the host decides; 0 means the
    /// fixed default). Applied by `Vm::new` and after copying a base (`from_base`); a
    /// restored snapshot (`from_bytes`) continues its own sequence.
    pub random_seed: u64,
    /// WeakRef and FinalizationRegistry (D1680). Applied by `Vm::new` and `from_base`; a
    /// restored image keeps what it had (the mode is heap state).
    pub weak_refs: WeakRefs,
    /// Frozen builtins (D022, D2304): the builtin objects of the base image behave as
    /// frozen (writes refused, descriptors non-writable and non-configurable, not
    /// extensible), and an assignment that meets one of their read-only properties on
    /// another object's prototype chain defines an own property on that object (the
    /// override fix of SES and Node's `--frozen-intrinsics`). The global object stays
    /// ordinary. Applied by `Vm::new` and `from_base`; a restored image keeps its mode.
    pub frozen_builtins: bool,
    /// Arms the prefix write barrier (D2065) without frozen builtins: in debug builds, a
    /// write into the builtin prefix panics. For test suites whose scripts never change
    /// builtins (TESTING.md lists them); frozen builtins arm it anyway. Not part of the
    /// image: applied at creation and restore. Release builds ignore it.
    pub prefix_barrier: bool,
    /// Lazy functions (D2990, builds with the `lazy` feature): `Vm::compile` parses the
    /// whole source (every early error is reported) but compiles a function's body at its
    /// first call. Not part of the image: a VM's lazy templates compile in any lazy build.
    #[cfg(feature = "lazy")]
    pub lazy_functions: bool,
}

impl Default for VmOptions {
    fn default() -> Self {
        VmOptions {
            memory_limit: crate::heap::DEFAULT_LIMIT,
            exec: ExecOptions::default(),
            prelude: true,
            random_seed: crate::builtins::number::DEFAULT_RANDOM_SEED,
            weak_refs: WeakRefs::Collected,
            frozen_builtins: false,
            prefix_barrier: false,
            #[cfg(feature = "lazy")]
            lazy_functions: false,
        }
    }
}

/// Misuse of the API.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VmError {
    /// `call` while a call is in progress (running or parked).
    Busy,
    /// `resolve_host_call` with an id the VM is not parked on.
    UnknownHostCall,
    Heap(HeapError),
}

impl From<HeapError> for VmError {
    fn from(e: HeapError) -> Self {
        VmError::Heap(e)
    }
}

/// A pending host call, as the embedder sees it: the call the VM is parked on, or an
/// async call (D1374).
#[derive(Clone, Debug)]
pub struct HostCallInfo {
    pub fn_id: u32,
    pub this: Value,
    pub args: Vec<Value>,
    /// An async host call: the script got a promise and went on.
    pub is_async: bool,
}

/// The collections an embedder can ask for.
#[derive(Clone, Copy)]
enum Collection {
    Full,
    Seal,
    Canonical,
}

/// A VM: a heap with a realm and interpreter state. Everything is in the heap; the
/// struct only holds options.
pub struct Vm {
    heap: Heap,
    opts: VmOptions,
    /// The base image's builtins as dumps name them (`dump::BaseTable`): made by the
    /// first `dump` and kept, with the length of the base it was made from (D2626).
    #[cfg(feature = "dump")]
    base_names: core::cell::OnceCell<(usize, alloc::sync::Arc<dump::BaseTable>)>,
    /// What dumps keep for the next dump: the suspension maps of their scripts.
    #[cfg(feature = "dump")]
    dump_cache: core::cell::RefCell<dump::Cache>,
}

impl Vm {
    pub fn new(opts: VmOptions) -> Result<Vm, HeapError> {
        let mut heap = Heap::with_limit(opts.memory_limit)?;
        IMAGE_DEPS.write(&mut heap);
        state::init(&mut heap)?;
        realm::init(&mut heap)?;
        let mut vm = Vm {
            heap,
            opts,
            #[cfg(feature = "dump")]
            base_names: Default::default(),
            #[cfg(feature = "dump")]
            dump_cache: Default::default(),
        };
        crate::builtins::number::set_seed(&mut vm.heap, opts.random_seed);
        if opts.prelude {
            #[cfg(djs_prelude)]
            vm.install_prelude()?;
            // A wasm build without the `prelude` feature starts VMs from the base image
            // (D3094).
            #[cfg(not(djs_prelude))]
            panic!(
                "this build cannot run the prelude: Vm::new needs djs-core's `prelude` feature in wasm"
            );
        }
        vm.set_weak_refs(opts.weak_refs)?;
        if opts.frozen_builtins {
            // Without a base image, the realm is made into one: the builtin prefix with
            // the per-VM cells above it (D2063).
            if vm.heap.base_end() == 0 {
                vm.prepare_base_image()?;
            }
            vm.heap.set_frozen_builtins(true);
        }
        vm.heap.force_prefix_barrier(opts.prefix_barrier);
        Ok(vm)
    }

    /// Compiles the generated prelude in privileged mode and runs it (D1172, D1281): it
    /// installs the builtin methods and registers the `@intrinsic` builtins. Compiled at
    /// every VM creation until there is a base snapshot (ARCH G2).
    #[cfg(djs_prelude)]
    fn install_prelude(&mut self) -> Result<(), HeapError> {
        use crate::compiler::{CompileErrorKind, CompileOptions};
        let opts = CompileOptions::privileged_script("<prelude>");
        let template = match crate::compiler::compile(&mut self.heap, PRELUDE.as_bytes(), &opts) {
            Ok(t) => t,
            Err(e) if e.kind == CompileErrorKind::OutOfMemory => {
                return Err(HeapError::OutOfMemory);
            }
            Err(e) if e.kind == CompileErrorKind::HostOutOfMemory => {
                return Err(HeapError::HostOutOfMemory);
            }
            Err(e) => panic!("the prelude does not compile: {e}"),
        };
        // Privileged templates carry no script (D1980); traces know them by their flag.
        if let Some(script) = crate::bytecode::template::debug_info(&self.heap, template.0)
            .script
            .get()
        {
            let r = realm::realm(&self.heap);
            self.heap.arena_mut().write(
                crate::heap::cells::Realm::INTRINSICS.at(r, Intrinsic::PreludeScript as u32),
                Value::internal(script),
            );
        }
        let f = self.instantiate(template)?;
        let this = Value::object(self.global());
        self.call(f, this, &[])
            .map_err(|_| HeapError::OutOfMemory)?;
        match self.run(u64::MAX) {
            Status::Done(_) => {}
            Status::Threw(e)
                if e == crate::interp::intrinsic(&self.heap, Intrinsic::OutOfMemoryError) =>
            {
                return Err(HeapError::OutOfMemory);
            }
            s => {
                let shown = match s {
                    Status::Threw(e) => self.display(e),
                    _ => alloc::format!("{s:?}"),
                };
                panic!("the prelude failed: {shown}");
            }
        }
        state::set_state(&mut self.heap, RunState::Idle);
        Ok(())
    }

    /// Restores a VM from [`Vm::bytes`] of a build with the same dependency descriptor
    /// ([`ImageDeps`], D2265), in any state (including
    /// suspended mid-call or parked on a host call). The whole image is verified: for the
    /// stress modes and tools, which check the heap at every restore.
    pub fn from_bytes(bytes: Vec<u8>, opts: VmOptions) -> Result<Vm, LoadError> {
        Vm::check_image(&bytes)?;
        Vm::over(Heap::from_bytes(bytes, opts.memory_limit)?, opts)
    }

    /// [`Vm::from_bytes`] for an image this process made (a wasm host's module
    /// transport, pi's prelude images): only the header and the dependency descriptor are
    /// checked, not every cell. Images are not a persistence format (D2700): nothing
    /// loads one from outside the process, so nothing needs the verification there.
    pub fn from_bytes_trusted(bytes: Vec<u8>, opts: VmOptions) -> Result<Vm, LoadError> {
        Vm::check_image(&bytes)?;
        Vm::over(Heap::from_bytes_trusted(bytes, opts.memory_limit)?, opts)
    }

    /// Replaces this VM's state by an image this process made, as
    /// [`Vm::from_bytes_trusted`] would restore it, but keeps what the VM holds outside
    /// its heap (its options, and what it derived from the base once): for a host that
    /// dumps many images with one VM (a wasm host's dump module, D2701).
    pub fn replace_image(&mut self, bytes: Vec<u8>) -> Result<(), LoadError> {
        let fresh = Vm::from_bytes_trusted(bytes, self.opts)?;
        self.heap = fresh.heap;
        Ok(())
    }

    /// [`Vm::replace_image`] from a borrowed image, copied into this VM's own arena
    /// buffer: one copy and, once the buffer is big enough, no allocation (D3182). For an
    /// embedder that resets a VM to the same snapshot after every call (djs-embed). On an
    /// error the VM has no heap left and must be dropped.
    #[cfg(feature = "embed")]
    pub fn reset_to(&mut self, image: &[u8]) -> Result<(), LoadError> {
        Vm::check_image(image)?;
        let mut bytes = self.heap.arena_mut().take_bytes();
        bytes.clear();
        bytes
            .try_reserve_exact(image.len())
            .map_err(|_| LoadError::TooLarge)?;
        bytes.extend_from_slice(image);
        let mut fresh = Vm::over(
            Heap::from_bytes_trusted(bytes, self.opts.memory_limit)?,
            self.opts,
        )?;
        // What the heap derived for itself and keeps for the VM's lifetime.
        #[cfg(feature = "unicode-normalization")]
        {
            fresh.heap.compositions = core::mem::take(&mut self.heap.compositions);
        }
        core::mem::swap(&mut self.heap, &mut fresh.heap);
        Ok(())
    }

    /// A new VM from a base snapshot: the image of an idle VM with its prelude installed,
    /// made at build time (`djs-prelude`, D1321). Creating a VM is then a copy, with no
    /// compile. The image is trusted (its cells are not verified: it was verified when it
    /// was built), but its header and dependency descriptor are checked.
    ///
    /// What the base holds: the realm (global object, intrinsics, builtins), the atoms
    /// and the compiled prelude. Nothing per VM: no handles, no host functions, no
    /// pending calls. Per-VM state is set up after the copy (see `after_copy`).
    pub fn from_base(base: &[u8], opts: VmOptions) -> Result<Vm, LoadError> {
        Vm::from_base_reserved(base, opts, 0)
    }

    /// [`Vm::from_base`] with the arena capacity a heap of `live` bytes would get (its
    /// next GC threshold plus the slack): for an embedder that knows how big the heap
    /// gets, such as pi's cold start, which runs its prelude next (D2043). Growing later
    /// would reallocate the arena, and in wasm the old block stays behind as a hole.
    pub fn from_base_reserved(base: &[u8], opts: VmOptions, live: usize) -> Result<Vm, LoadError> {
        Vm::check_image(base)?;
        // Copied into a buffer with the arena's capacity, so the heap does not reallocate
        // it right away (D1942).
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(Heap::start_capacity(
                base.len(),
                Heap::image_base_end(base),
                live,
                opts.memory_limit,
            ))
            .map_err(|_| LoadError::TooLarge)?;
        bytes.extend_from_slice(base);
        Vm::from_base_vec(bytes, opts, live)
    }

    /// [`Vm::from_base_reserved`] on a buffer the caller owns, which becomes the arena:
    /// no copy (D2060). A wasm instance decodes or receives the base straight into it.
    pub fn from_base_vec(
        mut bytes: Vec<u8>,
        opts: VmOptions,
        live: usize,
    ) -> Result<Vm, LoadError> {
        Vm::check_image(&bytes)?;
        let cap = Heap::start_capacity(
            bytes.len(),
            Heap::image_base_end(&bytes),
            live,
            opts.memory_limit,
        );
        bytes
            .try_reserve_exact(cap.saturating_sub(bytes.len()))
            .map_err(|_| LoadError::TooLarge)?;
        let heap = Heap::from_bytes_trusted(bytes, opts.memory_limit)?;
        let mut vm = Vm::over(heap, opts)?;
        if vm.state() != RunState::Idle || vm.heap.handle_count() != 0 {
            return Err(LoadError::BadHeader);
        }
        vm.after_copy().map_err(|_| LoadError::TooLarge)?;
        Ok(vm)
    }

    /// Per-VM initialization after copying a base: state that must not be shared between
    /// VMs or baked into the base: the `Math.random` seed and the weak-reference mode.
    fn after_copy(&mut self) -> Result<(), HeapError> {
        let seed = self.opts.random_seed;
        crate::builtins::number::set_seed(&mut self.heap, seed);
        self.set_weak_refs(self.opts.weak_refs)?;
        self.heap.set_frozen_builtins(self.opts.frozen_builtins);
        self.heap.force_prefix_barrier(self.opts.prefix_barrier);
        Ok(())
    }

    /// Whether the builtins are frozen ([`VmOptions::frozen_builtins`], D2304).
    pub fn frozen_builtins(&self) -> bool {
        self.heap.frozen_builtins()
    }

    /// Sets what WeakRef and FinalizationRegistry do (D1680); set it before running code.
    /// `Strong` affects the weak references created afterwards, `Absent` deletes the two
    /// globals, `Collected` makes new ones weak again (it cannot bring back deleted
    /// globals).
    pub fn set_weak_refs(&mut self, mode: WeakRefs) -> Result<(), HeapError> {
        self.heap.set_weak_refs_strong(mode == WeakRefs::Strong);
        if mode == WeakRefs::Absent {
            let g = self.global();
            for name in ["WeakRef", "FinalizationRegistry"] {
                let key = self.heap.atom_str(name)?;
                self.heap.delete_own_property(g, key)?;
            }
        }
        Ok(())
    }

    /// The dependency descriptor (D2265), before anything else: an image this build
    /// cannot run must be rejected with the component that differs, not fail
    /// verification somewhere in the middle.
    fn check_image(bytes: &[u8]) -> Result<(), LoadError> {
        let deps = ImageDeps::of(bytes).ok_or(LoadError::BadHeader)?;
        match deps.first_difference(&IMAGE_DEPS) {
            Some((dep, found, expected)) => Err(LoadError::Incompatible {
                dep,
                found,
                expected,
            }),
            None => Ok(()),
        }
    }

    fn over(mut heap: Heap, opts: VmOptions) -> Result<Vm, LoadError> {
        if state::interp(&heap) == 0 || heap.realm().as_internal().is_none() {
            return Err(LoadError::BadHeader);
        }
        heap.force_prefix_barrier(opts.prefix_barrier);
        Ok(Vm {
            heap,
            opts,
            #[cfg(feature = "dump")]
            base_names: Default::default(),
            #[cfg(feature = "dump")]
            dump_cache: Default::default(),
        })
    }

    /// The arena as it is: the complete VM state, including dead value-stack slots above
    /// the stack pointer that the next instructions overwrite. For images use
    /// [`Vm::image`].
    pub fn bytes(&self) -> &[u8] {
        self.heap.bytes()
    }

    /// The VM image (D2308): [`Vm::bytes`] with the value-stack slots above the stack
    /// pointer cleared, so that an image holds nothing a run left behind there and equal
    /// states give equal images however they were sliced. The slots are dead: the
    /// interpreter writes before it reads them, and every collection clears them before it
    /// traces the stack, so clearing them here changes nothing the VM does afterwards.
    pub fn image(&mut self) -> &[u8] {
        tidy_stack(&mut self.heap, false);
        self.heap.bytes()
    }

    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    pub fn options(&self) -> VmOptions {
        self.opts
    }

    pub fn set_options(&mut self, opts: VmOptions) {
        self.opts = opts;
    }

    pub fn state(&self) -> RunState {
        state::state(&self.heap)
    }

    /// The global object.
    pub fn global(&self) -> Obj {
        intrinsic_obj(&self.heap, Intrinsic::Global)
    }

    /// The global lexical record: Script-level `let`/`const`/`class` bindings as data
    /// properties of a hidden object (D1220). For tools that read the heap (the REPL's
    /// completion); an uninitialized binding holds `Value::UNINITIALIZED`.
    pub fn global_lexical(&self) -> Obj {
        crate::interp::realm::global_lex(&self.heap)
    }

    /// A closure for a top-level `FuncTemplate` (one without captures).
    pub fn instantiate(&mut self, template: Ref) -> Result<Value, HeapError> {
        let proto = intrinsic_obj(&self.heap, Intrinsic::FunctionProto);
        let f = self.heap.alloc_closure(Some(proto), template.0, 0)?;
        Ok(Value::object(f))
    }

    /// Compiles `src` (UTF-8) and returns the top-level function, ready for
    /// [`Vm::call`]. Call it with `this` = the global object; in
    /// [`Goal::AsyncBody`](crate::compiler::Goal) mode, pass the parameters as arguments.
    /// Until async functions exist, an async body runs as a plain function body.
    pub fn compile(
        &mut self,
        src: &[u8],
        opts: &crate::compiler::CompileOptions<'_>,
    ) -> Result<Value, crate::compiler::CompileError> {
        // A plain dump (it starts with its directive) is compiled with its suspension
        // maps, kept for this VM's next dump: its continuations are suspended in it, and
        // compiling all of it again there was a third of a re-dump (0215). Lazily, the
        // maps of what the load compiles; a dump asks for the others (D2994).
        #[cfg(feature = "dump")]
        let template = if src.starts_with(b"\"djs-dump") && opts.params.is_empty() {
            #[cfg(feature = "lazy")]
            let lazy = self.opts.lazy_functions;
            #[cfg(not(feature = "lazy"))]
            let lazy = false;
            let (t, maps, globals) = self.compile_with_maps(src, opts, lazy)?;
            if let Ok(text) = core::str::from_utf8(src) {
                let globals = self.global_texts(globals);
                self.dump_cache
                    .borrow_mut()
                    .keep(opts.bits(), text, maps, lazy, globals);
            }
            t
        } else {
            self.compile_unit(src, opts)?
        };
        #[cfg(not(feature = "dump"))]
        let template = self.compile_unit(src, opts)?;
        self.instantiate(template)
            .map_err(|e| crate::compiler::CompileError {
                kind: if e == HeapError::HostOutOfMemory {
                    crate::compiler::CompileErrorKind::HostOutOfMemory
                } else {
                    crate::compiler::CompileErrorKind::OutOfMemory
                },
                message: String::from(e.as_str()),
                pos: 0,
                line: 0,
                col: 0,
            })
    }

    /// Compiles every lazy template in the heap that has not run yet (D2990), the ones
    /// inside them too. Their closures switch to the compiled template at their next
    /// call.
    #[cfg(feature = "lazy")]
    pub fn compile_lazy_templates(&mut self) -> Result<(), HeapError> {
        use crate::heap::CellKind;
        use crate::heap::cells::FuncTemplate;
        let lazy = [crate::bytecode::Op::Lazy as u8];
        loop {
            let h = &self.heap;
            let todo: Vec<u32> = h
                .cells()
                .filter(|c| {
                    c.kind == CellKind::FuncTemplate
                        && h.arena()
                            .get::<Ref>(c.offset, FuncTemplate::IC)
                            .get()
                            .is_none()
                        && crate::bytecode::template::code(h, c.offset) == lazy
                })
                .map(|c| c.offset)
                .collect();
            if todo.is_empty() {
                return Ok(());
            }
            for t in todo {
                let c = crate::compiler::compile_lazy(&mut self.heap, Ref(t)).map_err(|e| {
                    if e.kind == crate::compiler::CompileErrorKind::HostOutOfMemory {
                        HeapError::HostOutOfMemory
                    } else {
                        HeapError::OutOfMemory
                    }
                })?;
                self.heap.arena_mut().set(t, FuncTemplate::IC, c);
            }
        }
    }

    /// [`crate::compiler::LazyGlobals`] as text.
    #[cfg(feature = "dump")]
    fn global_texts(
        &self,
        globals: Vec<crate::compiler::LazyGlobals>,
    ) -> Vec<(u32, u32, Vec<String>)> {
        let h = &self.heap;
        globals
            .into_iter()
            .map(|(a, b, g)| (a, b, g.into_iter().map(|g| h.atom_name(g)).collect()))
            .collect()
    }

    /// A plain dump's compile with its suspension maps, all of them or (`lazy`) those of
    /// the functions the load compiles (D2994).
    #[cfg(feature = "dump")]
    fn compile_with_maps(
        &mut self,
        src: &[u8],
        opts: &crate::compiler::CompileOptions<'_>,
        lazy: bool,
    ) -> Result<
        (
            Ref,
            Vec<crate::compiler::FunctionMap>,
            Vec<crate::compiler::LazyGlobals>,
        ),
        crate::compiler::CompileError,
    > {
        #[cfg(feature = "lazy")]
        if lazy {
            return crate::compiler::compile_lazy_with_maps(&mut self.heap, src, opts);
        }
        let _ = lazy;
        crate::compiler::compile_with_maps(&mut self.heap, src, opts)
            .map(|(t, m)| (t, m, Vec::new()))
    }

    /// Compiles `src`, lazily with `VmOptions::lazy_functions` (D2990).
    fn compile_unit(
        &mut self,
        src: &[u8],
        opts: &crate::compiler::CompileOptions<'_>,
    ) -> Result<Ref, crate::compiler::CompileError> {
        #[cfg(all(feature = "lazy", feature = "dump"))]
        if self.opts.lazy_functions {
            // The globals of its lazy functions, for a dump of this VM (D2993).
            let (t, globals) =
                crate::compiler::compile_lazy_with_globals(&mut self.heap, src, opts)?;
            if !globals.is_empty()
                && let Ok(text) = core::str::from_utf8(src)
            {
                let globals = self.global_texts(globals);
                let params = opts.params.join(",");
                self.dump_cache
                    .borrow_mut()
                    .keep_globals(opts.bits(), &params, text, globals);
            }
            return Ok(t);
        }
        #[cfg(all(feature = "lazy", not(feature = "dump")))]
        if self.opts.lazy_functions {
            return crate::compiler::compile_lazy_unit(&mut self.heap, src, opts);
        }
        crate::compiler::compile(&mut self.heap, src, opts)
    }

    /// A function implemented by the embedder: calling it parks the VM with
    /// `Status::HostCall { fn_id, .. }`.
    pub fn new_host_function(&mut self, fn_id: u32, name: &str) -> Result<Value, HeapError> {
        let proto = intrinsic_obj(&self.heap, Intrinsic::FunctionProto);
        let f = self
            .heap
            .alloc_object(ObjectClass::HostFunction, Some(proto), 1)?;
        self.heap.set_object_slot(f, 0, Value::int(fn_id as i32));
        let n = self.heap.alloc_str(name)?;
        let d = PropertyDescriptor::data(Value::string(n), PropFlags::CONFIGURABLE);
        self.heap.define_own_property(f, atoms::NAME, &d)?;
        Ok(Value::object(f))
    }

    /// An async host function (D1374): calling it returns a promise at once and adds a
    /// pending call (see [`Vm::new_host_calls`]); the VM goes on running. Resolving the
    /// call with [`Vm::resolve_host_call`] settles the promise.
    pub fn new_async_host_function(&mut self, fn_id: u32, name: &str) -> Result<Value, HeapError> {
        let f = self.new_host_function(fn_id, name)?;
        let o = f.as_object().expect("a function object");
        let flags = self.heap.cell_flags(o.offset());
        self.heap.set_cell_flags(
            o.offset(),
            flags | crate::heap::cells::flags::OBJ_HOST_ASYNC,
        );
        Ok(f)
    }

    /// The async host calls made since the last `new_host_calls`, oldest first. Read
    /// them whenever `run` returns, with [`Vm::host_call`], and start them.
    pub fn new_host_calls(&mut self) -> Vec<u32> {
        let calls = self.async_calls();
        let mut out = Vec::new();
        for c in calls {
            let flags = self.heap.arena().get(c, HostCall::FLAGS);
            if flags & HOST_CALL_REPORTED == 0 {
                self.heap
                    .arena_mut()
                    .set(c, HostCall::FLAGS, flags | HOST_CALL_REPORTED);
                out.push(self.heap.arena().get(c, HostCall::ID));
            }
        }
        out
    }

    /// Every async host call that is not resolved yet, oldest first (after a restore,
    /// the embedder re-issues or replays these).
    pub fn pending_host_calls(&self) -> Vec<u32> {
        self.async_calls()
            .into_iter()
            .map(|c| self.heap.arena().get(c, HostCall::ID))
            .collect()
    }

    /// The `HostCall` cells of the pending async calls.
    fn async_calls(&self) -> Vec<u32> {
        let table = state::host_calls(&self.heap);
        (0..self.heap.value_array_len(table))
            .map(|i| self.heap.value_array_get(table, i).as_internal().unwrap())
            .filter(|&c| {
                self.heap
                    .arena()
                    .get::<Value>(c, HostCall::PROMISE)
                    .as_object()
                    .is_some()
            })
            .collect()
    }

    /// A new ordinary object.
    pub fn new_object(&mut self) -> Result<Obj, HeapError> {
        let proto = intrinsic_obj(&self.heap, Intrinsic::ObjectProto);
        self.heap.new_object(Some(proto))
    }

    /// Defines a property of an object the way builtins are defined (writable,
    /// configurable, not enumerable).
    pub fn define(&mut self, o: Obj, name: &str, v: Value) -> Result<(), HeapError> {
        let key = self.heap.atom_str(name)?;
        define_hidden(&mut self.heap, o, key, v)
    }

    /// Defines a global binding (a property of the global object).
    pub fn define_global(&mut self, name: &str, v: Value) -> Result<(), HeapError> {
        let g = self.global();
        self.define(g, name, v)
    }

    /// The value at the dotted `path` from the global object (`tools.read`): own data
    /// properties only, no getters or Proxy traps run. For hosts that bind their values
    /// as globals and resolve a dump's `$djs.host(name)` by the binding (D2971); `None`
    /// if a step is missing.
    pub fn global_path(&mut self, path: &str) -> Option<Value> {
        let mut v = Value::object(self.global());
        for part in path.split('.') {
            let o = v.as_object()?;
            let k = self.heap.atom_str(part).ok()?;
            v = self.heap.get_own_property(o, k)?.value()?;
        }
        Some(v)
    }

    /// A new Error object of `kind`.
    /// Sets the clock that `Date.now()` and `new Date()` read, in ms since the epoch
    /// (ARCH "Nondeterminism", D1383). The VM never reads a system clock: the host sets
    /// it, typically before each run slice. It is heap data, so restores and replays see
    /// the value the host set. Until then it is 0.
    pub fn set_now(&mut self, ms: f64) {
        let v = Value::number(ms);
        crate::interp::realm::set_intrinsic(&mut self.heap, crate::interp::Intrinsic::Clock, v);
    }

    /// Sets the local time zone as a fixed offset in minutes east of UTC (D1383): local
    /// Date methods and toString use it. Default 0 (UTC). There is no daylight saving
    /// rule; a host that cares updates it.
    pub fn set_time_zone_offset(&mut self, minutes: i32) {
        let v = Value::int(minutes);
        crate::interp::realm::set_intrinsic(
            &mut self.heap,
            crate::interp::Intrinsic::TimeZoneOffset,
            v,
        );
    }

    /// Reseeds `Math.random` (see `VmOptions::random_seed`). The state lives in the
    /// heap, so a snapshot continues the same sequence. The key of the VM's string hash
    /// follows the seed (D2433): changing it rehashes the VM's own atoms and Map/Set
    /// tables, which costs time and, against a reference image, snapshot size.
    pub fn set_random_seed(&mut self, seed: u64) {
        crate::builtins::number::set_seed(&mut self.heap, seed);
    }

    /// Reseeds `Math.random` alone, keeping the hash key: for an embedder that keyed the
    /// VM once (pi keys its prelude image and reseeds every session from it, D2433).
    pub fn set_math_random_seed(&mut self, seed: u64) {
        self.set_math_random(seed, false);
    }

    /// Restores a dumped run's `Math.random` state (a plain dump's `$djs.seed(lo, hi)`):
    /// as a state that ran, which the next dump writes again (D2837).
    pub fn restore_math_random(&mut self, state: u64) {
        self.set_math_random(state, true);
    }

    /// [`Vm::set_math_random_seed`], or with `restore` [`Vm::restore_math_random`].
    pub fn set_math_random(&mut self, s: u64, restore: bool) {
        crate::builtins::number::set_math_state(&mut self.heap, s, restore);
    }

    pub fn new_error(&mut self, kind: ErrorKind, message: &str) -> Result<Value, HeapError> {
        realm::new_error(&mut self.heap, kind, message)
    }

    /// A one-byte string of `len` zero units for the embedder to fill in place (0162,
    /// D2551): its code units are the `len` bytes at arena offset
    /// [`Heap::str_data_offset`], one Latin-1 unit per byte, and the embedder fills them
    /// before anything else allocates (which may move the arena). Saves a large host string
    /// the staging copy and the transcode.
    pub fn reserve_string(&mut self, len: u32) -> Result<Str, HeapError> {
        self.heap.alloc_str_uninit(len, false)
    }

    /// Decodes a wire buffer (docs/wire.md) into a value of this VM's realm: objects and
    /// arrays get its prototypes. Allocates but never collects, so the value stays valid
    /// until the next `run` or collection. (Only with `embed`: the wasm modules decode with
    /// `wire::decode`, and even an unused caller moves that function in their code, D3180.)
    #[cfg(feature = "embed")]
    pub fn decode_wire(&mut self, bytes: &[u8]) -> Result<Value, crate::wire::WireError> {
        let opts = crate::wire::DecodeOptions {
            object_proto: Some(intrinsic_obj(&self.heap, Intrinsic::ObjectProto)),
            array_proto: Some(intrinsic_obj(&self.heap, Intrinsic::ArrayProto)),
            ..Default::default()
        };
        crate::wire::decode(&mut self.heap, bytes, &opts)
    }

    /// Encodes plain data as a wire buffer of at most `max_bytes` (`wire::encode`): reads
    /// the heap, never runs JS.
    #[cfg(feature = "embed")]
    pub fn encode_wire(
        &self,
        v: Value,
        max_bytes: usize,
    ) -> Result<Vec<u8>, crate::wire::EncodeError> {
        crate::wire::encode(&self.heap, v, max_bytes)
    }

    /// Starts a call of `f`: pushes the entry frame (or parks on a host function). `run`
    /// executes it. Never collects, so `f`, `this` and the arguments the embedder holds
    /// stay valid until the next `run`.
    pub fn call(&mut self, f: Value, this: Value, args: &[Value]) -> Result<(), VmError> {
        match self.state() {
            RunState::Running | RunState::HostCall | RunState::ThrowPending => {
                return Err(VmError::Busy);
            }
            RunState::Idle | RunState::Done | RunState::Threw => {}
        }
        let i = state::interp(&self.heap);
        for f in [Interp::BP, Interp::HP, Interp::SP, Interp::PC] {
            self.heap.arena_mut().set(i, f, 0);
        }
        // A new call starts without the previous one's overshoot (D1740).
        state::set_fuel_debt(&mut self.heap, 0);
        let argc = u32::try_from(args.len()).map_err(|_| HeapError::OutOfMemory)?;
        state::set_state(&mut self.heap, RunState::Running);
        let mut ex = Exec::new(&mut self.heap, self.opts.exec);
        let r = ex.ensure(2 + argc).and_then(|()| {
            ex.push(f);
            ex.push(this);
            for &a in args {
                ex.push(a);
            }
            ex.call(0, argc, Continuation::Entry, 0)
        });
        match r {
            Ok(_) => ex.flush(),
            Err(fault) => {
                let exc = ex.fault_value(fault, None);
                ex.set_sp(0);
                ex.flush();
                state::set_outcome(&mut self.heap, RunState::Threw, exc);
            }
        }
        Ok(())
    }

    /// Abandons the current call, if any (running, out of fuel, or parked on a host
    /// call): its frames, pending host calls, queued jobs and tracked rejections are
    /// dropped (and delivered results not applied yet) and the VM is `Idle` again,
    /// with its realm and globals intact. For hosts that stop a run that used up its fuel
    /// budget.
    ///
    /// What it guarantees: afterwards the VM is idle and the next [`Vm::call`] starts
    /// cleanly; nothing of the abandoned call runs any more (no frame, no `finally`, no job,
    /// no promise reaction, no host result is applied later); its fuel debt is cleared and
    /// [`Vm::outcome`] is `None`. What it keeps: the heap as the call left it (globals and
    /// objects it changed stay changed, handles stay valid), the clock and the random state.
    /// The abandoned frames' values become garbage for the next collection.
    pub fn abort(&mut self) {
        let i = state::interp(&self.heap);
        let stack = state::stack(&self.heap);
        let sp = self.heap.arena().get(i, Interp::SP);
        // Popped frames must not keep garbage alive.
        for k in 0..sp.min(self.heap.value_array_capacity(stack)) {
            self.heap.value_array_set(stack, k, Value::UNDEFINED);
        }
        let a = self.heap.arena_mut();
        for f in [
            Interp::BP,
            Interp::HP,
            Interp::SP,
            Interp::PC,
            Interp::HOST_CALL,
            Interp::FLAGS,
        ] {
            a.set(i, f, 0);
        }
        let calls = state::host_calls(&self.heap);
        let n = self.heap.value_array_len(calls);
        for k in 0..n {
            self.heap.value_array_set(calls, k, Value::ZERO);
        }
        self.heap.arena_mut().set(calls, ValueArray::LEN, 0);
        run::promise::clear_jobs(&mut self.heap);
        run::promise::clear_inbox(&mut self.heap);
        run::promise::clear_rejections(&mut self.heap);
        state::set_pending_throw(&mut self.heap, Value::UNDEFINED);
        state::set_fuel_debt(&mut self.heap, 0);
        state::set_outcome(&mut self.heap, RunState::Idle, Value::UNDEFINED);
    }

    /// Runs with `fuel` (instruction cost units).
    pub fn run(&mut self, fuel: u64) -> Status {
        let st = Exec::new(&mut self.heap, self.opts.exec).run(fuel);
        self.after_run(st)
    }

    /// A run that ran out of host memory abandons the call (D1690).
    fn after_run(&mut self, st: Status) -> Status {
        // Debug builds with the barrier armed: the hot loop writes through a raw slice.
        self.heap.check_prefix();
        if st == Status::HostOutOfMemory {
            self.abort();
        }
        st
    }

    /// [`Vm::run`], also returning the fuel used. It can exceed `fuel` by the cost of the
    /// instruction that ran out (for debuggers and fuel accounting, 0064).
    pub fn run_metered(&mut self, fuel: u64) -> (Status, u64) {
        let mut ex = Exec::new(&mut self.heap, self.opts.exec);
        let st = ex.run(fuel);
        let used = ex.fuel_used();
        (self.after_run(st), used)
    }

    /// The stack trace of the current call while it is suspended (out of fuel or parked
    /// on a host call), in the format of `Error.prototype.stack`; empty without frames.
    /// For debuggers (0064).
    pub fn backtrace(&mut self) -> String {
        let ex = Exec::new(&mut self.heap, self.opts.exec);
        // Not the program's Error.stackTraceLimit: a debugger's view.
        ex.stack_trace(ex.pc(), run::STACK_TRACE_LIMIT)
    }

    /// The pending host call `call_id` (parked or async).
    pub fn host_call(&self, call_id: u32) -> Option<HostCallInfo> {
        let c = run::host_call_cell(&self.heap, call_id)?;
        let a = self.heap.arena();
        let args: Ref = a.get(c, HostCall::ARGS);
        let n = a.get(args.0, ValueArray::LEN);
        Some(HostCallInfo {
            fn_id: a.get(c, HostCall::FN),
            this: a.get(c, HostCall::THIS),
            args: (0..n)
                .map(|i| self.heap.value_array_get(args.0, i))
                .collect(),
            is_async: a.get::<Value>(c, HostCall::PROMISE).as_object().is_some(),
        })
    }

    /// Continues an open call of a plain dump (D2890): while parked on the sync host call
    /// `parked`, a `resume(id, name, args, state)` hook the dump calls, opens async host
    /// call `id` of host function `fn_id` (the host's for `name`) with the elements of
    /// `args`, and returns its promise from the parked call. The call keeps its id, so a
    /// later dump writes it as it was; the next new call gets a higher one. It is not
    /// reported by [`Vm::new_host_calls`]: the host has it already. `Err`: the reason, for
    /// the host to throw at the parked call (only out of memory comes after the parked
    /// call returned).
    pub fn open_host_call(&mut self, parked: u32, fn_id: u32) -> Result<(), &'static str> {
        // Plain code over closures and iterator adapters: this is in the shipped module,
        // under the size budget (D020).
        let i = state::interp(&self.heap);
        if self.state() != RunState::HostCall
            || self.heap.arena().get(i, Interp::HOST_CALL) != parked
        {
            return Err("not parked on that call");
        }
        let c = run::host_call_cell(&self.heap, parked).ok_or("not parked on that call")?;
        let args: Ref = self.heap.arena().get(c, HostCall::ARGS);
        let argc = self.heap.arena().get(args.0, ValueArray::LEN);
        let arg = |n: u32| {
            if n < argc {
                self.heap.value_array_get(args.0, n)
            } else {
                Value::UNDEFINED
            }
        };
        let (id, list) = (arg(0).as_int(), arg(2).as_object());
        let Some(id) = id.filter(|&n| n > 0).map(|n| n as u32) else {
            return Err("resume: not a call id");
        };
        let Some(a) =
            list.filter(|&o| self.heap.object_class(o) == crate::heap::ObjectClass::Array)
        else {
            return Err("resume: the arguments are not an array");
        };
        // (Sync calls take ids from the same counter: `parked` itself may have `id`.)
        if id != parked && run::host_call_cell(&self.heap, id).is_some() {
            return Err("resume: the call is open already");
        }
        const OOM: &str = "out of memory";
        let n = self.heap.array_length(a);
        let arr = self.heap.alloc_value_array(n).map_err(|_| OOM)?;
        for k in 0..n {
            let v = self
                .heap
                .get_element_fast(a, k)
                .ok_or("resume: the arguments are an array with holes")?;
            self.heap.value_array_set(arr, k, v);
        }
        self.heap.arena_mut().set(arr, ValueArray::LEN, n);
        let proto = intrinsic_obj(&self.heap, Intrinsic::PromiseProto);
        let p = Value::object(run::promise::alloc(&mut self.heap, Some(proto)).map_err(|_| OOM)?);
        // The parked call returns first, so that its id is free again. Out of memory
        // after it leaves the promise pending forever: the host gives up the load.
        self.resolve_host_call(parked, Ok(p)).map_err(|_| OOM)?;
        run::add_loaded_host_call(&mut self.heap, id, fn_id, arr, p).map_err(|_| OOM)?;
        let c = run::host_call_cell(&self.heap, id).ok_or(OOM)?;
        let flags = self.heap.arena().get(c, HostCall::FLAGS);
        self.heap
            .arena_mut()
            .set(c, HostCall::FLAGS, flags | HOST_CALL_REPORTED);
        Ok(())
    }

    /// Sets the id the next host call gets (a plain dump's `$djs.seed(lo, hi, next)`,
    /// D2897): the host restores the dumped VM's counter once the dump has loaded, since
    /// the hooks, sync calls, took ids of their own. 0 is never an id.
    pub fn set_next_call_id(&mut self, id: u32) {
        let i = state::interp(&self.heap);
        self.heap
            .arena_mut()
            .set(i, Interp::NEXT_CALL_ID, id.max(1));
    }

    /// Completes the host call `call_id` with a value (`Ok`) or an exception (`Err`).
    /// The call the VM is parked on returns or throws at the call; `run` continues
    /// afterwards. An async call (D1374) goes into the host inbox (D1670), in any VM
    /// state: `run` applies it as a macrotask of its own once the job queue is empty
    /// (oldest result first, the job queue drained after each), so the script sees the
    /// same thing whenever the embedder delivers it. Its value is adopted like a resolve
    /// function's argument (thenables too); an exception rejects.
    pub fn resolve_host_call(
        &mut self,
        call_id: u32,
        result: Result<Value, Value>,
    ) -> Result<(), VmError> {
        self.complete_host_call(call_id, result, None)
    }

    /// [`Vm::resolve_host_call`] with the clock `Date.now()` reads from when the result is
    /// applied (ms since the epoch, as [`Vm::set_now`]): for an async call, when `run`
    /// takes it from the inbox; for the parked call, now. Lets a host give every result
    /// its own virtual time (a replay) without running the VM between results.
    pub fn resolve_host_call_at(
        &mut self,
        call_id: u32,
        result: Result<Value, Value>,
        now_ms: f64,
    ) -> Result<(), VmError> {
        self.complete_host_call(call_id, result, Some(now_ms))
    }

    fn complete_host_call(
        &mut self,
        call_id: u32,
        result: Result<Value, Value>,
        now: Option<f64>,
    ) -> Result<(), VmError> {
        if let Some(c) = run::host_call_cell(&self.heap, call_id) {
            let p: Value = self.heap.arena().get(c, HostCall::PROMISE);
            if let Some(p) = p.as_object() {
                run::promise::inbox_push(&mut self.heap, p, result, now)?;
                run::remove_host_call(&mut self.heap, call_id);
                return Ok(());
            }
        }
        let i = state::interp(&self.heap);
        if self.state() != RunState::HostCall
            || self.heap.arena().get(i, Interp::HOST_CALL) != call_id
        {
            return Err(VmError::UnknownHostCall);
        }
        if let Some(ms) = now {
            self.set_now(ms);
        }
        let (cont, entry) = run::parked(&self.heap);
        run::remove_host_call(&mut self.heap, call_id);
        self.heap.arena_mut().set(i, Interp::HOST_CALL, 0);
        if entry {
            let (s, v) = match result {
                Ok(v) => (RunState::Done, v),
                Err(e) => (RunState::Threw, e),
            };
            state::set_outcome(&mut self.heap, s, v);
            return Ok(());
        }
        match result {
            Ok(v) => {
                let mut ex = Exec::new(&mut self.heap, self.opts.exec);
                match cont {
                    Continuation::PushResult => ex.push(v),
                    Continuation::Replace(slot) | Continuation::Retry(slot) => ex.set_slot(slot, v),
                    _ => {}
                }
                ex.flush();
                state::set_state(&mut self.heap, RunState::Running);
            }
            Err(e) => {
                state::set_pending_throw(&mut self.heap, e);
                state::set_state(&mut self.heap, RunState::ThrowPending);
            }
        }
        Ok(())
    }

    /// How the entry call ended, while no frame is active: its return value (`Ok`) or
    /// exception (`Err`). `run` hands it out with `Done`/`Threw`, but also returns `Idle`
    /// after the call returned while async host calls are pending; the next `call`
    /// replaces it (0174: a REPL input's promise).
    pub fn outcome(&self) -> Option<Result<Value, Value>> {
        let v = state::result(&self.heap);
        match self.state() {
            RunState::Done => Some(Ok(v)),
            RunState::Threw => Some(Err(v)),
            _ => None,
        }
    }

    /// The state of a Promise: `None` if `v` is not a promise, `Some(None)` while it is
    /// pending, else its result (`Ok` fulfilled, `Err` rejected). For hosts whose entry
    /// function returns a promise (an async body).
    #[allow(clippy::option_option)]
    pub fn promise_state(&self, v: Value) -> Option<Option<Result<Value, Value>>> {
        use run::promise;
        let p = promise::as_promise(&self.heap, v)?;
        let r = promise::result_of(&self.heap, p);
        Some(match promise::state_of(&self.heap, p) {
            promise::FULFILLED => Some(Ok(r)),
            promise::REJECTED => Some(Err(r)),
            _ => None,
        })
    }

    /// Marks a promise handled (as a `then` would, D1423): its rejection is not reported
    /// by `take_unhandled_rejections`. For an embedder that consumes the outcome itself,
    /// such as an async body's result promise. No-op for other values.
    pub fn mark_promise_handled(&mut self, v: Value) {
        if let Some(p) = run::promise::as_promise(&self.heap, v) {
            run::promise::mark_handled(&mut self.heap, p);
        }
    }

    /// The reasons of the promises that were rejected without a handler and still have
    /// none (HostPromiseRejectionTracker, D1373), oldest first; they are forgotten. Check
    /// after `run` returns `Done`, `Threw` or `Idle` (the job queue is empty then), the way
    /// qjs reports `Possibly unhandled promise rejection`.
    pub fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        run::promise::take_rejections(&mut self.heap)
            .into_iter()
            .map(|p| run::promise::result_of(&self.heap, p))
            .collect()
    }

    /// Formats a value the way `console.log` shows it in `djs run`: strings raw, Errors
    /// as `Name: message`, everything else as JSON with a `String()`-like fallback.
    pub fn display(&mut self, v: Value) -> String {
        format::display(&mut self.heap, v)
    }

    /// The `stack` of an Error object, if it has one: an own data property `stack` (a
    /// script assigned one) or else the trace it recorded (D1971). Never runs JS.
    pub fn error_stack(&self, v: Value) -> Option<String> {
        let o = v.as_object()?;
        let s = match self.heap.get_own_property(o, atoms::STACK) {
            Some(p) => p.value()?.as_string()?,
            None => crate::interp::realm::error_stack_slot(&self.heap, v).as_string()?,
        };
        Some(self.heap.str_to_string(s))
    }

    /// Collects garbage now (a full collection). Raw values held outside the heap are
    /// invalid afterwards.
    pub fn collect(&mut self) -> Result<(), HeapError> {
        self.collect_as(Collection::Full)
    }

    /// [`Vm::collect`], then everything live is old (`Heap::seal`, D2041): the
    /// collections while scripts run leave it alone. For the end of the embedder's setup
    /// (pi calls it after its prelude).
    pub fn seal(&mut self) -> Result<(), HeapError> {
        // A sealed image (pi's prelude) runs many times: compile its lazy functions now
        // rather than in every run (D2990).
        #[cfg(feature = "lazy")]
        self.compile_lazy_templates()?;
        let g = self.global();
        self.heap.move_to_end(g)?;
        self.collect_as(Collection::Seal)
    }

    /// [`Vm::collect`] by copying (Cheney, `Heap::collect_canonical`): the heap comes out
    /// as a function of its object graph alone. For images that should not depend on how
    /// they were built (the base image, 0101).
    ///
    /// The builtin prefix stays as it is (D2064).
    pub fn collect_canonical(&mut self) -> Result<(), HeapError> {
        self.collect_as(Collection::Canonical)
    }

    fn collect_as(&mut self, how: Collection) -> Result<(), HeapError> {
        Self::collect_heap(&mut self.heap, how)?;
        if self.opts.exec.finalization_after_gc {
            self.schedule_finalization_cleanup()?;
        }
        Ok(())
    }

    /// Makes this (idle) VM's heap the base image, as djs-prelude's build does: tables
    /// shrunk to their use (0043), the prelude's internal names dropped (D2253), collected
    /// by copying so that the image depends on its graph alone (0101), then its cells
    /// ordered by kind for the module's gzip (D2251).
    pub fn prepare_base_image(&mut self) -> Result<(), HeapError> {
        // Builtins' `length` and `name` from their templates (D2445), in the prelude's
        // shape tree; the constant pool without the dead installing code's (D2446).
        image::lazy_builtin_props(&mut self.heap)?;
        self.collect()?;
        image::prune_pools(&mut self.heap)?;
        // A VM's own shape tree (D2063), with the shapes of the objects builtins create
        // already in it (D2067): the builtins' shapes keep the prelude's root.
        self.heap
            .arena_mut()
            .set(0, crate::heap::cells::Roots::ROOT_SHAPE, Ref(0));
        for keys in image::BUILTIN_RESULT_SHAPES {
            self.heap.precreate_shapes(keys)?;
        }
        // (Interned before the tables are shrunk: `drop_internal_names` keeps them.)
        for name in image::ENGINE_ATOMS {
            self.heap.atom_str(name)?;
        }
        crate::interp::run::promise::preallocate_queues(&mut self.heap)?;
        self.heap.compact_for_image()?;
        // Without the prelude's internal names (D2253).
        image::drop_internal_names(&mut self.heap)?;
        // The base's atoms go to the prefix, a VM's own to the tail (D2431).
        self.heap.split_atom_table(image::SESSION_ATOMS)?;
        self.collect_canonical()?;
        let tail = self.prepare_prefix()?;
        let tail_bytes: u32 = tail.iter().map(|&c| self.heap.cell_size(c)).sum();
        self.heap.order_for_image(&tail)?;
        // The image is the builtin prefix of every VM made from it (D022), then the
        // per-VM cells: `BASE_END` is where they start.
        let len = self.heap.arena().len();
        self.heap
            .arena_mut()
            .set(0, crate::heap::cells::Roots::BASE_END, len - tail_bytes);
        // The prefix's strings cache the fixed hash, not the building VM's keyed one (D2433).
        self.heap.prefix_hashes_to_base();
        // The stored threshold by the prefix rule (D2442), now that there is a prefix:
        // a VM's first collection comes after as much growth as later ones.
        self.heap.reset_gc_threshold();
        Ok(())
    }

    /// Readies a collected realm for the builtin prefix (D2063): what a VM would write
    /// lazily into base cells is written now, and the cells a VM writes are listed for
    /// the image's tail (above `BASE_END`), in their order there.
    /// - Every builtin object is marked as a prototype (D1621), so that is not written
    ///   later. (String hashes are not cached in the prefix: `Heap::str_hash`.)
    /// - The root shape is the VM's own (`prepare_base_image` replaced it), and its tree
    ///   goes to the tail: scripts extend it.
    /// - The tail: the realm and its random state, the global object, the global lexical
    ///   record and `Symbol.for`'s registry with their property tables, Map/Set tables,
    ///   the atom table, the host call table, the job queue (allocated now, D2067), the
    ///   interpreter cell, the value stack and the VM's shape tree.
    fn prepare_prefix(&mut self) -> Result<Vec<u32>, HeapError> {
        use crate::heap::cells::{Object, Realm, Roots};
        let cells: Vec<(u32, crate::heap::CellKind)> =
            self.heap.cells().map(|c| (c.offset, c.kind)).collect();
        // The objects the tail takes (below) are a VM's own, not prototypes.
        let realm = realm::realm(&self.heap);
        let lex: Ref = self.heap.arena().get(realm, Realm::GLOBAL_LEX);
        let registry = realm::intrinsic(&self.heap, Intrinsic::SymbolRegistry)
            .as_object()
            .map_or(0, |o| o.0);
        let own = [self.global().0, lex.0, registry];
        for &(c, kind) in &cells {
            match kind {
                // Any builtin can end up on a prototype chain (a class extending one, an
                // object created from one): all are marked now rather than when an
                // inline cache first caches through them.
                crate::heap::CellKind::Object if !own.contains(&c) => {
                    self.heap.mark_proto(Obj(c));
                }
                _ => {}
            }
        }
        let h = &self.heap;
        let a = h.arena();
        let mut tail: Vec<u32> = h
            .first_grown(self.global())
            .into_iter()
            .map(|e| h.first_grown_target(e))
            .filter(|&t| t != 0)
            .collect();
        let internal = |v: Value| v.as_internal().unwrap_or(0);
        let global = self.global().0;
        let mut front = alloc::vec![
            realm,
            internal(realm::intrinsic(h, Intrinsic::RandomState)),
            global,
            lex.0,
            registry,
        ];
        for o in [lex.0, registry] {
            if o != 0 {
                let p: Ref = a.get(o, Object::PROPS);
                front.push(p.0);
            }
        }
        // The atom table's cell goes with its entries and index (last in `tail`).
        let atoms: Ref = a.get(0, Roots::ATOMS);
        let interp: Ref = a.get(0, Roots::INTERP);
        let stack: Ref = a.get(0, Roots::STACK);
        let jobs: Ref = a.get(0, Roots::JOBS);
        let queues = [
            jobs.0,
            a.get::<Ref>(interp.0, crate::heap::cells::Interp::INBOX).0,
            a.get::<Ref>(interp.0, crate::heap::cells::Interp::REJECTIONS)
                .0,
        ];
        let calls: Ref = a.get(0, Roots::HOST_CALLS);
        let at = tail.len().saturating_sub(2);
        tail.insert(at, atoms.0);
        front.append(&mut tail);
        front.extend(h.root_shape_tree());
        front.push(calls.0);
        front.extend_from_slice(&queues);
        front.extend_from_slice(&[interp.0, stack.0]);
        let mut seen = alloc::collections::BTreeSet::new();
        front.retain(|&c| c != 0 && seen.insert(c));
        Ok(front)
    }

    /// A collection of `heap` alone (no finalization scheduling).
    fn collect_heap(heap: &mut Heap, how: Collection) -> Result<(), HeapError> {
        // Popped values above `sp` must not keep garbage alive (as at the interpreter's
        // safepoints): the stack is traced to its full capacity.
        // Not shrunk for a seal: a new stack would land after the cells it moved to the
        // end (D2057).
        tidy_stack(heap, !matches!(how, Collection::Seal));
        match how {
            Collection::Full => heap.collect(),
            Collection::Seal => heap.seal(),
            Collection::Canonical => heap.collect_canonical(),
        }
        .map(|_| ())
    }

    /// A hash of the VM's state that does not depend on when it last collected (D1730):
    /// FNV-1a (64 bits) over the image a collection would leave. Computed on a copy, so
    /// the VM itself is unchanged and a check never perturbs the run it checks. Two VMs
    /// with the same graph (the same script, inputs and engine build) have the same hash,
    /// whatever their fuel slices, snapshots or collection points were (D1090, D1601).
    /// Needs memory for the copy and its collection (about three times the arena).
    pub fn canonical_hash(&self) -> Result<u64, HeapError> {
        let limit = (self.heap.bytes().len()).max(self.opts.memory_limit);
        let mut copy = Heap::from_bytes_trusted(self.heap.bytes().to_vec(), limit)
            .map_err(|_| HeapError::OutOfMemory)?;
        Self::collect_heap(&mut copy, Collection::Canonical)?;
        Ok(fnv64(copy.bytes()))
    }

    /// Queues a cleanup job for every FinalizationRegistry with cells whose targets
    /// died in a collection (HostEnqueueFinalizationRegistryCleanupJob); the number of
    /// jobs. They run at the next `run`. Collections do this themselves unless
    /// `ExecOptions::finalization_after_gc` is off (D1513).
    pub fn schedule_finalization_cleanup(&mut self) -> Result<usize, HeapError> {
        crate::interp::run::late::schedule_finalization_cleanup(&mut self.heap)
    }
}

/// The value stack in an image (D2308): zeroes the slots above the stack pointer, as a
/// fresh stack has them (a zero word is the Number +0, which holds no reference). With
/// `shrink` (before a full collection the embedder asks for), a stack that grew for
/// a deep recursion, whose capacity every later image would carry (32K slots, 256 KB,
/// after 2,000 nested calls), also moves its live slots to a stack of `max(256, 2 * sp)`
/// slots when that is at most half of it; the old one is garbage. Only between runs (no interpreter holds the stack's offset), so it costs a run
/// nothing, and the interpreter grows it again on demand.
#[inline]
fn tidy_stack(heap: &mut Heap, shrink: bool) {
    let sp = heap.arena().get(state::interp(heap), Interp::SP);
    let cap = heap.value_array_capacity(state::stack(heap));
    let keep = crate::heap::IMAGE_STACK.max(2 * sp);
    let cap = if shrink && cap >= 2 * keep { keep } else { cap };
    // A failed allocation keeps the old stack, its dead slots zeroed.
    let _ = state::move_stack(heap, sp, cap);
}

/// The `hash` of a script's `$djs.source(name, hash)` (D2882): FNV-1a of its source text,
/// its parameter names and its compile options, folded to 32 bits, 8 hex digits (D2839:
/// enough to tell a host's scripts apart; the name stays). The host's script must have
/// the same, or `Vm::source_function` refuses it.
pub fn script_hash(source: &str, params: &str, bits: u32) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let bits = cat!(bits);
    for b in params
        .bytes()
        .chain([0])
        .chain(bits.bytes())
        .chain([0])
        .chain(source.bytes())
    {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let h = (h ^ (h >> 32)) as u32;
    let mut s = String::new();
    for i in (0..8).rev() {
        let d = (h >> (i * 4)) & 0xf;
        s.push(char::from_digit(d, 16).unwrap_or('0'));
    }
    s
}

/// FNV-1a, 64 bits.
fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod compat_tests;
#[cfg(all(test, feature = "dump"))]
mod dump_debugger_tests;
#[cfg(all(test, feature = "dump"))]
mod dump_harvest;
#[cfg(all(test, feature = "dump"))]
mod dump_perf;
#[cfg(all(test, feature = "dump"))]
mod dump_resume_tests;
#[cfg(all(test, feature = "dump"))]
mod dump_tests;
#[cfg(all(test, feature = "embed"))]
mod encoding_tests;
#[cfg(test)]
mod frozen_tests;
#[cfg(all(test, feature = "lazy"))]
mod lazy_tests;
#[cfg(all(test, feature = "embed"))]
mod module_tests;
#[cfg(test)]
mod slim_tests;
#[cfg(test)]
mod tests;

impl Vm {
    /// A plain dump's `$djs.source(name, hash)` (D2882): the host's script `name`, `src`
    /// compiled with the options it ran with, as the codemode body function the dump's
    /// continuations name as their origin. Refused (for the host to throw) when it is not
    /// the script the dump refers to: another text, parameters or options.
    pub fn source_function(
        &mut self,
        src: &[u8],
        opts: &crate::compiler::CompileOptions<'_>,
        hash: &str,
    ) -> Result<Value, String> {
        let f = self
            .compile(src, opts)
            .map_err(|e| cat!("source: the script does not compile: ", e.message))?;
        let h = &self.heap;
        let t = f.as_object().map(|o| h.closure_template(o));
        let script = t.and_then(|t| crate::bytecode::template::debug_info(h, t).script.get());
        let Some(script) = script else {
            return Err(String::from("source: not a script"));
        };
        let a = h.arena();
        let text = |r: Ref| {
            r.get()
                .map(|s| h.str_to_string(crate::value::Str(s)))
                .unwrap_or_default()
        };
        let source = crate::bytecode::template::script_text(h, script).unwrap_or_default();
        let params = text(a.get::<Ref>(script, crate::heap::cells::Script::PARAMS));
        let bits = a.get::<u32>(script, crate::heap::cells::Script::OPTIONS);
        if script_hash(&source, &params, bits) != hash {
            return Err(String::from(
                "source: the host's script is not the one the dump refers to (another text, parameters or options)",
            ));
        }
        Ok(f)
    }
}
