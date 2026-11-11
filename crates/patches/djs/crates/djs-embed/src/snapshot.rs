//! [`Snapshot`]: a VM image after the host's setup, compiled once and shared.

use std::sync::Arc;

use djs_core::compiler::CompileOptions;
use djs_core::heap::Heap;
use djs_core::value::Value as Raw;
use djs_core::vm::{ExecOptions, IMPORT_FN_ID, Status, Vm, VmOptions};

use crate::instance::{answer_imports, exception, guard_vm};
use crate::module::{Adapter, Export, SharedLoader};
use crate::value::{to_raw, wire_error};
use crate::{Data, Error, Instance, Limits, Loader, ModuleSource, SyntheticModule, Value};

/// JavaScript source with its name (for stack traces and syntax errors).
#[derive(Clone, Debug)]
pub struct Source {
    name: String,
    text: String,
    kind: SourceKind,
}

/// How a [`Source`] is compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    /// A classic script: its top-level declarations become globals
    Script,
    /// An ES module: its name is its key, its imports go through the snapshot's
    /// [`Loader`] and synthetic modules, and its exports are read with
    /// [`crate::Instance::export`] and called with [`crate::Instance::start_export`]
    Module,
    /// A module in TypeScript
    TypeScriptModule,
}

impl Source {
    /// A classic script.
    pub fn script(name: impl Into<String>, text: impl Into<String>) -> Source {
        Source {
            name: name.into(),
            text: text.into(),
            kind: SourceKind::Script,
        }
    }

    /// An ES module with key `name` (a policy module that exports `onRequest`). Setup loads
    /// it and the modules it imports, links and evaluates them; the snapshot holds the
    /// evaluated graph. The loader is not asked for `name` itself.
    pub fn module(name: impl Into<String>, text: impl Into<String>) -> Source {
        Source {
            name: name.into(),
            text: text.into(),
            kind: SourceKind::Module,
        }
    }

    /// [`Source::module`] in TypeScript.
    pub fn typescript_module(name: impl Into<String>, text: impl Into<String>) -> Source {
        Source {
            name: name.into(),
            text: text.into(),
            kind: SourceKind::TypeScriptModule,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }
}

/// A host function: a global the scripts call, which the host answers by `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostFn {
    id: u32,
    name: String,
    is_async: bool,
}

impl HostFn {
    /// A synchronous host function: a call stops the VM (`Step::HostCall`) until the host
    /// answers; the script gets the value or the exception at the call. For bounded work
    /// (parsing, logging).
    pub fn sync(id: u32, name: impl Into<String>) -> HostFn {
        HostFn {
            id,
            name: name.into(),
            is_async: false,
        }
    }

    /// An async host function: a call returns a promise at once and the script runs on;
    /// the host starts the work when the VM waits (`Step::Wait`) and its answer settles
    /// the promise. For I/O.
    pub fn asynchronous(id: u32, name: impl Into<String>) -> HostFn {
        HostFn {
            id,
            name: name.into(),
            is_async: true,
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_async(&self) -> bool {
        self.is_async
    }
}

/// Builds a [`Snapshot`]: host functions and data globals are defined first, then the
/// sources run in order, each to completion.
#[derive(Clone, Debug)]
pub struct Builder {
    host_fns: Vec<HostFn>,
    globals: Vec<(String, Data)>,
    modules: Vec<SyntheticModule>,
    loader: Option<SharedLoader>,
    sources: Vec<Source>,
    frozen_builtins: bool,
    memory: usize,
    fuel: u64,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            host_fns: Vec::new(),
            globals: Vec::new(),
            modules: Vec::new(),
            loader: None,
            sources: Vec::new(),
            frozen_builtins: true,
            memory: 64 << 20,
            fuel: 1_000_000_000,
        }
    }
}

impl Builder {
    /// Defines a host function as a global (`name`, not enumerable).
    pub fn host_fn(mut self, f: HostFn) -> Builder {
        self.host_fns.push(f);
        self
    }

    /// Defines a global holding plain data (configuration for the scripts).
    pub fn global(mut self, name: impl Into<String>, value: Data) -> Builder {
        self.globals.push((name.into(), value));
        self
    }

    /// Defines a synthetic module (its exports host functions and data), which modules
    /// import by its key without the loader.
    pub fn module(mut self, m: SyntheticModule) -> Builder {
        self.modules.push(m);
        self
    }

    /// The loader of the modules that setup modules import, and of every `import()` in
    /// setup and in calls (answered inside [`crate::Instance::step`]). Without one, only
    /// synthetic modules and setup modules can be imported.
    pub fn loader(mut self, loader: impl Loader + 'static) -> Builder {
        self.loader = Some(SharedLoader(Arc::new(loader)));
        self
    }

    /// Runs `source` at setup, after the sources added before it.
    pub fn source(mut self, source: Source) -> Builder {
        self.sources.push(source);
        self
    }

    /// Frozen builtins (default on): the builtin objects behave as frozen, so scripts
    /// cannot patch `Array.prototype` and the like; assigning an inherited builtin
    /// property defines an own property instead (as with Node's
    /// `--frozen-intrinsics`). The global object stays writable.
    pub fn frozen_builtins(mut self, on: bool) -> Builder {
        self.frozen_builtins = on;
        self
    }

    /// The heap cap during setup in `bytes` (default 64 MiB).
    pub fn memory(mut self, bytes: usize) -> Builder {
        self.memory = bytes;
        self
    }

    /// The fuel budget of all sources together (default 10^9, about 4 s).
    pub fn fuel(mut self, fuel: u64) -> Builder {
        self.fuel = fuel;
        self
    }

    /// Runs the setup and takes the snapshot. Setup sources must not call host functions:
    /// the snapshot has no host to answer them ([`Error::Usage`]).
    pub fn build(self) -> Result<Snapshot, Error> {
        let opts = vm_options(self.memory, self.frozen_builtins);
        let mut vm = guard_vm(|| djs_prelude::new_vm(opts))?.map_err(|_| Error::OutOfMemory)?;
        let image = guard_vm(|| self.setup(&mut vm))??;
        Ok(Snapshot(Arc::new(Inner {
            image,
            fingerprint: Default::default(),
            frozen_builtins: self.frozen_builtins,
            host_fns: self.host_fns,
            modules: self.modules,
            loader: self.loader,
        })))
    }

    fn setup(&self, vm: &mut Vm) -> Result<Vec<u8>, Error> {
        let oom = |_| Error::OutOfMemory;
        let reserved = self
            .host_fns
            .iter()
            .chain(self.modules.iter().flat_map(|m| {
                m.exports.iter().filter_map(|e| match e {
                    Export::Fn(f) => Some(f),
                    Export::Data(..) => None,
                })
            }))
            .find(|f| f.id == IMPORT_FN_ID);
        if let Some(f) = reserved {
            return Err(Error::Usage(format!(
                "host function {}: id {IMPORT_FN_ID} is reserved for import()",
                f.name
            )));
        }
        for f in &self.host_fns {
            let v = if f.is_async {
                vm.new_async_host_function(f.id, &f.name)
            } else {
                vm.new_host_function(f.id, &f.name)
            }
            .map_err(oom)?;
            vm.define_global(&f.name, v).map_err(oom)?;
        }
        for (name, data) in &self.globals {
            let v = vm.decode_wire(&data.to_wire()).map_err(wire_error)?;
            vm.define_global(name, v).map_err(oom)?;
        }
        for m in &self.modules {
            let mut values = Vec::with_capacity(m.exports.len());
            for e in &m.exports {
                values.push(match e {
                    Export::Fn(f) if f.is_async => {
                        vm.new_async_host_function(f.id, &f.name).map_err(oom)?
                    }
                    Export::Fn(f) => vm.new_host_function(f.id, &f.name).map_err(oom)?,
                    Export::Data(_, d) => to_raw(vm, &Value::Data(d))?,
                });
            }
            let exports: Vec<(&str, Raw)> = m
                .exports
                .iter()
                .zip(values)
                .map(|(e, v)| match e {
                    Export::Fn(f) => (f.name.as_str(), v),
                    Export::Data(n, _) => (n.as_str(), v),
                })
                .collect();
            if let Err(e) = vm.define_synthetic_module(&m.key, &exports) {
                return Err(Error::Usage(format!("module {}: {}", m.key, vm.display(e))));
            }
        }
        vm.set_dynamic_import(true)
            .map_err(|_| Error::OutOfMemory)?;
        let mut fuel = self.fuel;
        for s in &self.sources {
            if s.kind == SourceKind::Script {
                let f = vm
                    .compile(s.text.as_bytes(), &CompileOptions::script(&s.name))
                    .map_err(|e| Error::Syntax {
                        source: s.name.clone(),
                        message: e.to_string(),
                    })?;
                let this = Raw::object(vm.global());
                vm.call(f, this, &[])
                    .map_err(|e| Error::Usage(format!("{e:?}")))?;
            } else {
                let source = if s.kind == SourceKind::TypeScriptModule {
                    ModuleSource::typescript(s.text.clone())
                } else {
                    ModuleSource::js(s.text.clone())
                };
                let mut adapter = self.adapter(Some((&s.name, &source)));
                let key = vm
                    .load_module(&s.name, &mut adapter)
                    .map_err(|e| load_error(vm, &s.name, e))?;
                vm.evaluate_module(&key)
                    .map_err(|e| Error::Usage(format!("{e:?}")))?;
            }
            loop {
                let (st, used) = vm.run_metered(fuel);
                fuel = fuel.saturating_sub(used);
                match st {
                    Status::Done(v) => {
                        // A module's evaluation ends with its promise, rejected if it threw,
                        // still pending if its top-level await waits on nothing.
                        match vm.promise_state(v) {
                            Some(Some(Err(e))) => {
                                vm.mark_promise_handled(v);
                                return Err(exception(vm, e));
                            }
                            Some(None) => return Err(Error::NeverSettles),
                            _ => {}
                        }
                        break;
                    }
                    Status::Threw(e) => return Err(exception(vm, e)),
                    Status::OutOfFuel if fuel == 0 => {
                        vm.abort();
                        return Err(Error::OutOfFuel { budget: self.fuel });
                    }
                    Status::OutOfFuel => {}
                    Status::HostOutOfMemory => return Err(Error::OutOfMemory),
                    Status::HostCall { fn_id, .. } => {
                        vm.abort();
                        return Err(Error::Usage(format!(
                            "{} called host function {fn_id} during setup",
                            s.name
                        )));
                    }
                    Status::Idle => {
                        // Only `import()`s can be answered here.
                        let calls = vm.new_host_calls();
                        let none = calls.is_empty();
                        let rest = answer_imports(vm, &mut self.adapter(None), calls)?;
                        if none || !rest.is_empty() {
                            vm.abort();
                            return Err(Error::Usage(format!(
                                "{} left async host calls pending during setup",
                                s.name
                            )));
                        }
                    }
                }
            }
        }
        // What every call writes (registers, stack, queues) goes last in the image:
        // instances note their writes below it only (`Vm::track_image`).
        vm.move_interp_to_end().map_err(oom)?;
        // Everything the setup left is old from now on: collections during calls leave it
        // alone.
        vm.seal().map_err(oom)?;
        Ok(vm.image().to_vec())
    }
}

impl Builder {
    fn adapter<'a>(&'a self, entry: Option<(&'a str, &'a ModuleSource)>) -> Adapter<'a> {
        Adapter {
            loader: self.loader.as_ref().map(|l| &*l.0),
            synthetic: &self.modules,
            entry,
        }
    }
}

/// A module graph that did not load: a syntax or link error (a SyntaxError), or the
/// loader's.
pub(crate) fn load_error(vm: &mut Vm, name: &str, e: Raw) -> Error {
    let message = vm.display(e);
    if message.starts_with("SyntaxError") {
        Error::Syntax {
            source: name.to_owned(),
            message,
        }
    } else {
        Error::Load {
            module: name.to_owned(),
            message,
        }
    }
}

/// The options of every VM of a snapshot: reaching the heap cap is never catchable.
pub(crate) fn vm_options(memory: usize, frozen_builtins: bool) -> VmOptions {
    VmOptions {
        memory_limit: memory,
        frozen_builtins,
        exec: ExecOptions {
            fatal_out_of_memory: true,
            ..ExecOptions::default()
        },
        ..VmOptions::default()
    }
}

/// What a native instance's arena has beyond its image before a call allocates: one arena
/// page (64 KiB). A call that allocates less never moves the arena; more than this
/// measured no better for short calls and cost the allocator more per instance.
pub(crate) const IMAGE_ROOM: usize = 64 << 10;

/// A compiled setup: the image of a VM after the host's host functions, globals and setup
/// sources, from which [`Instance`]s start. Cheap to clone (shared), `Send` and `Sync`.
/// Images live in this process only: there is no way to save one, and none is needed,
/// since building one from the sources is the same thing.
#[derive(Clone)]
pub struct Snapshot(Arc<Inner>);

struct Inner {
    image: Vec<u8>,
    fingerprint: crate::park::Fingerprint,
    frozen_builtins: bool,
    host_fns: Vec<HostFn>,
    modules: Vec<SyntheticModule>,
    loader: Option<SharedLoader>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("image_len", &self.0.image.len())
            .field("host_fns", &self.0.host_fns)
            .finish()
    }
}

const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<Snapshot>();
};

impl Snapshot {
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// A new VM in the snapshot's state. Copies the image (no compile, no setup run).
    pub fn instantiate(&self, limits: Limits) -> Result<Instance, Error> {
        Instance::new(self.clone(), limits)
    }

    /// The image's size in `bytes`: what each instance copies.
    pub fn image_len(&self) -> usize {
        self.0.image.len()
    }

    /// The host functions, as declared.
    pub fn host_fns(&self) -> &[HostFn] {
        &self.0.host_fns
    }

    /// The synthetic modules, as declared.
    pub fn modules(&self) -> &[SyntheticModule] {
        &self.0.modules
    }

    /// The loader of `import()` in calls.
    pub(crate) fn adapter(&self) -> Adapter<'_> {
        Adapter {
            loader: self.0.loader.as_ref().map(|l| &*l.0),
            synthetic: &self.0.modules,
            entry: None,
        }
    }

    pub(crate) fn image(&self) -> &[u8] {
        &self.0.image
    }

    pub(crate) fn frozen_builtins(&self) -> bool {
        self.0.frozen_builtins
    }

    /// Whether `other` is this snapshot (a clone of it): the same image.
    pub(crate) fn same_image(&self, other: &Snapshot) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn fingerprint_cell(&self) -> &crate::park::Fingerprint {
        &self.0.fingerprint
    }

    /// A buffer of the image with room for the heap to start growing in. A native
    /// allocator moves a block that grows (a copy of the whole arena), so there the
    /// buffer has [`IMAGE_ROOM`] more than the image.
    pub(crate) fn image_buffer(&self, memory: usize) -> Vec<u8> {
        let image = self.image();
        let room = if cfg!(target_arch = "wasm32") {
            0
        } else {
            IMAGE_ROOM
        };
        let mut v = Vec::with_capacity(Heap::image_capacity(image.len() + room, memory));
        v.extend_from_slice(image);
        v
    }
}
