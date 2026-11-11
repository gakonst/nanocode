//! ES modules for embedders (D3171): a VM loads a module graph through the host's
//! [`ModuleLoader`], synchronously and with no I/O of its own, links it, evaluates it in
//! a run like any call, and hands out its exports. The records live in the realm
//! (interp/module.rs), so an image of the VM carries the loaded, linked and evaluated
//! graph: a VM made from it reads the same exports, and their bindings stay live.
//!
//! ```ignore
//! let key = vm.load_module("main.js", &mut loader)?; // loads and links the graph
//! vm.evaluate_module(&key)?;                          // then run until Done
//! let on_request = vm.module_export(&key, "onRequest");
//! ```
//!
//! Without the `embed` feature (D3175) every call fails: loading with a SyntaxError "modules
//! are not available in this build".

use alloc::string::String;
use alloc::vec::Vec;

use super::{Vm, VmError};
use crate::compiler::{CompileOptions, compile_module};
use crate::interp::conv::{Fault, FaultKind};
use crate::interp::module::{self, status};
use crate::interp::{ErrorKind, realm};
use crate::value::{Obj, Value};

/// The fn id of the async host call an `import()` makes in a VM with
/// [`Vm::set_dynamic_import`] on: arguments the specifier and the referrer (strings).
/// Hosts answer it with [`Vm::answer_import`]; their own host functions must not use it.
pub const IMPORT_FN_ID: u32 = u32::MAX;

/// What a [`ModuleSource`]'s text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    JavaScript,
    /// Compiled as TypeScript (`CompileOptions::typescript`).
    TypeScript,
    /// JSON text: a JSON module, whose default export is the parsed value. Only an import
    /// with `with { type: "json" }` may load it, and such an import loads nothing else.
    Json,
}

/// A module's source as the host supplies it.
#[derive(Clone, Debug)]
pub struct ModuleSource {
    pub text: String,
    pub kind: ModuleKind,
}

/// The host side of module loading. Specifiers and keys are opaque strings: the engine
/// gives them no URL or path meaning. A key names one module: the VM loads each key once
/// and keeps its record.
pub trait ModuleLoader {
    /// The key of the module that `specifier` names when module `referrer` (its key)
    /// imports it; `None` for the graph's entry point. `attributes` are the import's
    /// attributes as `(key, value)`: djs supports `type` alone (an import with any other
    /// key fails before this is asked), so they are empty or `[("type", value)]`. An
    /// error becomes an `Error` with the message.
    fn resolve(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        attributes: &[(&str, &str)],
    ) -> Result<String, String>;
    /// The source of module `key`. An error becomes an `Error` with the message.
    fn load(&mut self, key: &str) -> Result<ModuleSource, String>;
}

impl Vm {
    /// Loads the module `specifier` names and every module it imports, directly or not,
    /// through `loader`, compiles and links them: its key. A module already loaded (by
    /// key) is not loaded again. Fails with the JS error: a SyntaxError of a source, a
    /// link error (a missing or ambiguous import, as a SyntaxError), or the loader's
    /// message (an `Error`); the modules this call loaded are forgotten then, so a later
    /// call loads them again. Runs no JS: allowed whenever the VM is not running.
    pub fn load_module(
        &mut self,
        specifier: &str,
        loader: &mut dyn ModuleLoader,
    ) -> Result<String, Value> {
        self.load_module_from(specifier, None, loader)
    }

    /// [`Vm::load_module`] of `specifier` as module (or script) `referrer` imports it: the
    /// loader resolves the specifier against it (an `import()`).
    pub fn load_module_from(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        loader: &mut dyn ModuleLoader,
    ) -> Result<String, Value> {
        self.load_request(specifier, referrer, None, loader)
    }

    /// [`Vm::load_module_from`] of a request with module type `ty` (the `type` import
    /// attribute).
    fn load_request(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        ty: Option<&str>,
        loader: &mut dyn ModuleLoader,
    ) -> Result<String, Value> {
        if !crate::MODULES {
            return Err(self.error_value(
                ErrorKind::SyntaxError,
                "modules are not available in this build",
            ));
        }
        let mut created: Vec<String> = Vec::new();
        let r = self.load_graph(specifier, referrer, ty, loader, &mut created);
        match r {
            Ok(key) => Ok(key),
            Err(f) => {
                for k in &created {
                    let _ = module::remove(&mut self.heap, k);
                }
                Err(self.fault_to_value(f))
            }
        }
    }

    fn load_graph(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        ty: Option<&str>,
        loader: &mut dyn ModuleLoader,
        created: &mut Vec<String>,
    ) -> Result<String, Fault> {
        let json = check_type(ty)?;
        let attrs: &[(&str, &str)] = match ty {
            Some(t) => &[("type", t)],
            None => &[],
        };
        let key = loader
            .resolve(specifier, referrer, attrs)
            .map_err(host_error)?;
        let mut queue = Vec::new();
        let root = self.module_record(&key, json, loader, created, &mut queue)?;
        // The new records' requests (each key is loaded once).
        while let Some(m) = queue.pop() {
            let meta = module::meta(&self.heap, m);
            let referrer = module::key_of(&self.heap, m);
            let mut records = Vec::new();
            for req in &meta.requests {
                // AllImportAttributesSupported, at load time (InnerModuleLoading).
                if !req.unsupported.is_null() {
                    let k = self.heap.atom_name(req.unsupported);
                    return Err(Fault::error(
                        ErrorKind::SyntaxError,
                        crate::text::cat!("unsupported import attribute '", &k, "'"),
                    ));
                }
                let ty = (!req.ty.is_null()).then(|| self.heap.atom_name(req.ty));
                let json = check_type(ty.as_deref())?;
                let attrs: &[(&str, &str)] = match &ty {
                    Some(t) => &[("type", t)],
                    None => &[],
                };
                let s = self.heap.atom_name(req.specifier);
                let k = loader
                    .resolve(&s, Some(&referrer), attrs)
                    .map_err(host_error)?;
                records.push(self.module_record(&k, json, loader, created, &mut queue)?);
            }
            module::set_requested(&mut self.heap, m, &records)?;
        }
        module::link(&mut self.heap, root)?;
        Ok(key)
    }

    /// The record of `key`: loaded and compiled now if new (and queued for its requests).
    /// `json`: the import has `type: "json"`, and the module must be a JSON module (and
    /// one without must not be).
    fn module_record(
        &mut self,
        key: &str,
        json: bool,
        loader: &mut dyn ModuleLoader,
        created: &mut Vec<String>,
        queue: &mut Vec<Obj>,
    ) -> Result<Obj, Fault> {
        let mismatch = |is_json: bool| {
            Fault::error(
                ErrorKind::TypeError,
                if is_json {
                    crate::text::cat!("module '", key, "' is JSON: import it with type 'json'")
                } else {
                    crate::text::cat!("module '", key, "' is not a JSON module")
                },
            )
        };
        if let Some(m) = module::find(&mut self.heap, key)? {
            let is_json = module::flags_of(&self.heap, m) & module::flags::JSON != 0;
            if is_json != json {
                return Err(mismatch(is_json));
            }
            return Ok(m);
        }
        let src = loader.load(key).map_err(host_error)?;
        if (src.kind == ModuleKind::Json) != json {
            return Err(mismatch(!json));
        }
        if json {
            // ParseJSONModule: JSON.parse of the text, the default export of a synthetic
            // module.
            let opts = crate::json::ParseOptions {
                object_proto: realm::intrinsic(&self.heap, realm::Intrinsic::ObjectProto)
                    .as_object(),
                array_proto: realm::intrinsic(&self.heap, realm::Intrinsic::ArrayProto).as_object(),
                ..Default::default()
            };
            let v = match crate::json::parse_utf8(&mut self.heap, src.text.as_bytes(), &opts) {
                Ok(v) => v,
                Err(crate::json::ParseError::Heap(e)) => return Err(Fault::heap(e)),
                Err(crate::json::ParseError::Syntax(e)) => {
                    return Err(Fault::error(
                        ErrorKind::SyntaxError,
                        crate::text::cat!(&e.message, " in JSON module '", key, "'"),
                    ));
                }
            };
            let m = module::new_synthetic(
                &mut self.heap,
                key,
                &[crate::heap::atoms::DEFAULT],
                &[v],
                module::flags::JSON,
            )?;
            created.push(String::from(key));
            return Ok(m);
        }
        let mut opts = CompileOptions::module(key);
        opts.typescript = src.kind == ModuleKind::TypeScript;
        let (t, meta) =
            compile_module(&mut self.heap, src.text.as_bytes(), &opts).map_err(|e| {
                if e.kind == crate::compiler::CompileErrorKind::Syntax {
                    Fault::error(
                        ErrorKind::SyntaxError,
                        crate::text::cat!(&e.message, " at ", key, ":", e.line, ":", e.col),
                    )
                } else {
                    Fault::heap(crate::heap::HeapError::OutOfMemory)
                }
            })?;
        let m = module::new_record(&mut self.heap, key, t, meta)?;
        created.push(String::from(key));
        queue.push(m);
        Ok(m)
    }

    /// Defines synthetic module `key` (the JSON modules proposal's Synthetic Module Record):
    /// a module without source whose exports are `exports` (name and value: host
    /// functions, data, anything the VM holds). It counts as linked and evaluated: a graph
    /// that imports `key` links to it without asking the loader for it (resolve still maps
    /// specifiers to the key), and its exports are live ([`Vm::set_module_export`]). Fails
    /// with an Error if a module `key` exists, a SyntaxError for a duplicate export name.
    pub fn define_synthetic_module(
        &mut self,
        key: &str,
        exports: &[(&str, Value)],
    ) -> Result<(), Value> {
        if !crate::MODULES {
            return Err(self.error_value(
                ErrorKind::SyntaxError,
                "modules are not available in this build",
            ));
        }
        let r = (|| -> Result<(), Fault> {
            if module::find(&mut self.heap, key)?.is_some() {
                return Err(Fault::error(
                    ErrorKind::Error,
                    crate::text::cat!("module '", key, "' is loaded already"),
                ));
            }
            let mut names = Vec::with_capacity(exports.len());
            for &(n, _) in exports {
                let a = self.heap.atom_str(n)?;
                if names.contains(&a) {
                    return Err(Fault::error(
                        ErrorKind::SyntaxError,
                        crate::text::cat!("duplicate export '", n, "'"),
                    ));
                }
                names.push(a);
            }
            let values: Vec<Value> = exports.iter().map(|e| e.1).collect();
            module::new_synthetic(&mut self.heap, key, &names, &values, 0)?;
            Ok(())
        })();
        r.map_err(|f| self.fault_to_value(f))
    }

    /// Sets export `name` of synthetic module `key` to `v` (SetSyntheticModuleExport):
    /// importers read the new value from then on. `false` if there is no synthetic module
    /// `key` or it has no export `name`.
    pub fn set_module_export(&mut self, key: &str, name: &str, v: Value) -> Result<bool, VmError> {
        let Some(m) = module::find(&mut self.heap, key)? else {
            return Ok(false);
        };
        Ok(module::set_synthetic_export(&mut self.heap, m, name, v)?)
    }

    /// Lets `import()` load modules through the host (`on`), in modules and scripts alike:
    /// a call becomes an async host call with fn id [`IMPORT_FN_ID`] and the arguments
    /// specifier, referrer (the importing module's key or script's name) and the `type`
    /// import attribute (a string, or undefined without one), which the host
    /// answers with [`Vm::answer_import`]; the VM then evaluates the graph in a later run
    /// and fulfills the promise with its namespace. Off (the default), `import()` rejects
    /// as in a build without modules. Kept in the heap: images carry it.
    pub fn set_dynamic_import(&mut self, on: bool) -> Result<(), VmError> {
        if !crate::MODULES {
            return Ok(());
        }
        let f = if on {
            self.new_async_host_function(IMPORT_FN_ID, "import")?
        } else {
            Value::UNDEFINED
        };
        module::set_import_function(&mut self.heap, f)?;
        Ok(())
    }

    /// The specifier and referrer of pending host call `call_id` if it is an `import()`
    /// ([`Vm::set_dynamic_import`]).
    pub fn import_request(&self, call_id: u32) -> Option<(String, String)> {
        let c = self.host_call(call_id)?;
        if c.fn_id != IMPORT_FN_ID || !c.is_async {
            return None;
        }
        let s = |v: Option<&Value>| {
            v.and_then(|v| v.as_string())
                .map(|s| self.heap.str_to_string(s))
                .unwrap_or_default()
        };
        Some((s(c.args.first()), s(c.args.get(1))))
    }

    /// Answers `import()` call `call_id` (see [`Vm::import_request`]): loads and links the
    /// graph of its specifier through `loader` ([`Vm::load_module_from`] with its referrer)
    /// and resolves the call, so that the next run evaluates the graph and fulfills the
    /// import's promise with the namespace; a load or link error rejects it. Runs no JS.
    pub fn answer_import(
        &mut self,
        call_id: u32,
        loader: &mut dyn ModuleLoader,
    ) -> Result<(), VmError> {
        let Some((specifier, referrer)) = self.import_request(call_id) else {
            return Err(VmError::UnknownHostCall);
        };
        // The third argument: the `type` import attribute, if any.
        let ty = self
            .host_call(call_id)
            .and_then(|c| c.args.get(2).copied())
            .and_then(|v| v.as_string())
            .map(|s| self.heap.str_to_string(s));
        let result = match self.load_request(&specifier, Some(&referrer), ty.as_deref(), loader) {
            Ok(key) => Ok(Value::string(self.heap.alloc_str(&key)?)),
            Err(e) => Err(e),
        };
        self.resolve_host_call(call_id, result)
    }

    /// Starts the evaluation of the loaded module `key` (Evaluate(), 16.2.1.5.3): `run`
    /// executes it like a [`Vm::call`], and it ends `Done` with a promise, fulfilled once
    /// the graph has run, or rejected with the error a module threw. A module evaluates
    /// once: a later call settles the same way without running anything (an error is
    /// kept). `Ok(false)` if no module `key` is loaded.
    pub fn evaluate_module(&mut self, key: &str) -> Result<bool, VmError> {
        let Some(m) = module::find(&mut self.heap, key)? else {
            return Ok(false);
        };
        if module::status_of(&self.heap, m) < status::LINKED {
            return Ok(false);
        }
        use crate::bytecode::intrinsics::intrinsic_id_of as id;
        // `%ModuleOp` with `this` 5: Evaluate (builtins/module.ts).
        let f = realm::installed_intrinsic(&self.heap, id!("ModuleOp"));
        self.call(f, Value::int(5), &[Value::object(m)])?;
        Ok(true)
    }

    /// Whether a module `key` is loaded (and linked).
    pub fn has_module(&mut self, key: &str) -> bool {
        matches!(module::find(&mut self.heap, key), Ok(Some(m)) if module::status_of(&self.heap, m) >= status::LINKED)
    }

    /// The namespace object of the loaded module `key` (made on first use), or `None`.
    pub fn module_namespace(&mut self, key: &str) -> Result<Option<Value>, VmError> {
        let Some(m) = self.linked_module(key)? else {
            return Ok(None);
        };
        match module::namespace(&mut self.heap, m) {
            Ok(ns) => Ok(Some(Value::object(ns))),
            Err(f) => Err(fault_vm_error(f)),
        }
    }

    /// The current value of export `name` of the loaded module `key`: `None` if there is
    /// no such module or export (or it is ambiguous), or the binding is not initialized
    /// yet (its module has not run that far).
    pub fn module_export(&mut self, key: &str, name: &str) -> Result<Option<Value>, VmError> {
        let Some(m) = self.linked_module(key)? else {
            return Ok(None);
        };
        module::export_value(&mut self.heap, m, name).map_err(fault_vm_error)
    }

    fn linked_module(&mut self, key: &str) -> Result<Option<Obj>, VmError> {
        let m = module::find(&mut self.heap, key)?;
        Ok(m.filter(|&m| module::status_of(&self.heap, m) >= status::LINKED))
    }

    fn error_value(&mut self, kind: ErrorKind, msg: &str) -> Value {
        realm::new_error(&mut self.heap, kind, msg).unwrap_or(Value::UNDEFINED)
    }

    fn fault_to_value(&mut self, f: Fault) -> Value {
        match *f.0 {
            FaultKind::Throw(v) => v,
            FaultKind::Error(kind, msg) => self.error_value(kind, &msg),
            FaultKind::Heap(e) => self.error_value(ErrorKind::InternalError, e.as_str()),
        }
    }
}

/// Whether a request of module type `ty` (the `type` import attribute) is for a JSON
/// module; a TypeError for a type djs has no modules of.
fn check_type(ty: Option<&str>) -> Result<bool, Fault> {
    match ty {
        None => Ok(false),
        Some("json") => Ok(true),
        Some(t) => Err(Fault::error(
            ErrorKind::TypeError,
            crate::text::cat!("unsupported module type '", t, "'"),
        )),
    }
}

fn host_error(msg: String) -> Fault {
    Fault::error(ErrorKind::Error, msg)
}

fn fault_vm_error(f: Fault) -> VmError {
    match *f.0 {
        FaultKind::Heap(e) => VmError::Heap(e),
        _ => VmError::Heap(crate::heap::HeapError::OutOfMemory),
    }
}
