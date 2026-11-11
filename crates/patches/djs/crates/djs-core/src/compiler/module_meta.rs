//! The metadata of a compiled module ([`super::compile_module`]): what ParseModule
//! (ECMA-262 16.2.1.6.1) records about a module besides its code, and how its
//! environment is laid out. Stored as a U32Array of atom ids and integers (atoms never
//! die, so the array needs no tracing):
//!
//! ```text
//! n_env n_req n_imp n_local n_indirect n_star
//! env       n_env × [name, kind | aux << 3]   the module closure's captures, in order
//! requests  n_req × [specifier, type, other]  [[RequestedModules]], in source order
//! imports   n_imp × [env, request, name]      name NULL: a namespace import
//! local     n_local × [export name, env]
//! indirect  n_indirect × [export name, request, import name]   import NULL: `export * as`
//! star      n_star × [request]
//! ```
//!
//! An environment entry's kind says how the linker initializes its Box (`ENV_*`); `aux` is
//! a function's child index in the module template. A request's import attributes are the
//! value of its `type` attribute and the first other key (each an atom or NULL): djs
//! supports `type` alone, and an other key is a SyntaxError when the graph loads
//! (AllImportAttributesSupported), not when the module parses.

use alloc::vec::Vec;

use crate::heap::Atom;

/// A `var`: `undefined`.
pub const ENV_VAR: u32 = 0;
/// `let`, `const`, `class`, the hidden `*default*`: uninitialized (TDZ).
pub const ENV_LEXICAL: u32 = 1;
/// A function declaration: the linker instantiates child `aux`.
pub const ENV_FUNCTION: u32 = 2;
/// An import: the exporting module's Box (or one holding a namespace).
pub const ENV_IMPORT: u32 = 3;
/// The `import.meta` object.
pub const ENV_META: u32 = 4;

/// A ModuleRequest Record: a specifier with its import attributes, reduced to what djs
/// supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub specifier: Atom,
    /// The value of the `type` attribute; NULL without one.
    pub ty: Atom,
    /// The first attribute key other than `type`; NULL if there is none.
    pub unsupported: Atom,
}

/// The decoded metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleMeta {
    /// `(name, kind, aux)` per environment entry.
    pub env: Vec<(Atom, u32, u32)>,
    pub requests: Vec<Request>,
    /// `(env index, request, import name)`; NULL: the namespace.
    pub imports: Vec<(u32, u32, Atom)>,
    /// `(export name, env index)`.
    pub local: Vec<(Atom, u32)>,
    /// `(export name, request, import name)`; NULL: `export * as name`.
    pub indirect: Vec<(Atom, u32, Atom)>,
    pub star: Vec<u32>,
}

impl ModuleMeta {
    pub fn encode(&self) -> Vec<u32> {
        let mut w = Vec::new();
        for n in [
            self.env.len(),
            self.requests.len(),
            self.imports.len(),
            self.local.len(),
            self.indirect.len(),
            self.star.len(),
        ] {
            w.push(n as u32);
        }
        for &(name, kind, aux) in &self.env {
            w.extend([name.id(), kind | aux << 3]);
        }
        for r in &self.requests {
            w.extend([r.specifier.id(), r.ty.id(), r.unsupported.id()]);
        }
        for &(e, r, n) in &self.imports {
            w.extend([e, r, n.id()]);
        }
        for &(n, e) in &self.local {
            w.extend([n.id(), e]);
        }
        for &(n, r, i) in &self.indirect {
            w.extend([n.id(), r, i.id()]);
        }
        w.extend(&self.star);
        w
    }

    /// The metadata of `w` (`encode`'s output); `None` if it is malformed.
    pub fn decode(w: &[u32]) -> Option<ModuleMeta> {
        let mut i = 6;
        let mut take = |n: usize| {
            let s = w.get(i..i + n)?;
            i += n;
            Some(s)
        };
        let count = |k: usize| w.get(k).map(|&n| n as usize);
        let mut m = ModuleMeta::default();
        let a = Atom::from_id;
        for c in take(2 * count(0)?)?.chunks(2) {
            m.env.push((a(c[0]), c[1] & 7, c[1] >> 3));
        }
        for c in take(3 * count(1)?)?.chunks(3) {
            m.requests.push(Request {
                specifier: a(c[0]),
                ty: a(c[1]),
                unsupported: a(c[2]),
            });
        }
        for c in take(3 * count(2)?)?.chunks(3) {
            m.imports.push((c[0], c[1], a(c[2])));
        }
        for c in take(2 * count(3)?)?.chunks(2) {
            m.local.push((a(c[0]), c[1]));
        }
        for c in take(3 * count(4)?)?.chunks(3) {
            m.indirect.push((a(c[0]), c[1], a(c[2])));
        }
        m.star = take(count(5)?)?.to_vec();
        Some(m)
    }
}
