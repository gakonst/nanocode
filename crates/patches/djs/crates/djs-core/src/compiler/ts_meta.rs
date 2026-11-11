//! `emitDecoratorMetadata` (0065, docs/compiler.md 7.11): the runtime value of a type
//! annotation, as tsc's `serializeTypeNode()`, after qjs's `ts_meta_*`.
//!
//! An annotation is read speculatively (the type skipper consumes it later) into a
//! [`TsMeta`]: a primitive's constructor, `undefined` (`void`, `null`, `undefined`,
//! `never`), `Object` (anything tsc cannot serialize), or a type reference `A.B`, which
//! is emitted as `typeof (v = typeof A !== "undefined" && A.B) === "function" ? v :
//! Object`. Unions and intersections fold their constituents
//! (`serializeUnionOrIntersectionConstituents()` with `strictNullChecks`).
use crate::PushOut;

use alloc::vec::Vec;

use super::PResult;
use super::parser::Parser;
use super::ts::{Level, T_NO_COND, T_RETURN};
use super::ts_decl::{DeclKind, EnumVal};
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MetaKind {
    Void,
    Object,
    Function,
    Array,
    Boolean,
    String,
    Number,
    BigInt,
    Symbol,
    /// A type reference (`names`).
    Ref,
    /// Only while folding unions and intersections.
    Never,
    Unknown,
    Any,
}

/// Qualified names longer than this serialize as `Object` (as qjs).
const MAX_NAMES: usize = 4;

/// A serialized type.
#[derive(Clone, Debug)]
pub(super) struct TsMeta {
    pub kind: MetaKind,
    /// `Array`: the element kind of `T[]` (a rest parameter's type).
    pub elem: MetaKind,
    /// `Ref` (or an `Array` of `Ref`): the qualified name.
    pub names: Vec<Atom>,
}

impl TsMeta {
    pub fn new(kind: MetaKind) -> TsMeta {
        TsMeta {
            kind,
            elem: MetaKind::Object,
            names: Vec::new(),
        }
    }

    fn same(&self, o: &TsMeta) -> bool {
        self.kind == o.kind && (self.kind != MetaKind::Ref || self.names == o.names)
    }

    /// The element type of a rest parameter of this type: `T` for `T[]`.
    pub fn rest_elem(mut self) -> TsMeta {
        if self.kind == MetaKind::Array {
            self.kind = self.elem;
            self
        } else {
            TsMeta::new(MetaKind::Object)
        }
    }
}

/// Folds the constituents of a union or intersection.
struct Fold {
    done: bool,
    ty: Option<TsMeta>,
}

impl Fold {
    fn new() -> Fold {
        Fold {
            done: false,
            ty: None,
        }
    }

    fn add(&mut self, item: TsMeta, intersection: bool) {
        if self.done {
            return;
        }
        let result = match item.kind {
            MetaKind::Never => intersection.then_some(MetaKind::Void),
            MetaKind::Unknown => (!intersection).then_some(MetaKind::Object),
            MetaKind::Any | MetaKind::Object => Some(MetaKind::Object),
            _ => match &self.ty {
                None => {
                    self.ty = Some(item);
                    return;
                }
                Some(t) => (!t.same(&item)).then_some(MetaKind::Object),
            },
        };
        if let Some(k) = result {
            self.ty = Some(TsMeta::new(k));
            self.done = true;
        }
    }

    fn result(self) -> TsMeta {
        self.ty.unwrap_or(TsMeta::new(MetaKind::Void))
    }
}

impl Parser<'_, '_> {
    /// The serialized type of the `: T` annotation at the current token (`Object` if
    /// there is none or it cannot be read), without consuming it.
    pub(super) fn ts_meta_annotation(&mut self) -> TsMeta {
        let mut m = TsMeta::new(MetaKind::Object);
        if self.is(TokenKind::Colon) {
            self.ts_lookahead(|p| {
                p.next()?;
                m = p.ts_meta_union()?;
                Ok(true)
            });
        }
        m
    }

    /// The annotation of the destructuring parameter at the current token.
    pub(super) fn ts_meta_pattern_annotation(&mut self) -> TsMeta {
        let mut m = TsMeta::new(MetaKind::Object);
        self.ts_lookahead(|p| {
            p.ts_skip_balanced()?;
            if p.is(TokenKind::Question) {
                p.next()?;
            }
            m = p.ts_meta_annotation();
            Ok(true)
        });
        m
    }

    /// A type with unions and conditional types (their branches fold).
    fn ts_meta_union(&mut self) -> PResult<TsMeta> {
        self.enter()?;
        let r = self.ts_meta_union_inner();
        self.leave();
        r
    }

    fn ts_meta_union_inner(&mut self) -> PResult<TsMeta> {
        let m = self.ts_meta_list(false)?;
        if !(self.is(TokenKind::Extends) && !self.tok.newline_before()) {
            return Ok(m);
        }
        // A conditional type: the union of the two branches.
        self.next()?;
        self.ts_skip_type2(Level::Lowest, T_NO_COND)?;
        self.ts_expect(TokenKind::Question)?;
        let mut f = Fold::new();
        let t = self.ts_meta_union()?;
        f.add(t, false);
        self.ts_expect(TokenKind::Colon)?;
        let t = self.ts_meta_union()?;
        f.add(t, false);
        Ok(f.result())
    }

    /// A union (`intersection` false: of intersections) or an intersection (of
    /// postfix types).
    fn ts_meta_list(&mut self, intersection: bool) -> PResult<TsMeta> {
        let sep = if intersection {
            TokenKind::Amp
        } else {
            TokenKind::Pipe
        };
        if self.is(sep) {
            self.next()?;
        }
        let mut f = Fold::new();
        let mut first = true;
        loop {
            let item = if intersection {
                self.ts_meta_postfix()?
            } else {
                self.ts_meta_list(true)?
            };
            if first && !self.is(sep) {
                return Ok(item);
            }
            first = false;
            f.add(item, intersection);
            if !self.is(sep) {
                break;
            }
            self.next()?;
        }
        Ok(f.result())
    }

    /// A primary type with the `T[]`, `T[K]` and `T!` suffixes.
    fn ts_meta_postfix(&mut self) -> PResult<TsMeta> {
        let mut m = self.ts_meta_primary()?;
        loop {
            if self.is(TokenKind::LBracket) && !self.tok.newline_before() {
                self.next()?;
                if self.is(TokenKind::RBracket) {
                    // An array type keeps its element type.
                    m.elem = m.kind;
                    if m.kind != MetaKind::Ref {
                        m.names.clear();
                    }
                    m.kind = MetaKind::Array;
                } else {
                    // An indexed access type.
                    m = TsMeta::new(MetaKind::Object);
                    self.ts_skip_type()?;
                }
                self.ts_expect(TokenKind::RBracket)?;
            } else if self.is(TokenKind::Bang) && !self.tok.newline_before() {
                self.next()?;
            } else {
                return Ok(m);
            }
        }
    }

    /// A primary type, as tsc's `serializeTypeNode()`.
    fn ts_meta_primary(&mut self) -> PResult<TsMeta> {
        use MetaKind as K;
        let kind = match self.tok.kind {
            TokenKind::Number => K::Number,
            TokenKind::BigInt => K::BigInt,
            TokenKind::Minus => {
                self.next()?;
                match self.tok.kind {
                    TokenKind::Number => K::Number,
                    TokenKind::BigInt => K::BigInt,
                    _ => return self.ts_error(),
                }
            }
            TokenKind::String => K::String,
            TokenKind::NoSubstTemplate | TokenKind::TemplateHead => {
                self.ts_skip_type2(Level::Prefix, T_NO_COND)?;
                return Ok(TsMeta::new(K::String));
            }
            TokenKind::True | TokenKind::False => K::Boolean,
            TokenKind::Null | TokenKind::Void => K::Void,
            TokenKind::This => {
                self.next()?;
                if self.ts_at(Kw::Is) && !self.tok.newline_before() {
                    self.next()?;
                    self.ts_skip_type()?;
                    return Ok(TsMeta::new(K::Boolean));
                }
                return Ok(TsMeta::new(K::Object));
            }
            TokenKind::Typeof | TokenKind::Import => {
                self.ts_skip_type2(Level::Prefix, T_NO_COND)?;
                return Ok(TsMeta::new(K::Object));
            }
            TokenKind::New | TokenKind::Lt => {
                self.ts_skip_type2(Level::Prefix, 0)?;
                return Ok(TsMeta::new(K::Function));
            }
            TokenKind::LParen => {
                if self.ts_try_skip_arrow_args() {
                    self.ts_skip_return_type()?;
                    return Ok(TsMeta::new(K::Function));
                }
                self.next()?;
                let m = self.ts_meta_union()?;
                self.ts_expect(TokenKind::RParen)?;
                return Ok(m);
            }
            TokenKind::LBracket => {
                self.ts_skip_balanced()?;
                return Ok(TsMeta::new(K::Array));
            }
            TokenKind::LBrace => {
                self.ts_skip_object_type()?;
                return Ok(TsMeta::new(K::Object));
            }
            TokenKind::Ident => match self.lx.text(&self.tok) {
                b"any" => K::Any,
                b"unknown" => K::Unknown,
                b"never" => K::Never,
                b"undefined" => K::Void,
                b"object" => K::Object,
                b"number" => K::Number,
                b"string" => K::String,
                b"boolean" => K::Boolean,
                b"bigint" => K::BigInt,
                b"symbol" => K::Symbol,
                b"readonly" => {
                    self.next()?;
                    return self.ts_meta_union();
                }
                b"keyof" | b"unique" | b"infer" => {
                    self.ts_skip_type2(Level::Prefix, T_NO_COND)?;
                    return Ok(TsMeta::new(K::Object));
                }
                b"asserts" => {
                    self.ts_skip_type2(Level::Lowest, T_RETURN)?;
                    return Ok(TsMeta::new(K::Boolean));
                }
                b"abstract" => {
                    self.ts_skip_type2(Level::Prefix, 0)?;
                    return Ok(TsMeta::new(K::Function));
                }
                _ => return self.ts_meta_name(),
            },
            _ => return self.ts_error(),
        };
        self.next()?;
        Ok(TsMeta::new(kind))
    }

    /// A qualified name, possibly with type arguments, or a type predicate `x is T`.
    fn ts_meta_name(&mut self) -> PResult<TsMeta> {
        let mut m = TsMeta::new(MetaKind::Ref);
        let mut overflow = false;
        loop {
            if m.names.len() < MAX_NAMES {
                let a = self.ident_atom()?;
                m.names.push_out(a);
            } else {
                overflow = true;
            }
            self.next()?;
            if !self.is(TokenKind::Dot) {
                break;
            }
            self.next()?;
            if !self.tok.kind.is_identifier_name() {
                return self.ts_error();
            }
        }
        if !self.tok.newline_before() {
            self.ts_skip_type_args(false)?;
        }
        if self.ts_at(Kw::Is) && !self.tok.newline_before() {
            self.next()?;
            self.ts_skip_type()?;
            return Ok(TsMeta::new(MetaKind::Boolean));
        }
        if overflow {
            return Ok(TsMeta::new(MetaKind::Object));
        }
        Ok(m)
    }

    /// The kind of an enum declared before (tsc serializes it as `Number`, `String`
    /// or, mixed, `Object`), `None` if `name` is not one.
    fn ts_meta_enum_kind(&self, name: Atom) -> Option<MetaKind> {
        let d = &self.ts_decls[self.ts_lookup_decl(name)? as usize];
        if !matches!(d.kind, DeclKind::Enum | DeclKind::ConstEnum) {
            return None;
        }
        let has_str = d
            .members
            .iter()
            .any(|m| matches!(m.value, Some(EnumVal::Str(_))));
        let has_num = d
            .members
            .iter()
            .any(|m| !matches!(m.value, Some(EnumVal::Str(_))));
        Some(match (has_str, has_num) {
            (true, true) => MetaKind::Object,
            (true, false) => MetaKind::String,
            _ => MetaKind::Number,
        })
    }

    /// Pushes the runtime value of `m`.
    pub(super) fn emit_ts_meta(&mut self, m: &TsMeta, pos: u32) -> PResult<()> {
        use MetaKind as K;
        let kind = match m.kind {
            K::Void | K::Never => {
                self.code().undefined();
                return Ok(());
            }
            K::BigInt | K::Symbol => {
                let name = if m.kind == K::BigInt {
                    "BigInt"
                } else {
                    "Symbol"
                };
                let a = self.heap.atom_str(name)?;
                return self.emit_ts_meta_ref(&[a], pos);
            }
            K::Ref => {
                let e = match m.names.as_slice() {
                    [a] => self.ts_meta_enum_kind(*a),
                    _ => None,
                };
                match e {
                    Some(k) => k,
                    None => return self.emit_ts_meta_ref(&m.names, pos),
                }
            }
            k => k,
        };
        let name = match kind {
            K::Function => "Function",
            K::Array => "Array",
            K::Boolean => "Boolean",
            K::String => "String",
            K::Number => "Number",
            _ => "Object",
        };
        let a = self.heap.atom_str(name)?;
        let r = self.name_ref(a, pos);
        self.code().name_get(r);
        Ok(())
    }

    /// `typeof (v = typeof A !== "undefined" && A.B) === "function" ? v : Object`.
    fn emit_ts_meta_ref(&mut self, names: &[Atom], pos: u32) -> PResult<()> {
        let obj = self.new_label();
        let done = self.new_label();
        let r = self.name_ref(names[0], pos);
        self.mark(pos);
        self.code().name_typeof(r);
        for &a in &names[1..] {
            self.code().dup();
            self.code().is_nullish();
            self.code().jump_if_true(obj);
            self.mark(pos);
            self.code().get_field(a, 0);
        }
        self.code().dup();
        self.code().typeof_();
        let f = self.heap.atom_str("function")?;
        self.code().push_atom(f);
        self.code().strict_eq();
        self.code().jump_if_true(done);
        self.place(obj);
        self.code().drop();
        let a = self.heap.atom_str("Object")?;
        let r = self.name_ref(a, pos);
        self.code().name_get(r);
        self.place(done);
        Ok(())
    }
}
