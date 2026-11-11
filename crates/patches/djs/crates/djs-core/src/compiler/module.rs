//! Module syntax. Module code (`Goal::Module`, D3171): import and export declarations
//! become the module's metadata (compiler/module_meta.rs) and its top-level bindings the
//! module environment, which the linker sets up (vm/module.rs). And for `strip_types`
//! (0048): the TypeScript corpus of the strip-types oracle is modules, so `import` and
//! `export` declarations parse in that mode (`Parser::strip`) and compile to nothing
//! meaningful. Type-only imports and exports (`import type`, `export type`, `type`
//! specifiers) are erased like ts-blank-space erases them.

use alloc::vec::Vec;

use super::PResult;
use super::func::{VarId, VarKind};
use super::module_meta::{
    ENV_FUNCTION, ENV_IMPORT, ENV_LEXICAL, ENV_META, ENV_VAR, ModuleMeta, Request,
};
use super::parser::{Parser, syntax};
use crate::heap::{Atom, Ref};
use crate::lexer::{Kw, TokenKind};

impl Parser<'_, '_> {
    /// At `import` or `export`: a declaration (not `import(...)` or `import.meta`)?
    pub(super) fn strip_at_module_item(&self) -> bool {
        !(self.is(TokenKind::Import)
            && matches!(self.peek().kind, TokenKind::LParen | TokenKind::Dot))
    }

    pub(super) fn strip_module_item(&mut self) -> PResult<()> {
        let start = self.tok.start;
        let type_only = if self.is(TokenKind::Import) {
            self.strip_import(start)?
        } else {
            self.strip_export(start)?
        };
        if !type_only {
            self.ts_note_stmt_end();
        }
        self.ts_type_only = type_only;
        Ok(())
    }

    /// A module specifier string.
    fn strip_from(&mut self) -> PResult<()> {
        if !self.tok.is_contextual(Kw::From) {
            return syntax(self.tok.start, "expecting 'from'");
        }
        self.next()?;
        self.strip_module_name()
    }

    fn strip_module_name(&mut self) -> PResult<()> {
        if !self.is(TokenKind::String) {
            return syntax(self.tok.start, "expecting string");
        }
        self.next()?;
        // Import attributes: `with { type: "json" }`.
        if (self.is(TokenKind::With)
            || (self.lx.text(&self.tok) == b"assert" && self.is(TokenKind::Ident)))
            && !self.tok.newline_before()
        {
            self.next()?;
            if !self.is(TokenKind::LBrace) {
                return syntax(self.tok.start, "expecting '{'");
            }
            self.ts_skip_balanced()?;
        }
        Ok(())
    }

    /// A name in an import or export list: an identifier name or a string.
    fn strip_module_export_name(&mut self) -> PResult<()> {
        if !self.tok.kind.is_identifier_name() && !self.is(TokenKind::String) {
            return syntax(self.tok.start, "identifier expected");
        }
        self.next()
    }

    /// `{ a, b as c, type T }` of an import or export: `type` specifiers are erased with
    /// their comma.
    fn strip_specifiers(&mut self) -> PResult<()> {
        self.expect(TokenKind::LBrace, "{")?;
        while !self.is(TokenKind::RBrace) {
            let start = self.tok.start;
            // `type` is a modifier before a name, also before one named `as`
            // (`{ type as }`), but not in `{ type as x }`.
            let is_type = self.tok.is_contextual(Kw::Type) && {
                let mut lx = self.lx;
                let n = lx.next_token().ok();
                let n2 = lx.next_token().ok();
                match n {
                    Some(n) if n.is_contextual(Kw::As) => {
                        n2.is_some_and(|t| matches!(t.kind, TokenKind::Comma | TokenKind::RBrace))
                    }
                    Some(n) => n.kind.is_identifier_name() || n.kind == TokenKind::String,
                    None => false,
                }
            };
            if is_type {
                self.next()?;
            }
            self.strip_module_export_name()?;
            if self.tok.is_contextual(Kw::As) {
                self.next()?;
                self.strip_module_export_name()?;
            }
            let more = self.eat(TokenKind::Comma)?;
            if is_type {
                self.ts_erase(start);
            }
            if !more {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "}")
    }

    /// An import declaration at `import`. Returns whether it is type-only (erased).
    fn strip_import(&mut self, start: u32) -> PResult<bool> {
        self.next()?;
        if self.is(TokenKind::String) {
            self.strip_module_name()?;
            self.expect_semicolon()?;
            return Ok(false);
        }
        // `import type X from` (not `import type from "m"`, the default import `type`);
        // `import defer * as ns from` likewise.
        let modifier = |p: &Self| {
            let mut lx = p.lx;
            let n = lx.next_token().ok();
            let n2 = lx.next_token().ok();
            match n {
                Some(n) if matches!(n.kind, TokenKind::LBrace | TokenKind::Star) => true,
                Some(n) if n.is_contextual(Kw::From) => {
                    n2.is_some_and(|t| t.is_contextual(Kw::From))
                }
                Some(n) => n.kind == TokenKind::Ident,
                None => false,
            }
        };
        let type_only = self.tok.is_contextual(Kw::Type) && modifier(self);
        if type_only || (self.lx.text(&self.tok) == b"defer" && modifier(self)) {
            self.next()?;
        }
        if self.is(TokenKind::Ident) {
            self.next()?;
            if !self.eat(TokenKind::Comma)? {
                self.strip_from()?;
                return self.strip_import_end(start, type_only);
            }
        }
        if self.is(TokenKind::Star) {
            self.next()?;
            if !self.tok.is_contextual(Kw::As) {
                return syntax(self.tok.start, "expecting 'as'");
            }
            self.next()?;
            self.strip_module_export_name()?;
        } else {
            self.strip_specifiers()?;
        }
        self.strip_from()?;
        self.strip_import_end(start, type_only)
    }

    fn strip_import_end(&mut self, start: u32, type_only: bool) -> PResult<bool> {
        self.expect_semicolon()?;
        if type_only {
            self.ts_erase_stmt(start);
        }
        Ok(type_only)
    }

    /// An export declaration at `export`. Returns whether it is type-only (erased).
    fn strip_export(&mut self, start: u32) -> PResult<bool> {
        self.next()?;
        let tok = self.tok;
        match tok.kind {
            TokenKind::Star => {
                self.next()?;
                if self.tok.is_contextual(Kw::As) {
                    self.next()?;
                    self.strip_module_export_name()?;
                }
                self.strip_from()?;
                self.expect_semicolon()?;
                return Ok(false);
            }
            TokenKind::LBrace => {
                self.strip_specifiers()?;
                if self.tok.is_contextual(Kw::From) {
                    self.strip_from()?;
                }
                self.expect_semicolon()?;
                return Ok(false);
            }
            TokenKind::Default => {
                self.next()?;
                return self.strip_export_default(start);
            }
            TokenKind::Assign => {
                return syntax(tok.start, "'export =' is not supported");
            }
            TokenKind::Ident
                if tok.is_contextual(Kw::Type)
                    && matches!(self.peek().kind, TokenKind::LBrace | TokenKind::Star) =>
            {
                // `export type { T } [from "m"]`, `export type * from "m"`.
                self.next()?;
                if self.is(TokenKind::Star) {
                    self.next()?;
                    if self.tok.is_contextual(Kw::As) {
                        self.next()?;
                        self.strip_module_export_name()?;
                    }
                    self.strip_from()?;
                } else {
                    self.strip_specifiers()?;
                    if self.tok.is_contextual(Kw::From) {
                        self.strip_from()?;
                    }
                }
                self.expect_semicolon()?;
                self.ts_erase_stmt(start);
                return Ok(true);
            }
            TokenKind::Ident if tok.is_contextual(Kw::As) => {
                // `export as namespace N;` (declaration files): kept, like ts-blank-space.
                self.next()?;
                self.next()?;
                self.strip_module_export_name()?;
                self.expect_semicolon()?;
                return Ok(false);
            }
            _ => {}
        }
        // `export <declaration>`: a type-only one is erased from the `export`.
        let mark = self.erased.as_ref().map_or(0, |e| e.len());
        self.parse_statement_list_item()?;
        let type_only = self.ts_type_only;
        if type_only && let Some(e) = &mut self.erased {
            for r in &mut e[mark..] {
                if r.0 == tok.start {
                    r.0 = start;
                    break;
                }
            }
        }
        Ok(type_only)
    }

    /// `export default ...` after `default`.
    fn strip_export_default(&mut self, start: u32) -> PResult<bool> {
        let tok = self.tok;
        if tok.is_contextual(Kw::Interface) && self.peek().kind == TokenKind::Ident {
            self.ts_interface()?;
            self.ts_erase_stmt(start);
            return Ok(true);
        }
        if tok.is_contextual(Kw::Abstract) && self.peek().kind == TokenKind::Class {
            self.next()?;
            self.ts_erase(tok.start);
        }
        match self.tok.kind {
            TokenKind::Function => {
                if self.ts_lookahead(|p| p.ts_skip_signature()) {
                    // An overload signature of the default export.
                    let s = self.tok.start;
                    self.ts_statement()?;
                    if let Some(e) = &mut self.erased
                        && let Some(r) = e.iter_mut().rev().find(|r| r.0 == s)
                    {
                        r.0 = start;
                    }
                    return Ok(true);
                }
                let s = self.tok.start;
                self.parse_function_expression(Atom::NULL, s, false)?;
                self.code().drop();
            }
            TokenKind::Ident if self.at_async_function() => {
                let s = self.tok.start;
                self.next()?;
                self.parse_function_expression(Atom::NULL, s, true)?;
                self.code().drop();
            }
            TokenKind::Class => {
                self.parse_class(false, Atom::NULL)?;
                self.code().drop();
            }
            _ => {
                self.parse_assign(false)?;
                self.code().drop();
                self.expect_semicolon()?;
            }
        }
        Ok(false)
    }
}

// ---- Module code (`Goal::Module`, D3171) ---------------------------------------------

/// An export entry of the module being parsed (ExportEntry, 16.2.1.6.1 table 59).
#[derive(Clone, Copy)]
pub(super) struct ExportEntry {
    pub name: Atom,
    /// The local binding of a local export; NULL for an indirect one.
    pub local: Atom,
    /// The request of an indirect export, else `NO_REQUEST`.
    pub request: u32,
    /// An indirect export's import name; NULL: `export * as name`.
    pub import: Atom,
    pub pos: u32,
}

const NO_REQUEST: u32 = u32::MAX;

/// The import and export entries of a module while it is parsed, and the bindings an
/// export declaration declares.
pub(super) struct ModuleBuild {
    /// The module specifiers in source order, deduplicated, with whether something
    /// keeps the request (TypeScript drops the imports no value refers to).
    pub requests: Vec<(Request, bool)>,
    /// `(binding, request, import name or NULL for a namespace import)`.
    pub imports: Vec<(VarId, u32, Atom)>,
    pub exports: Vec<ExportEntry>,
    /// `export * from` requests.
    pub stars: Vec<u32>,
    /// Set while an `export` declaration is parsed: the names it binds.
    pub collect: Option<Vec<(Atom, u32)>>,
    /// The hidden binding of `import.meta`, once referred to.
    pub meta: Option<VarId>,
    pub atom_default: Atom,
    /// The hidden binding of `export default <expression>` and of an anonymous default
    /// function or class (not an identifier, so no source name reaches it).
    pub atom_star_default: Atom,
    pub atom_meta: Atom,
}

impl ModuleBuild {
    pub fn new(heap: &mut crate::heap::Heap) -> PResult<ModuleBuild> {
        Ok(ModuleBuild {
            requests: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            stars: Vec::new(),
            collect: None,
            meta: None,
            atom_default: heap.atom_str("default")?,
            atom_star_default: heap.atom_str("*default*")?,
            atom_meta: heap.atom_str("*meta*")?,
        })
    }
}

/// The parser's module state: `Option<Box<ModuleBuild>>`, and in builds without modules a
/// unit that is never set, so that their parser stays as it is (D3190).
#[cfg(feature = "embed")]
#[derive(Default)]
pub(super) struct ModuleSlot(Option<alloc::boxed::Box<ModuleBuild>>);
#[cfg(not(feature = "embed"))]
#[derive(Default)]
pub(super) struct ModuleSlot;

impl ModuleSlot {
    #[allow(unused_variables)]
    pub fn new(m: ModuleBuild) -> ModuleSlot {
        #[cfg(feature = "embed")]
        return ModuleSlot(Some(alloc::boxed::Box::new(m)));
        #[cfg(not(feature = "embed"))]
        return ModuleSlot;
    }

    #[inline(always)]
    pub fn is_some(&self) -> bool {
        #[cfg(feature = "embed")]
        return self.0.is_some();
        #[cfg(not(feature = "embed"))]
        return false;
    }

    pub fn as_mut(&mut self) -> Option<&mut ModuleBuild> {
        #[cfg(feature = "embed")]
        return self.0.as_deref_mut();
        #[cfg(not(feature = "embed"))]
        return None;
    }

    pub fn take(&mut self) -> Option<alloc::boxed::Box<ModuleBuild>> {
        #[cfg(feature = "embed")]
        return self.0.take();
        #[cfg(not(feature = "embed"))]
        return None;
    }
}

fn well_formed(units: &[u16]) -> bool {
    let mut i = 0;
    while i < units.len() {
        match units[i] {
            0xD800..=0xDBFF
                if units
                    .get(i + 1)
                    .is_some_and(|u| (0xDC00..=0xDFFF).contains(u)) =>
            {
                i += 2
            }
            0xD800..=0xDFFF => return false,
            _ => i += 1,
        }
    }
    true
}

impl Parser<'_, '_> {
    /// In the top-level function of module code (not in a nested function).
    pub(super) fn at_module_top(&self) -> bool {
        crate::MODULES && self.module.is_some() && self.stack.is_empty()
    }

    fn mb(&mut self) -> &mut ModuleBuild {
        self.module.as_mut().expect("module code")
    }

    /// A top-level declaration of `name`: an export declaration being parsed exports it.
    pub(super) fn note_export(&mut self, name: Atom, pos: u32) {
        if self.at_module_top()
            && let Some(c) = &mut self.mb().collect
        {
            c.push((name, pos));
        }
    }

    /// An import or export declaration at the top of module code.
    pub(super) fn module_item(&mut self) -> PResult<()> {
        if self.is(TokenKind::Import) {
            self.module_import()?;
        } else {
            self.module_export()?;
        }
        self.ts_type_only = false;
        Ok(())
    }

    /// The request for specifier `spec` (ModuleRequests: one per distinct specifier).
    fn module_request(&mut self, spec: Request, keep: bool) -> u32 {
        let m = self.mb();
        // ModuleRequestsEqual: the same specifier and attributes.
        if let Some(i) = m.requests.iter().position(|r| r.0 == spec) {
            m.requests[i].1 |= keep;
            return i as u32;
        }
        m.requests.push((spec, keep));
        (m.requests.len() - 1) as u32
    }

    /// A ModuleExportName: an identifier name or a well-formed string. Returns its atom
    /// and whether it was a string.
    fn module_export_name(&mut self) -> PResult<(Atom, bool)> {
        if self.is(TokenKind::String) {
            let units = self.cooked_string();
            if !well_formed(&units) {
                return syntax(self.tok.start, "invalid module export name");
            }
            let a = self.atom_of_units(&units)?;
            self.next()?;
            return Ok((a, true));
        }
        if !self.tok.kind.is_identifier_name() {
            return syntax(self.tok.start, "identifier expected");
        }
        let a = self.ident_atom()?;
        self.next()?;
        Ok((a, false))
    }

    /// `from "specifier"` with its import attributes, and the `;`.
    fn module_from(&mut self) -> PResult<Request> {
        if !self.tok.is_contextual(Kw::From) {
            return syntax(self.tok.start, "expecting 'from'");
        }
        self.next()?;
        self.module_specifier()
    }

    /// The module specifier string, its import attributes (a WithClause) and the `;`.
    fn module_specifier(&mut self) -> PResult<Request> {
        if !self.is(TokenKind::String) {
            return syntax(self.tok.start, "expecting string");
        }
        let units = self.cooked_string();
        let specifier = self.atom_of_units(&units)?;
        self.next()?;
        let mut req = Request {
            specifier,
            ty: Atom::NULL,
            unsupported: Atom::NULL,
        };
        // A line break may precede `with`: a `with` statement cannot follow in module code.
        if self.is(TokenKind::With) {
            self.next()?;
            self.expect(TokenKind::LBrace, "{")?;
            let ty = self.heap.atom_str("type")?;
            let mut keys: Vec<Atom> = Vec::new();
            while !self.is(TokenKind::RBrace) {
                let pos = self.tok.start;
                let key = if self.is(TokenKind::String) {
                    let units = self.cooked_string();
                    let a = self.atom_of_units(&units)?;
                    self.next()?;
                    a
                } else if self.tok.kind.is_identifier_name() {
                    let a = self.ident_atom()?;
                    self.next()?;
                    a
                } else {
                    return syntax(pos, "identifier expected");
                };
                if keys.contains(&key) {
                    return syntax(pos, "duplicate import attribute");
                }
                keys.push(key);
                self.expect(TokenKind::Colon, ":")?;
                if !self.is(TokenKind::String) {
                    return syntax(self.tok.start, "expecting string");
                }
                let units = self.cooked_string();
                let value = self.atom_of_units(&units)?;
                self.next()?;
                if key == ty {
                    req.ty = value;
                } else if req.unsupported.is_null() {
                    req.unsupported = key;
                }
                if !self.eat(TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(TokenKind::RBrace, "}")?;
        }
        self.expect_semicolon()?;
        Ok(req)
    }

    /// `import type X from` / `import type { .. }` (not the default import `type`).
    fn import_type_modifier(&self) -> bool {
        let mut lx = self.lx;
        let n = lx.next_token().ok();
        let n2 = lx.next_token().ok();
        match n {
            Some(n) if matches!(n.kind, TokenKind::LBrace | TokenKind::Star) => true,
            Some(n) if n.is_contextual(Kw::From) => n2.is_some_and(|t| t.is_contextual(Kw::From)),
            Some(n) => n.kind == TokenKind::Ident,
            None => false,
        }
    }

    /// A `type` modifier on an import or export specifier (`{ type T }`, `{ type as }`).
    fn specifier_type_modifier(&self) -> bool {
        self.ts && self.tok.is_contextual(Kw::Type) && {
            let mut lx = self.lx;
            let n = lx.next_token().ok();
            let n2 = lx.next_token().ok();
            match n {
                Some(n) if n.is_contextual(Kw::As) => {
                    n2.is_some_and(|t| matches!(t.kind, TokenKind::Comma | TokenKind::RBrace))
                }
                Some(n) => n.kind.is_identifier_name() || n.kind == TokenKind::String,
                None => false,
            }
        }
    }

    /// ImportDeclaration (16.2.2) at `import`.
    fn module_import(&mut self) -> PResult<()> {
        self.next()?;
        if self.is(TokenKind::String) {
            let spec = self.module_specifier()?;
            self.module_request(spec, true);
            return Ok(());
        }
        // TypeScript's `import type ...`: parsed and dropped.
        let record = !(self.ts && self.tok.is_contextual(Kw::Type) && self.import_type_modifier());
        if !record {
            self.next()?;
        }
        if self.ts && self.is(TokenKind::Ident) && self.peek().kind == TokenKind::Assign {
            return syntax(self.tok.start, "'import =' is not supported");
        }
        let default = self.mb().atom_default;
        // (import name or NULL for the namespace, local name, position)
        let mut binds: Vec<(Atom, Atom, u32)> = Vec::new();
        let mut more = true;
        if self.is(TokenKind::Ident) {
            let pos = self.tok.start;
            let local = self.binding_name()?;
            self.next()?;
            binds.push((default, local, pos));
            more = self.eat(TokenKind::Comma)?;
        }
        if more {
            if self.is(TokenKind::Star) {
                self.next()?;
                if !self.tok.is_contextual(Kw::As) {
                    return syntax(self.tok.start, "expecting 'as'");
                }
                self.next()?;
                let pos = self.tok.start;
                let local = self.binding_name()?;
                self.next()?;
                binds.push((Atom::NULL, local, pos));
            } else if self.is(TokenKind::LBrace) {
                self.next()?;
                while !self.is(TokenKind::RBrace) {
                    let skip = self.specifier_type_modifier();
                    if skip {
                        self.next()?;
                    }
                    let pos = self.tok.start;
                    let local = if self.peek().is_contextual(Kw::As) || self.is(TokenKind::String) {
                        let (name, _) = self.module_export_name()?;
                        if !self.tok.is_contextual(Kw::As) {
                            return syntax(self.tok.start, "expecting 'as'");
                        }
                        self.next()?;
                        let pos = self.tok.start;
                        let local = self.binding_name()?;
                        self.next()?;
                        (name, local, pos)
                    } else {
                        let local = self.binding_name()?;
                        self.next()?;
                        (local, local, pos)
                    };
                    if !skip {
                        binds.push(local);
                    }
                    if !self.eat(TokenKind::Comma)? {
                        break;
                    }
                }
                self.expect(TokenKind::RBrace, "}")?;
            } else {
                return syntax(self.tok.start, "expecting import clause");
            }
        }
        let spec = self.module_from()?;
        if !record {
            return Ok(());
        }
        // A TypeScript import keeps its request only if a binding is used (finish_module).
        let req = self.module_request(spec, !self.ts);
        for (name, local, pos) in binds {
            let v = self.declare_lexical(local, VarKind::Import, pos)?;
            if name.is_null() {
                // A namespace is there from linking on: never in TDZ.
                self.f.vars[v as usize].init_end = 0;
            }
            self.mb().imports.push((v, req, name));
        }
        Ok(())
    }

    /// ExportDeclaration (16.2.3) at `export`.
    fn module_export(&mut self) -> PResult<()> {
        let start = self.tok.start;
        self.next()?;
        let tok = self.tok;
        let type_only = self.ts
            && tok.is_contextual(Kw::Type)
            && matches!(self.peek().kind, TokenKind::LBrace | TokenKind::Star);
        if type_only {
            // `export type { T } [from "m"]`, `export type * from "m"`: dropped.
            self.next()?;
        }
        match self.tok.kind {
            TokenKind::Star => {
                self.next()?;
                let name = if self.tok.is_contextual(Kw::As) {
                    self.next()?;
                    let pos = self.tok.start;
                    Some((self.module_export_name()?.0, pos))
                } else {
                    None
                };
                let spec = self.module_from()?;
                if type_only {
                    return Ok(());
                }
                let req = self.module_request(spec, true);
                match name {
                    Some((name, pos)) => self.mb().exports.push(ExportEntry {
                        name,
                        local: Atom::NULL,
                        request: req,
                        import: Atom::NULL,
                        pos,
                    }),
                    None => self.mb().stars.push(req),
                }
                Ok(())
            }
            TokenKind::LBrace => {
                self.next()?;
                // (local or import name, was a string or a reserved word, export name, pos)
                let mut specs: Vec<(Atom, bool, Atom, u32)> = Vec::new();
                while !self.is(TokenKind::RBrace) {
                    let skip = self.specifier_type_modifier();
                    if skip {
                        self.next()?;
                    }
                    let pos = self.tok.start;
                    let t = self.tok;
                    let reserved = t.kind != TokenKind::Ident
                        || t.is_escaped_reserved()
                        || t.kw.is_strict_reserved()
                        || t.kw == Kw::Await;
                    let (local, string) = self.module_export_name()?;
                    let name = if self.tok.is_contextual(Kw::As) {
                        self.next()?;
                        self.module_export_name()?.0
                    } else {
                        local
                    };
                    if !skip {
                        specs.push((local, string || reserved, name, pos));
                    }
                    if !self.eat(TokenKind::Comma)? {
                        break;
                    }
                }
                self.expect(TokenKind::RBrace, "}")?;
                if self.tok.is_contextual(Kw::From) {
                    let spec = self.module_from()?;
                    if type_only {
                        return Ok(());
                    }
                    let req = self.module_request(spec, true);
                    for (import, _, name, pos) in specs {
                        self.mb().exports.push(ExportEntry {
                            name,
                            local: Atom::NULL,
                            request: req,
                            import,
                            pos,
                        });
                    }
                    return Ok(());
                }
                self.expect_semicolon()?;
                if type_only {
                    return Ok(());
                }
                for (local, bad, name, pos) in specs {
                    if bad {
                        return syntax(pos, "identifier expected");
                    }
                    self.mb().exports.push(ExportEntry {
                        name,
                        local,
                        request: NO_REQUEST,
                        import: Atom::NULL,
                        pos,
                    });
                }
                Ok(())
            }
            TokenKind::Default => {
                self.next()?;
                self.module_export_default(start)
            }
            TokenKind::Assign if self.ts => syntax(tok.start, "'export =' is not supported"),
            TokenKind::Import if self.ts => syntax(tok.start, "'export import' is not supported"),
            _ => {
                let declaration = match self.tok.kind {
                    TokenKind::Var
                    | TokenKind::Const
                    | TokenKind::Function
                    | TokenKind::Class
                    | TokenKind::At => true,
                    TokenKind::Enum => self.ts,
                    TokenKind::Ident => {
                        self.tok.is_contextual(Kw::Let)
                            || self.at_async_function()
                            || (self.ts
                                && matches!(
                                    self.tok.kw,
                                    Kw::Type
                                        | Kw::Interface
                                        | Kw::Declare
                                        | Kw::Namespace
                                        | Kw::Module
                                        | Kw::Abstract
                                ))
                    }
                    _ => false,
                };
                if !declaration {
                    return self.unexpected();
                }
                let pos = self.tok.start;
                self.export_declaration(pos)
            }
        }
    }

    /// `export <declaration>`: every top-level name it binds is a local export. Returns
    /// the names.
    fn export_declaration(&mut self, pos: u32) -> PResult<()> {
        let outer = self.mb().collect.replace(Vec::new());
        let r = self.parse_statement_list_item();
        let names = core::mem::replace(&mut self.mb().collect, outer).unwrap_or_default();
        r?;
        let _ = pos;
        for (name, pos) in names {
            self.mb().exports.push(ExportEntry {
                name,
                local: name,
                request: NO_REQUEST,
                import: Atom::NULL,
                pos,
            });
        }
        Ok(())
    }

    /// The name a function or class declaration at the current token binds, if it has one
    /// (`function f`, `function* f`, `async function f`, `class C`).
    fn declaration_has_name(&self) -> bool {
        let mut lx = self.lx;
        let mut t = lx.next_token().ok();
        // `async` `function` [`*`] name; `function` [`*`] name; `class` name
        if self.tok.kind == TokenKind::Ident {
            t = lx.next_token().ok();
        }
        if t.is_some_and(|t| t.kind == TokenKind::Star) {
            t = lx.next_token().ok();
        }
        t.is_some_and(|t| {
            t.kind == TokenKind::Ident
                && !(self.tok.kind == TokenKind::Class
                    && (t.kw == Kw::Extends || (self.ts && t.is_contextual(Kw::Implements))))
        })
    }

    /// `export default ...` after `default`.
    fn module_export_default(&mut self, start: u32) -> PResult<()> {
        let default = self.mb().atom_default;
        let hidden = self.mb().atom_star_default;
        let tok = self.tok;
        let local = |p: &mut Self, local: Atom, pos: u32| {
            p.mb().exports.push(ExportEntry {
                name: default,
                local,
                request: NO_REQUEST,
                import: Atom::NULL,
                pos,
            });
        };
        if self.ts && tok.is_contextual(Kw::Interface) && self.peek().kind == TokenKind::Ident {
            self.ts_interface()?;
            return Ok(());
        }
        let is_fn = tok.kind == TokenKind::Function || self.at_async_function();
        if is_fn || tok.kind == TokenKind::Class {
            if self.declaration_has_name() {
                let outer = self.mb().collect.replace(Vec::new());
                let r = self.parse_statement_list_item();
                let names = core::mem::replace(&mut self.mb().collect, outer).unwrap_or_default();
                r?;
                for (name, pos) in names {
                    local(self, name, pos);
                }
                return Ok(());
            }
            if is_fn {
                // An anonymous default function declaration, hoisted like any other,
                // named "default" (16.2.3.7).
                let is_async = tok.kind != TokenKind::Function;
                if is_async {
                    self.next()?;
                }
                self.next()?;
                let is_gen = self.eat(TokenKind::Star)?;
                let child = self.parse_function_rest(
                    super::func::FuncKind::Normal,
                    default,
                    tok.start,
                    false,
                    (is_async, is_gen),
                )?;
                self.declare_function(hidden, child, tok.start)?;
                local(self, hidden, tok.start);
                return Ok(());
            }
        }
        // `export default <expression>;` and an anonymous class: the hidden lexical
        // binding, initialized when the statement runs.
        let _ = start;
        self.declare_lexical(hidden, VarKind::Let, tok.start)?;
        if tok.kind == TokenKind::Class {
            self.parse_class(false, default)?;
        } else {
            self.parse_named(default, |p| p.parse_assign(false))?;
        }
        let r = self.init_ref(hidden, tok.start);
        self.code().name_init(r);
        if tok.kind != TokenKind::Class {
            self.expect_semicolon()?;
        }
        local(self, hidden, tok.start);
        Ok(())
    }

    /// `import.meta` after `import .` (the current token `meta`): the hidden binding the
    /// linker initializes with the module's import.meta object.
    pub(super) fn import_meta(&mut self, pos: u32) -> PResult<()> {
        self.next()?;
        let atom = self.mb().atom_meta;
        if self.mb().meta.is_none() {
            let root = match self.stack.first_mut() {
                Some(s) => &mut s.f,
                None => &mut self.f,
            };
            let v = root.add_var(atom, VarKind::Import, super::func::BODY_SCOPE);
            root.vars[v as usize].init_end = 0;
            self.mb().meta = Some(v);
        }
        let r = self.name_ref(atom, pos);
        self.code().name_get(r);
        Ok(())
    }

    /// Parses module source: the module template and its metadata (a U32Array,
    /// compiler/module_meta.rs).
    /// Compiles the module, as an async body if `is_async` (top-level await). The error's
    /// flag: compiled as sync, the module met an `await` operator; compile it as async.
    pub fn compile_module(mut self, is_async: bool) -> Result<(Ref, Ref), (super::PErr, bool)> {
        if is_async {
            self.start_flavor((true, false)).map_err(|e| (e, false))?;
        }
        let parsed = self.parse_top();
        if !is_async && self.module_await.get() {
            return Err((
                parsed
                    .err()
                    .unwrap_or_else(|| super::resolve::internal("await")),
                true,
            ));
        }
        self.compile_module_parsed(parsed).map_err(|e| (e, false))
    }

    fn compile_module_parsed(mut self, parsed: PResult<()>) -> PResult<(Ref, Ref)> {
        let m = self.module.take().expect("module code");
        parsed?;
        let meta = self.finish_module(m)?.encode();
        let arr = self.heap.alloc_u32_array(meta.len() as u32)?;
        for (i, &w) in meta.iter().enumerate() {
            self.heap.u32_array_set(arr, i as u32, w);
        }
        let (t, ..) = self.finish_unit()?;
        Ok((t, Ref(arr)))
    }

    /// The module's early errors on its entries (16.2.1.1), its environment (every
    /// top-level binding gets an entry, `Var::env`) and its metadata.
    fn finish_module(&mut self, mut m: alloc::boxed::Box<ModuleBuild>) -> PResult<ModuleMeta> {
        use super::func::{BODY_SCOPE, Capture};
        // Duplicate export names.
        for (i, e) in m.exports.iter().enumerate() {
            if m.exports[..i].iter().any(|d| d.name == e.name) {
                let name = self.heap.atom_name(e.name);
                return syntax(
                    e.pos,
                    crate::text::cat!("duplicate exported name '", &name, "'"),
                );
            }
        }
        // TypeScript: an import no value refers to names a type (its request goes too,
        // unless something else keeps it).
        if self.ts {
            let referenced = |a: Atom, p: &Self| {
                p.f.refs.iter().any(|r| r.atom == a)
                    || p.done
                        .iter()
                        .flatten()
                        .any(|f| f.refs.iter().any(|r| r.atom == a))
                    || m.exports.iter().any(|e| e.local == a)
            };
            let mut kept = Vec::new();
            for &(v, req, name) in &m.imports {
                if referenced(self.f.vars[v as usize].name, self) {
                    m.requests[req as usize].1 = true;
                    kept.push((v, req, name));
                }
            }
            m.imports = kept;
        }
        // Local exports of imported bindings re-export them (ParseModule step 10), a
        // namespace import too: `import * as ns from "m"; export { ns }` resolves to m's
        // namespace like `export * as ns from "m"`, so two modules that both re-export it
        // under one name through `export *` are not ambiguous.
        let mut local = Vec::new();
        let mut indirect = Vec::new();
        for e in &m.exports {
            if e.local.is_null() {
                indirect.push((e.name, e.request, e.import));
                continue;
            }
            let Some(v) = self.f.find_in_scope(BODY_SCOPE, e.local) else {
                if self.ts {
                    // `export { T }` of a type.
                    continue;
                }
                let name = self.heap.atom_name(e.local);
                return syntax(
                    e.pos,
                    crate::text::cat!("exported variable '", &name, "' does not exist"),
                );
            };
            match m.imports.iter().find(|i| i.0 == v) {
                Some(&(_, req, import)) => indirect.push((e.name, req, import)),
                None => local.push((e.name, v)),
            }
        }
        // Requests that nothing keeps (TypeScript type imports) are dropped.
        let mut remap = alloc::vec![u32::MAX; m.requests.len()];
        let mut requests = Vec::new();
        for (i, &(spec, keep)) in m.requests.iter().enumerate() {
            if keep {
                remap[i] = requests.len() as u32;
                requests.push(spec);
            }
        }
        // The environment: the top-level bindings in declaration order.
        let funcs = self.f.scopes[BODY_SCOPE as usize].funcs.clone();
        let mut env = Vec::new();
        let dropped = |v: VarId, m: &ModuleBuild| {
            self.f.vars[v as usize].kind == VarKind::Import
                && m.meta != Some(v)
                && !m.imports.iter().any(|i| i.0 == v)
        };
        let mut names = Vec::new();
        let mut env_of = alloc::vec![u32::MAX; self.f.vars.len()];
        for (i, var) in self.f.vars.iter().enumerate() {
            let kind = match var.kind {
                _ if var.scope != BODY_SCOPE => continue,
                VarKind::Var => (ENV_VAR, 0),
                VarKind::Let | VarKind::Const => (ENV_LEXICAL, 0),
                VarKind::Func => match funcs.iter().rev().find(|f| f.0 == i as VarId) {
                    Some(&(_, child)) => (ENV_FUNCTION, child),
                    None => (ENV_VAR, 0),
                },
                VarKind::Import if dropped(i as VarId, &m) => continue,
                VarKind::Import if m.meta == Some(i as VarId) => (ENV_META, 0),
                VarKind::Import => (ENV_IMPORT, 0),
                _ => continue,
            };
            env_of[i] = env.len() as u32;
            env.push((var.name, kind.0, kind.1));
            names.push(var.name);
        }
        if env.len() >= u16::MAX as usize {
            return syntax(0, "too many module bindings");
        }
        for (i, &e) in env_of.iter().enumerate() {
            if e != u32::MAX {
                self.f.vars[i].param = e as u16 + 1;
            }
        }
        self.f.captures = (0..env.len() as u16).map(Capture::Outer).collect();
        self.f.capture_names = names;
        Ok(ModuleMeta {
            env,
            requests,
            imports: m
                .imports
                .iter()
                .map(|&(v, req, name)| (env_of[v as usize], remap[req as usize], name))
                .collect(),
            local: local
                .iter()
                .map(|&(n, v)| (n, env_of[v as usize]))
                .collect(),
            indirect: indirect
                .iter()
                .map(|&(n, req, i)| (n, remap[req as usize], i))
                .collect(),
            star: m.stars.iter().map(|&r| remap[r as usize]).collect(),
        })
    }
}
