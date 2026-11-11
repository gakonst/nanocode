//! ES modules: the host's [`Loader`], the in-memory [`ModuleMap`] and [`SyntheticModule`]s.

use std::collections::BTreeMap;

use djs_core::vm::{ModuleKind, ModuleLoader, ModuleSource as CoreSource};

use crate::{Data, HostFn};

/// A module's source text, as a [`Loader`] returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleSource {
    pub(crate) text: String,
    pub(crate) kind: ModuleKind,
}

impl ModuleSource {
    /// JavaScript.
    pub fn js(text: impl Into<String>) -> ModuleSource {
        ModuleSource {
            text: text.into(),
            kind: ModuleKind::JavaScript,
        }
    }

    /// TypeScript: types are erased as the module compiles (imports used only as types are
    /// dropped).
    pub fn typescript(text: impl Into<String>) -> ModuleSource {
        ModuleSource {
            text: text.into(),
            kind: ModuleKind::TypeScript,
        }
    }

    /// JSON text: a JSON module, whose default export is the parsed value (a fresh copy
    /// per instance state, like any module's). Only an import with `with { type: "json" }`
    /// may load it, and such an import loads nothing else (a `TypeError` either way).
    pub fn json(text: impl Into<String>) -> ModuleSource {
        ModuleSource {
            text: text.into(),
            kind: ModuleKind::Json,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_typescript(&self) -> bool {
        self.kind == ModuleKind::TypeScript
    }

    pub fn is_json(&self) -> bool {
        self.kind == ModuleKind::Json
    }
}

/// Where module sources come from. Specifiers and keys are opaque strings: djs gives them
/// no URL or path meaning, and does no I/O. A key names one module: an instance loads each
/// key once (per snapshot state) and keeps it. Called synchronously, while building the
/// snapshot (static imports) and inside [`crate::Instance::step`] (an `import()`), so it
/// must not block for long; the snapshot keeps it, hence `Send + Sync`.
pub trait Loader: Send + Sync {
    /// The key of the module `specifier` names when the module (or script) `referrer`
    /// imports it; `None` for a setup [`crate::Source`]'s own name. An error becomes an
    /// `Error` with the message (a failed build, or a rejected `import()`).
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String>;
    /// The source of module `key`.
    fn load(&self, key: &str) -> Result<ModuleSource, String>;
}

/// An in-memory [`Loader`]: module sources by key. A specifier that starts with `./` or
/// `../` is resolved against the referrer's key as a `/`-separated path (`lib/a.js`
/// importing `./b.js` gets `lib/b.js`; `..` above the root is an error); any other
/// specifier is a key as it is. A key that is not in the map is an error
/// (`cannot find module 'x' imported by 'y'`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleMap(BTreeMap<String, ModuleSource>);

impl ModuleMap {
    pub fn new() -> ModuleMap {
        ModuleMap::default()
    }

    /// Adds (or replaces) JavaScript module `key`.
    pub fn module(mut self, key: impl Into<String>, text: impl Into<String>) -> ModuleMap {
        self.0.insert(key.into(), ModuleSource::js(text));
        self
    }

    /// Adds (or replaces) JSON module `key` (imported `with { type: "json" }`).
    pub fn json(mut self, key: impl Into<String>, text: impl Into<String>) -> ModuleMap {
        self.0.insert(key.into(), ModuleSource::json(text));
        self
    }

    /// Adds (or replaces) module `key`.
    pub fn insert(&mut self, key: impl Into<String>, source: ModuleSource) {
        self.0.insert(key.into(), source);
    }

    pub fn get(&self, key: &str) -> Option<&ModuleSource> {
        self.0.get(key)
    }
}

impl<K: Into<String>, S: Into<String>> FromIterator<(K, S)> for ModuleMap {
    /// JavaScript modules from `(key, text)` pairs.
    fn from_iter<I: IntoIterator<Item = (K, S)>>(iter: I) -> Self {
        let mut m = ModuleMap::new();
        for (k, s) in iter {
            m.insert(k, ModuleSource::js(s));
        }
        m
    }
}

impl Loader for ModuleMap {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        let missing = || match referrer {
            Some(r) => format!("cannot find module '{specifier}' imported by '{r}'"),
            None => format!("cannot find module '{specifier}'"),
        };
        let key = if specifier.starts_with("./") || specifier.starts_with("../") {
            join(referrer.unwrap_or(""), specifier).ok_or_else(missing)?
        } else {
            specifier.to_owned()
        };
        if self.0.contains_key(&key) {
            Ok(key)
        } else {
            Err(missing())
        }
    }

    fn load(&self, key: &str) -> Result<ModuleSource, String> {
        self.0
            .get(key)
            .cloned()
            .ok_or_else(|| format!("cannot find module '{key}'"))
    }
}

/// `relative` (`./x`, `../x`) against the directory of key `base`, `.` and `..` segments
/// removed; `None` if it climbs above the root.
fn join(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for seg in relative.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// One export of a [`SyntheticModule`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Export {
    Fn(HostFn),
    Data(String, Data),
}

/// A module the host defines without source (`@sdk/net`): its exports are host functions
/// and plain data. Scripts and modules import it by its key, verbatim (`import { allowHosts }
/// from "@sdk/net"`, `import("@sdk/net")`), without the [`Loader`]: its key wins over
/// anything the loader would resolve the same specifier to. It has no code to run; its
/// exports are set when the snapshot is built.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticModule {
    pub(crate) key: String,
    pub(crate) exports: Vec<Export>,
}

impl SyntheticModule {
    pub fn new(key: impl Into<String>) -> SyntheticModule {
        SyntheticModule {
            key: key.into(),
            exports: Vec::new(),
        }
    }

    /// Exports host function `f` under its name. It is not a global; the host answers its
    /// calls by id like any host function's.
    pub fn host_fn(mut self, f: HostFn) -> SyntheticModule {
        self.exports.push(Export::Fn(f));
        self
    }

    /// Exports plain data under `name` (a fresh copy in every instance, as the snapshot
    /// left it).
    pub fn data(mut self, name: impl Into<String>, value: Data) -> SyntheticModule {
        self.exports.push(Export::Data(name.into(), value));
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

/// A [`Loader`] the snapshot shares with its instances.
#[derive(Clone)]
pub(crate) struct SharedLoader(pub std::sync::Arc<dyn Loader>);

impl std::fmt::Debug for SharedLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Loader")
    }
}

/// djs-core's loader over the snapshot's: synthetic module keys first, then a setup
/// source's own text, then the host's [`Loader`].
pub(crate) struct Adapter<'a> {
    pub loader: Option<&'a dyn Loader>,
    pub synthetic: &'a [SyntheticModule],
    /// A setup module: its name and source, which the loader is not asked for
    pub entry: Option<(&'a str, &'a ModuleSource)>,
}

impl ModuleLoader for Adapter<'_> {
    fn resolve(
        &mut self,
        specifier: &str,
        referrer: Option<&str>,
        // Only `type` reaches a loader; djs-core checks it against the source's kind.
        _attributes: &[(&str, &str)],
    ) -> Result<String, String> {
        if self.synthetic.iter().any(|m| m.key == specifier)
            || self
                .entry
                .is_some_and(|e| e.0 == specifier && referrer.is_none())
        {
            return Ok(specifier.to_owned());
        }
        match self.loader {
            Some(l) => l.resolve(specifier, referrer),
            None => Err(format!(
                "cannot find module '{specifier}' (the snapshot has no module loader)"
            )),
        }
    }

    fn load(&mut self, key: &str) -> Result<CoreSource, String> {
        let source = match self.entry {
            Some((name, s)) if name == key => s.clone(),
            _ => match self.loader {
                Some(l) => l.load(key)?,
                None => return Err(format!("cannot find module '{key}'")),
            },
        };
        Ok(CoreSource {
            text: source.text,
            kind: source.kind,
        })
    }
}
