//! TypeScript (0048, docs/compiler.md 3.9 and 7.10): the type skipper.
//!
//! Types are erased while parsing: the skipper consumes them token by token and never
//! emits. The rules follow dquickts (whose disambiguation follows esbuild's and tsc's
//! TypeScript parsers), so djs accepts and rejects the same programs. Speculation (a
//! generic call `f<T>(x)` against `f < T > (x)`, arrows with return types, ...) only
//! moves the lexer: a checkpoint and a rewind (D1080). Errors during speculation mean
//! "not this" and cost no message.
//!
//! Every erased range is recorded (when `Parser::erased` is on) by the code that commits
//! to it, never during speculation, for `djs strip-types` (the parse-only oracle against
//! ts-blank-space, TESTING.md).
use crate::PushOut;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use super::parser::{Parser, syntax};
use super::{PErr, PResult};
use crate::lexer::{Kw, Token, TokenKind};

/// Precedence levels of the type grammar (`|` binds loosest).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Level {
    Lowest,
    Or,
    And,
    Prefix,
}

/// Type flags: a return type (`asserts x`), an index signature or mapped type key, a
/// tuple element (labels), no conditional type (`extends` in a constraint).
pub(super) const T_RETURN: u8 = 1;
const T_INDEX_SIG: u8 = 2;
const T_TUPLE_LABELS: u8 = 4;
pub(super) const T_NO_COND: u8 = 8;

/// Type parameter list flags: an empty list `<>` is allowed (type aliases,
/// interfaces); the others document which modifiers tsc allows where.
pub(super) const TP_IN_OUT: u8 = 1;
pub(super) const TP_CONST: u8 = 2;
pub(super) const TP_EMPTY: u8 = 4;

/// What a type parameter list looked like (`<T>` alone could be a type assertion in
/// some positions; `<T extends U>`, `<T,>`, `<const T>` cannot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TypeParams {
    None,
    MaybeCast,
    Definite,
}

/// Token skipping flags: stop after the first closed group, at a `,`, at a line break.
const SKIP_BALANCED: u8 = 1;
const SKIP_COMMA: u8 = 2;
const SKIP_LF: u8 = 4;

/// Primitive type names: `number<T>` is never a generic, so `x as number < y` compares.
fn is_primitive_type(name: &[u8]) -> bool {
    matches!(
        name,
        b"any"
            | b"never"
            | b"unknown"
            | b"undefined"
            | b"object"
            | b"number"
            | b"string"
            | b"boolean"
            | b"bigint"
            | b"symbol"
    )
}

/// How a type that starts with an identifier continues.
enum Named {
    /// With the postfix and binary type operators.
    Suffix,
    /// Complete (a type predicate).
    Done,
    /// Parse the current token as the start of the type again (`abstract new`).
    Redo,
}

/// `<` and the tokens that start with one.
pub(super) fn is_lt(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Lt | TokenKind::Shl | TokenKind::Le | TokenKind::ShlAssign
    )
}

impl<'a> Parser<'a, '_> {
    // ---- Errors, speculation, erasure ------------------------------------------------

    pub(super) fn ts_error<T>(&mut self) -> PResult<T> {
        Err(self.ts_error_value())
    }

    /// The error of `ts_error`, out of line (0092: one copy, not one per result type).
    #[cold]
    #[inline(never)]
    fn ts_error_value(&self) -> PErr {
        if self.spec > 0 {
            return PErr::syntax(self.tok.start, Cow::Borrowed(""));
        }
        if self.tok.kind == TokenKind::Eof {
            return PErr::syntax(
                self.tok.start,
                Cow::Borrowed("unexpected end of input in type"),
            );
        }
        let text = String::from_utf8_lossy(self.lx.text(&self.tok)).into_owned();
        PErr::syntax(
            self.tok.start,
            Cow::Owned(crate::text::cat!("unexpected token in type: '", text, "'")),
        )
    }

    pub(super) fn ts_expect(&mut self, kind: TokenKind) -> PResult<()> {
        if self.tok.kind != kind {
            return self.ts_error();
        }
        self.next()
    }

    /// Runs `f` speculatively: keeps the position if it returns `Ok(true)`, otherwise
    /// rewinds. Returns whether it was kept.
    pub(super) fn ts_try(&mut self, f: impl FnOnce(&mut Self) -> PResult<bool>) -> bool {
        let cp = self.checkpoint();
        self.spec += 1;
        let depth = self.depth;
        let ok = matches!(f(self), Ok(true));
        self.depth = depth;
        self.spec -= 1;
        if !ok {
            self.rewind(cp);
        }
        ok
    }

    /// Runs `f` as pure lookahead: always rewinds. Returns its verdict.
    pub(super) fn ts_lookahead(&mut self, f: impl FnOnce(&mut Self) -> PResult<bool>) -> bool {
        let cp = self.checkpoint();
        self.spec += 1;
        let depth = self.depth;
        let ok = matches!(f(self), Ok(true));
        self.depth = depth;
        self.spec -= 1;
        self.rewind(cp);
        ok
    }

    /// Records the erased range from `start` to the end of the last token.
    pub(super) fn ts_erase(&mut self, start: u32) {
        self.ts_erase_kind(start, self.prev_end, ER_BLANK);
    }

    /// Records an erased statement (a type-only declaration) from `start`:
    /// ts-blank-space writes a `;` in its place when the JS before it did not end with
    /// one (`Parser::ts_semi`), so the code around it cannot join.
    pub(super) fn ts_erase_stmt(&mut self, start: u32) {
        let kind = if self.ts_semi { ER_SEMI } else { ER_BLANK };
        self.ts_erase_kind(start, self.prev_end, kind);
    }

    pub(super) fn ts_erase_kind(&mut self, start: u32, end: u32, kind: u8) {
        if self.spec == 0 && start < end {
            self.syn_erased(start, end);
            if let Some(e) = &mut self.erased {
                e.push_out((start, end, kind));
            }
        }
    }

    /// A statement ended without a `;` (ASI): an `as`/`satisfies` that ends it is
    /// replaced by `;` + blanks (ts-blank-space), so the next line cannot continue it.
    pub(super) fn ts_stmt_end_asi(&mut self) {
        let end = self.prev_end;
        let next_semi = self.src.get(end as usize) == Some(&b';');
        if let Some(e) = &mut self.erased
            && let Some(last) = e.last_mut()
            && last.2 == ER_AS
            && last.1 == end
            && !next_semi
        {
            last.2 = ER_SEMI;
        }
    }

    /// A JS statement (or class member) ended: does what follows need a `;` before an
    /// erased statement? (ts-blank-space's `semicolonNeeded`.)
    pub(super) fn ts_note_stmt_end(&mut self) {
        if self.prev_end > 0 {
            self.ts_semi = self.src[self.prev_end as usize - 1] != b';';
        }
    }

    /// The current token is the unescaped contextual keyword `kw`.
    pub(super) fn ts_at(&self, kw: Kw) -> bool {
        self.tok.is_contextual(kw)
    }

    // ---- Token skipping ----------------------------------------------------------------

    /// Skips tokens without emitting. Stops at depth 0 before a closing bracket, `;`,
    /// the end, a `:` not paired with a `?`, or as `flags` say. Regexes and templates
    /// are recognized with the scans' previous-token heuristic.
    fn ts_skip_tokens(&mut self, flags: u8) -> PResult<()> {
        let mut stack: Vec<u8> = Vec::new();
        let mut nq = 0u32;
        let mut first = true;
        let mut prev: Option<Token> = None;
        loop {
            if !first && stack.is_empty() && flags & SKIP_LF != 0 && self.tok.newline_before() {
                return Ok(());
            }
            first = false;
            match self.tok.kind {
                TokenKind::LParen => stack.push_out(b'('),
                TokenKind::LBracket => stack.push_out(b'['),
                TokenKind::LBrace => stack.push_out(b'{'),
                TokenKind::TemplateHead => stack.push_out(b'`'),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    let Some(c) = stack.pop() else {
                        return Ok(());
                    };
                    if c == b'`' {
                        if self.tok.kind != TokenKind::RBrace {
                            return self.ts_error();
                        }
                        self.tok = self.lx.rescan_template_continuation(self.tok)?;
                        if self.tok.kind == TokenKind::TemplateMiddle {
                            stack.push_out(b'`');
                        }
                    } else {
                        let open = match self.tok.kind {
                            TokenKind::RParen => b'(',
                            TokenKind::RBracket => b'[',
                            _ => b'{',
                        };
                        if c != open {
                            return self.ts_error();
                        }
                        if stack.is_empty() && flags & SKIP_BALANCED != 0 {
                            return self.next();
                        }
                    }
                }
                TokenKind::Eof => {
                    if !stack.is_empty() {
                        return self.ts_error();
                    }
                    return Ok(());
                }
                TokenKind::Semicolon if stack.is_empty() => return Ok(()),
                TokenKind::Comma if stack.is_empty() && flags & SKIP_COMMA != 0 => {
                    return Ok(());
                }
                TokenKind::Question if stack.is_empty() => nq += 1,
                TokenKind::Colon if stack.is_empty() => {
                    if nq == 0 {
                        return Ok(());
                    }
                    nq -= 1;
                }
                TokenKind::Slash | TokenKind::SlashAssign
                    if prev.is_none_or(|p| super::scan::regex_allowed(&p)) =>
                {
                    self.tok = self.lx.rescan_as_regex(self.tok)?;
                }
                _ => {}
            }
            prev = Some(self.tok);
            self.next()?;
        }
    }

    /// Skips the balanced group at the current `(`, `[` or `{`.
    pub(super) fn ts_skip_balanced(&mut self) -> PResult<()> {
        self.ts_skip_tokens(SKIP_BALANCED)
    }

    /// Skips the rest of an expression up to a `,` (at depth 0) or what ends it (a
    /// default value, an ambient initializer).
    pub(super) fn ts_skip_expr(&mut self, stop_at_lf: bool) -> PResult<()> {
        self.ts_skip_tokens(SKIP_COMMA | if stop_at_lf { SKIP_LF } else { 0 })
    }

    /// The current token is the `}` that closes a template literal type substitution.
    fn ts_template_continue(&mut self) -> PResult<()> {
        if self.tok.kind != TokenKind::RBrace {
            return self.ts_error();
        }
        self.tok = self.lx.rescan_template_continuation(self.tok)?;
        Ok(())
    }

    /// Expects a `<`, splitting `<<` and `<=`.
    fn ts_expect_lt(&mut self) -> PResult<()> {
        if !is_lt(self.tok.kind) {
            return self.ts_error();
        }
        if self.tok.kind != TokenKind::Lt {
            self.tok = self.lx.rescan_less(self.tok);
        }
        self.next()
    }

    // ---- Types ------------------------------------------------------------------------

    pub(super) fn ts_skip_type(&mut self) -> PResult<()> {
        self.ts_skip_type2(Level::Lowest, 0)
    }

    pub(super) fn ts_skip_return_type(&mut self) -> PResult<()> {
        self.ts_skip_type2(Level::Lowest, T_RETURN)
    }

    /// A binding name or pattern in a function type.
    fn ts_skip_binding(&mut self) -> PResult<()> {
        match self.tok.kind {
            TokenKind::Ident | TokenKind::This => self.next(),
            TokenKind::LBracket | TokenKind::LBrace => self.ts_skip_balanced(),
            _ => self.ts_error(),
        }
    }

    /// The parameters of a function type or signature.
    fn ts_skip_fn_args(&mut self) -> PResult<()> {
        self.ts_expect(TokenKind::LParen)?;
        while !self.is(TokenKind::RParen) {
            if self.is(TokenKind::Ellipsis) {
                self.next()?;
            }
            self.ts_skip_binding()?;
            if self.is(TokenKind::Question) {
                self.next()?;
            }
            if self.is(TokenKind::Colon) {
                self.next()?;
                self.ts_skip_type()?;
            }
            if !self.is(TokenKind::Comma) {
                break;
            }
            self.next()?;
        }
        self.ts_expect(TokenKind::RParen)
    }

    /// A type parameter list `<T extends U = V, ...>` if one starts here.
    pub(super) fn ts_skip_type_params(&mut self, flags: u8) -> PResult<TypeParams> {
        if !self.is(TokenKind::Lt) {
            return Ok(TypeParams::None);
        }
        self.next()?;
        let mut result = TypeParams::MaybeCast;
        if flags & TP_EMPTY != 0 && self.is(TokenKind::Gt) {
            self.next()?;
            return Ok(TypeParams::Definite);
        }
        loop {
            // `in`, `out` and `const` modifiers (`out` may be the name).
            let mut expect_ident = true;
            loop {
                if self.is(TokenKind::Const) {
                    result = TypeParams::Definite;
                    expect_ident = true;
                } else if self.is(TokenKind::In) {
                    expect_ident = true;
                } else if self.ts_at(Kw::Out) {
                    expect_ident = false;
                } else {
                    break;
                }
                self.next()?;
            }
            if expect_ident || self.is(TokenKind::Ident) {
                if !self.is(TokenKind::Ident) {
                    return self.ts_error();
                }
                self.next()?;
            }
            if self.is(TokenKind::Extends) {
                result = TypeParams::Definite;
                self.next()?;
                self.ts_skip_type()?;
            }
            if self.is(TokenKind::Assign) {
                result = TypeParams::Definite;
                self.next()?;
                self.ts_skip_type()?;
            }
            if !self.is(TokenKind::Comma) {
                break;
            }
            self.next()?;
            if self.is(TokenKind::Gt) {
                result = TypeParams::Definite;
                break;
            }
        }
        self.ts_expect(TokenKind::Gt)?;
        Ok(result)
    }

    /// A type argument list if one starts here. In an expression (`in_expr`) the list
    /// must end with a `>` that is not part of `>=`, `>>`, ... (`x < y >= z` compares).
    pub(super) fn ts_skip_type_args(&mut self, in_expr: bool) -> PResult<bool> {
        if !is_lt(self.tok.kind) {
            return Ok(false);
        }
        self.ts_expect_lt()?;
        loop {
            self.ts_skip_type()?;
            if !self.is(TokenKind::Comma) {
                break;
            }
            self.next()?;
        }
        if in_expr
            && self.is(TokenKind::Gt)
            && matches!(self.src.get(self.tok.end as usize), Some(b'=' | b'>'))
        {
            return self.ts_error();
        }
        self.ts_expect(TokenKind::Gt)?;
        Ok(true)
    }

    /// The parameters of a function type followed by `=>`, if they are (else rewinds).
    pub(super) fn ts_try_skip_arrow_args(&mut self) -> bool {
        self.ts_try(|p| {
            p.ts_skip_fn_args()?;
            if !p.is(TokenKind::Arrow) {
                return Ok(false);
            }
            p.next()?;
            Ok(true)
        })
    }

    /// `(T)` or a function type `(a: T) => U`.
    fn ts_skip_paren_or_fn_type(&mut self) -> PResult<()> {
        if self.ts_try_skip_arrow_args() {
            return self.ts_skip_return_type();
        }
        self.ts_expect(TokenKind::LParen)?;
        self.ts_skip_type()?;
        self.ts_expect(TokenKind::RParen)
    }

    /// `infer T extends U`: a constraint unless a `?` follows in a conditional type.
    fn ts_try_skip_infer_constraint(&mut self, flags: u8) {
        self.ts_try(|p| {
            p.next()?;
            p.ts_skip_type2(Level::Prefix, T_NO_COND)?;
            Ok(!(flags & T_NO_COND == 0 && p.is(TokenKind::Question)))
        });
    }

    pub(super) fn ts_skip_type2(&mut self, level: Level, flags: u8) -> PResult<()> {
        self.enter()?;
        let r = self.ts_skip_type_inner(level, flags);
        self.leave();
        r
    }

    fn ts_skip_type_inner(&mut self, level: Level, flags: u8) -> PResult<()> {
        let labels = flags & T_TUPLE_LABELS != 0;
        loop {
            match self.tok.kind {
                TokenKind::Number
                | TokenKind::BigInt
                | TokenKind::String
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
                | TokenKind::Void
                | TokenKind::Const
                | TokenKind::NoSubstTemplate => self.next()?,
                TokenKind::TemplateHead => {
                    // A template literal type: `a${T}b`.
                    while matches!(
                        self.tok.kind,
                        TokenKind::TemplateHead | TokenKind::TemplateMiddle
                    ) {
                        self.next()?;
                        self.ts_skip_type()?;
                        self.ts_template_continue()?;
                    }
                    self.next()?;
                }
                TokenKind::This => {
                    self.next()?;
                    // A `this is T` type predicate.
                    if self.ts_at(Kw::Is) && !self.tok.newline_before() {
                        self.next()?;
                        return self.ts_skip_type();
                    }
                }
                TokenKind::Minus => {
                    self.next()?;
                    if !matches!(self.tok.kind, TokenKind::Number | TokenKind::BigInt) {
                        return self.ts_error();
                    }
                    self.next()?;
                }
                // A leading separator: `type A = | B | C`.
                TokenKind::Amp | TokenKind::Pipe => {
                    self.next()?;
                    continue;
                }
                TokenKind::Import => {
                    // `import("mod").T`
                    self.next()?;
                    if labels && matches!(self.tok.kind, TokenKind::Colon | TokenKind::Question) {
                        return Ok(());
                    }
                    self.ts_expect(TokenKind::LParen)?;
                    self.ts_expect(TokenKind::String)?;
                    if self.is(TokenKind::Comma) {
                        self.next()?;
                        self.ts_skip_object_type()?;
                        if self.is(TokenKind::Comma) {
                            self.next()?;
                        }
                    }
                    self.ts_expect(TokenKind::RParen)?;
                }
                TokenKind::New => {
                    // A constructor type: `new (...) => T`.
                    self.next()?;
                    if labels && matches!(self.tok.kind, TokenKind::Colon | TokenKind::Question) {
                        return Ok(());
                    }
                    self.ts_skip_type_params(TP_CONST)?;
                    self.ts_skip_paren_or_fn_type()?;
                }
                TokenKind::Lt => {
                    // A generic function type: `<T>(x: T) => T`.
                    self.ts_skip_type_params(TP_CONST)?;
                    self.ts_skip_paren_or_fn_type()?;
                }
                TokenKind::LParen => self.ts_skip_paren_or_fn_type()?,
                TokenKind::Typeof => {
                    self.next()?;
                    if labels && matches!(self.tok.kind, TokenKind::Colon | TokenKind::Question) {
                        return Ok(());
                    }
                    if self.is(TokenKind::Import) {
                        continue;
                    }
                    if !self.tok.kind.is_identifier_name() {
                        return self.ts_error();
                    }
                    self.next()?;
                    while self.is(TokenKind::Dot) {
                        self.next()?;
                        if !self.tok.kind.is_identifier_name() && !self.is(TokenKind::PrivateName) {
                            return self.ts_error();
                        }
                        self.next()?;
                    }
                    if !self.tok.newline_before() {
                        self.ts_skip_type_args(false)?;
                    }
                }
                TokenKind::LBracket => {
                    // A tuple, with labels, optional and rest elements.
                    self.next()?;
                    while !self.is(TokenKind::RBracket) {
                        if self.is(TokenKind::Ellipsis) {
                            self.next()?;
                        }
                        self.ts_skip_type2(Level::Lowest, T_TUPLE_LABELS)?;
                        if self.is(TokenKind::Question) {
                            self.next()?;
                        }
                        if self.is(TokenKind::Colon) {
                            self.next()?;
                            self.ts_skip_type()?;
                        }
                        if !self.is(TokenKind::Comma) {
                            break;
                        }
                        self.next()?;
                    }
                    self.ts_expect(TokenKind::RBracket)?;
                }
                TokenKind::LBrace => self.ts_skip_object_type()?,
                TokenKind::Ident => match self.ts_skip_named_type(flags)? {
                    Named::Suffix => {}
                    Named::Done => return Ok(()),
                    Named::Redo => continue,
                },
                k if labels && k.is_identifier_name() => {
                    // A keyword as a tuple label: `[function: T]`.
                    self.next()?;
                    if !matches!(self.tok.kind, TokenKind::Colon | TokenKind::Question) {
                        return self.ts_error();
                    }
                    return Ok(());
                }
                _ => return self.ts_error(),
            }
            break;
        }
        self.ts_skip_type_suffix(level, flags)
    }

    /// A type that starts with an identifier.
    fn ts_skip_named_type(&mut self, flags: u8) -> PResult<Named> {
        let tok = self.tok;
        let name_like = |p: &Self| {
            matches!(
                p.tok.kind,
                TokenKind::Colon | TokenKind::Question | TokenKind::In
            ) && flags & (T_INDEX_SIG | T_TUPLE_LABELS) != 0
        };
        let mut check_type_args = true;
        if tok.is_contextual(Kw::Keyof) || tok.is_contextual(Kw::Readonly) {
            self.next()?;
            // `[keyof: T]` and `{ [keyof in T]: U }` use it as a name.
            if !name_like(self) {
                self.ts_skip_type2(Level::Prefix, 0)?;
            }
            return Ok(Named::Suffix);
        }
        if tok.is_contextual(Kw::Infer) {
            self.next()?;
            if !name_like(self) {
                if !self.is(TokenKind::Ident) {
                    return self.ts_error();
                }
                self.next()?;
                if self.is(TokenKind::Extends) {
                    self.ts_try_skip_infer_constraint(flags);
                }
            }
            return Ok(Named::Suffix);
        }
        if tok.is_contextual(Kw::Unique) {
            self.next()?;
            if self.lx.text(&self.tok) == b"symbol" && self.is(TokenKind::Ident) {
                self.next()?;
                return Ok(Named::Suffix);
            }
        } else if tok.is_contextual(Kw::Abstract) {
            self.next()?;
            // An abstract constructor type.
            if self.is(TokenKind::New) {
                return Ok(Named::Redo);
            }
        } else if tok.is_contextual(Kw::Asserts) {
            self.next()?;
            // `asserts x` and `asserts x is T`.
            if flags & T_RETURN != 0
                && !self.tok.newline_before()
                && matches!(self.tok.kind, TokenKind::Ident | TokenKind::This)
            {
                self.next()?;
            }
        } else {
            check_type_args = !is_primitive_type(self.lx.text(&tok));
            self.next()?;
        }
        // An `x is T` type predicate.
        if self.ts_at(Kw::Is) && !self.tok.newline_before() {
            self.next()?;
            self.ts_skip_type()?;
            return Ok(Named::Done);
        }
        // `let x: T \n <U>y` must not become type arguments.
        if check_type_args && !self.tok.newline_before() {
            self.ts_skip_type_args(false)?;
        }
        Ok(Named::Suffix)
    }

    fn ts_skip_type_suffix(&mut self, level: Level, flags: u8) -> PResult<()> {
        loop {
            match self.tok.kind {
                TokenKind::Pipe => {
                    if level >= Level::Or {
                        return Ok(());
                    }
                    self.next()?;
                    self.ts_skip_type2(Level::Or, flags)?;
                }
                TokenKind::Amp => {
                    if level >= Level::And {
                        return Ok(());
                    }
                    self.next()?;
                    self.ts_skip_type2(Level::And, flags)?;
                }
                TokenKind::Bang => {
                    // The JSDoc non-nullable postfix, accepted like tsc.
                    if self.tok.newline_before() {
                        return Ok(());
                    }
                    self.next()?;
                }
                TokenKind::Dot => {
                    self.next()?;
                    if !self.tok.kind.is_identifier_name() {
                        return self.ts_error();
                    }
                    self.next()?;
                    if !self.tok.newline_before() {
                        self.ts_skip_type_args(false)?;
                    }
                }
                TokenKind::LBracket => {
                    // An indexed access or array type, not across lines.
                    if self.tok.newline_before() {
                        return Ok(());
                    }
                    self.next()?;
                    if !self.is(TokenKind::RBracket) {
                        self.ts_skip_type()?;
                    }
                    self.ts_expect(TokenKind::RBracket)?;
                }
                TokenKind::Extends => {
                    // A conditional type.
                    if self.tok.newline_before() || flags & T_NO_COND != 0 {
                        return Ok(());
                    }
                    self.next()?;
                    self.ts_skip_type2(Level::Lowest, T_NO_COND)?;
                    self.ts_expect(TokenKind::Question)?;
                    self.ts_skip_type()?;
                    self.ts_expect(TokenKind::Colon)?;
                    self.ts_skip_type()?;
                }
                _ => return Ok(()),
            }
        }
    }

    /// An object type, interface body or mapped type.
    pub(super) fn ts_skip_object_type(&mut self) -> PResult<()> {
        self.ts_expect(TokenKind::LBrace)?;
        while !self.is(TokenKind::RBrace) {
            // `-readonly [K in T]: U`
            if matches!(self.tok.kind, TokenKind::Plus | TokenKind::Minus) {
                self.next()?;
            }
            // Modifiers and the property name.
            let mut found_key = false;
            while self.tok.kind.is_identifier_name()
                || matches!(
                    self.tok.kind,
                    TokenKind::String | TokenKind::Number | TokenKind::BigInt
                )
            {
                self.next()?;
                found_key = true;
            }
            if self.is(TokenKind::LBracket) {
                // An index signature, mapped type or computed property.
                self.next()?;
                self.ts_skip_type2(Level::Lowest, T_INDEX_SIG)?;
                if self.is(TokenKind::Colon) {
                    self.next()?;
                    self.ts_skip_type()?;
                } else if self.is(TokenKind::In) {
                    self.next()?;
                    self.ts_skip_type()?;
                    if self.ts_at(Kw::As) {
                        // Key remapping.
                        self.next()?;
                        self.ts_skip_type()?;
                    }
                }
                self.ts_expect(TokenKind::RBracket)?;
                if matches!(self.tok.kind, TokenKind::Plus | TokenKind::Minus) {
                    self.next()?;
                }
                found_key = true;
            }
            if found_key && matches!(self.tok.kind, TokenKind::Question | TokenKind::Bang) {
                self.next()?;
            }
            self.ts_skip_type_params(TP_CONST)?;
            match self.tok.kind {
                TokenKind::Colon => {
                    if !found_key {
                        return self.ts_error();
                    }
                    self.next()?;
                    self.ts_skip_type()?;
                }
                TokenKind::LParen => {
                    // A method, call or construct signature.
                    self.ts_skip_fn_args()?;
                    if self.is(TokenKind::Colon) {
                        self.next()?;
                        self.ts_skip_return_type()?;
                    }
                }
                _ => {
                    if !found_key {
                        return self.ts_error();
                    }
                }
            }
            match self.tok.kind {
                TokenKind::RBrace => {}
                TokenKind::Comma | TokenKind::Semicolon => self.next()?,
                _ => {
                    if !self.tok.newline_before() {
                        return self.ts_error();
                    }
                }
            }
        }
        self.ts_expect(TokenKind::RBrace)
    }

    // ---- Expressions --------------------------------------------------------------------

    /// tsc's `isBinaryOperator`.
    fn ts_is_binary_operator(&self) -> bool {
        use TokenKind::*;
        match self.tok.kind {
            In | QuestionQuestion | PipePipe | AmpAmp | Pipe | Caret | Amp | Eq | Ne | StrictEq
            | StrictNe | Lt | Gt | Le | Ge | Instanceof | Shl | Shr | UShr | Plus | Minus
            | Star | Slash | Percent | StarStar => true,
            _ => self.ts_at(Kw::As) || self.ts_at(Kw::Satisfies),
        }
    }

    /// tsc's `isStartOfExpression`.
    fn ts_is_start_of_expression(&self) -> bool {
        use TokenKind::*;
        match self.tok.kind {
            This | Super | Null | True | False | Number | BigInt | String | NoSubstTemplate
            | TemplateHead | LParen | LBracket | LBrace | Function | Class | New | Slash
            | SlashAssign | Plus | Minus | Tilde | Bang | Delete | Typeof | Void | PlusPlus
            | MinusMinus | Lt | PrivateName | At => true,
            Import => matches!(self.peek().kind, LParen | Lt | Dot),
            Ident => true,
            _ => self.ts_is_binary_operator(),
        }
    }

    /// tsc's `canFollowTypeArgumentsInExpression`.
    fn ts_can_follow_type_args(&self) -> bool {
        use TokenKind::*;
        match self.tok.kind {
            LParen | NoSubstTemplate | TemplateHead => true,
            Lt | Gt | Plus | Minus | Ge | Shr | UShr | ShrAssign | UShrAssign => false,
            _ => {
                self.tok.newline_before()
                    || self.ts_is_binary_operator()
                    || !self.ts_is_start_of_expression()
            }
        }
    }

    /// The type arguments of a call, `new`, tagged template or instantiation
    /// expression, if they are (else nothing moves). Records the erasure.
    pub(super) fn ts_type_args_in_expr(&mut self) -> bool {
        if !is_lt(self.tok.kind) {
            return false;
        }
        let start = self.tok.start;
        let ok = self.ts_try(|p| Ok(p.ts_skip_type_args(true)? && p.ts_can_follow_type_args()));
        if ok {
            self.ts_erase(start);
        }
        ok
    }

    /// The parameter list of an arrow function with type annotations.
    fn ts_skip_arrow_params(&mut self) -> PResult<()> {
        self.ts_expect(TokenKind::LParen)?;
        while !self.is(TokenKind::RParen) {
            if self.is(TokenKind::Ellipsis) {
                self.next()?;
            }
            self.ts_skip_binding()?;
            if self.is(TokenKind::Question) {
                self.next()?;
            }
            if self.is(TokenKind::Colon) {
                self.next()?;
                self.ts_skip_type()?;
            }
            if self.is(TokenKind::Assign) {
                self.next()?;
                self.ts_skip_expr(false)?;
            }
            if !self.is(TokenKind::Comma) {
                break;
            }
            self.next()?;
        }
        self.ts_expect(TokenKind::RParen)
    }

    /// At a `(` whose group is not followed by `=>`: does it start an arrow function
    /// with a return type, `(a: T): U => body`? After the `?` of a conditional
    /// (`after_q`), only if another `:` follows the arrow body (tsc: `a ? (b) : c => d`
    /// is a conditional, `a ? (b) : c => d : e` contains an arrow).
    pub(super) fn ts_is_arrow_with_return_type(&mut self, after_q: bool) -> bool {
        self.ts_lookahead(|p| {
            p.ts_skip_arrow_params()?;
            if !p.is(TokenKind::Colon) {
                return Ok(false);
            }
            p.next()?;
            p.ts_skip_return_type()?;
            if !p.is(TokenKind::Arrow) || p.tok.newline_before() {
                return Ok(false);
            }
            if after_q {
                p.next()?;
                if p.is(TokenKind::LBrace) {
                    p.ts_skip_balanced()?;
                } else {
                    p.ts_skip_expr(false)?;
                }
                return Ok(p.is(TokenKind::Colon));
            }
            Ok(true)
        })
    }

    /// Is the arrow at the current `(` (or `async (`) one with annotations: its
    /// parameters skip as TypeScript and `=>` or a return type follows?
    pub(super) fn ts_is_arrow_paren(&mut self, after_q: bool) -> bool {
        let mut colon = false;
        let is_arrow = self.ts_lookahead(|p| {
            p.ts_skip_balanced()?;
            colon = p.is(TokenKind::Colon);
            Ok(p.is(TokenKind::Arrow) && !p.tok.newline_before())
        });
        is_arrow || (colon && self.ts_is_arrow_with_return_type(after_q))
    }

    /// At a `<` in an expression: does a generic arrow `<T>(x: T) => x` start here?
    pub(super) fn ts_is_generic_arrow(&mut self) -> bool {
        self.ts_lookahead(|p| {
            if p.ts_skip_type_params(TP_CONST)? == TypeParams::None || !p.is(TokenKind::LParen) {
                return Ok(false);
            }
            let cp = p.checkpoint();
            p.ts_skip_balanced()?;
            if p.is(TokenKind::Arrow) && !p.tok.newline_before() {
                return Ok(true);
            }
            p.rewind(cp);
            p.ts_skip_arrow_params()?;
            if !p.is(TokenKind::Colon) {
                return Ok(false);
            }
            p.next()?;
            p.ts_skip_return_type()?;
            Ok(p.is(TokenKind::Arrow))
        })
    }

    // ---- Statements -----------------------------------------------------------------------

    /// TypeScript declarations at the start of a statement: returns true if one was
    /// parsed. The contextual keywords follow tsc's line break rules (a line break
    /// after `type`, `declare`, ... makes it an identifier).
    pub(super) fn ts_statement(&mut self) -> PResult<bool> {
        self.ts_type_only = false;
        let tok = self.tok;
        let start = tok.start;
        match tok.kind {
            TokenKind::Function => return self.ts_function_signature(),
            TokenKind::Ident if tok.is_contextual(Kw::Async) && self.at_async_function() => {
                return self.ts_function_signature();
            }
            TokenKind::Enum => {
                self.ts_enum(false, None)?;
                return Ok(true);
            }
            TokenKind::Const if self.peek().kind == TokenKind::Enum => {
                self.next()?;
                self.ts_enum(true, None)?;
                return Ok(true);
            }
            TokenKind::Import if self.ts_at_import_alias() => {
                self.next()?;
                self.ts_import_alias(None)?;
                return Ok(true);
            }
            TokenKind::Ident if !tok.has_escape() => {}
            _ => return Ok(false),
        }
        if !matches!(
            tok.kw,
            Kw::Type | Kw::Interface | Kw::Declare | Kw::Abstract | Kw::Namespace | Kw::Module
        ) {
            return Ok(false);
        }
        let next = self.peek();
        if next.newline_before() {
            return Ok(false);
        }
        match tok.kw {
            Kw::Type | Kw::Interface => {
                if next.kind != TokenKind::Ident {
                    return Ok(false);
                }
                if tok.kw == Kw::Type {
                    self.ts_type_alias()?;
                } else {
                    self.ts_interface()?;
                }
                self.ts_type_only = true;
            }
            Kw::Declare => {
                let ok = matches!(
                    next.kind,
                    TokenKind::Var
                        | TokenKind::Const
                        | TokenKind::Function
                        | TokenKind::Class
                        | TokenKind::Enum
                ) || (next.kind == TokenKind::Ident
                    && !next.has_escape()
                    && matches!(
                        next.kw,
                        Kw::Let
                            | Kw::Async
                            | Kw::Abstract
                            | Kw::Namespace
                            | Kw::Module
                            | Kw::Global
                            | Kw::Type
                            | Kw::Interface
                    ));
                if !ok {
                    return Ok(false);
                }
                self.ts_declare(None)?;
                self.ts_type_only = true;
            }
            Kw::Abstract => {
                if next.kind != TokenKind::Class {
                    return Ok(false);
                }
                self.next()?;
                self.ts_erase(start);
                self.parse_class(true, crate::heap::Atom::NULL)?;
                return Ok(true);
            }
            _ => {
                // `namespace N {}` / `module N {}` / `module "m" {}`.
                if next.kind == TokenKind::String && tok.kw == Kw::Module {
                    self.next()?;
                    self.ts_skip_ambient_body()?;
                    self.ts_type_only = true;
                } else if next.kind == TokenKind::Ident {
                    self.next()?;
                    self.ts_namespace(None)?;
                    if !self.ts_type_only {
                        return Ok(true);
                    }
                } else {
                    return Ok(false);
                }
            }
        }
        self.ts_erase_stmt(start);
        Ok(true)
    }

    /// At `function` or `async function` of a declaration: an overload signature or
    /// ambient declaration without a body is skipped (returns true).
    fn ts_function_signature(&mut self) -> PResult<bool> {
        let start = self.tok.start;
        let ok = self.ts_try(|p| p.ts_skip_signature());
        if !ok {
            return Ok(false);
        }
        self.expect_semicolon()?;
        self.ts_erase(start);
        self.ts_type_only = true;
        Ok(true)
    }

    /// Skips a function header without a body (speculative: false if it has one).
    pub(super) fn ts_skip_signature(&mut self) -> PResult<bool> {
        {
            let p = self;
            if p.tok.is_contextual(Kw::Async) {
                p.next()?;
            }
            if !p.is(TokenKind::Function) {
                return Ok(false);
            }
            p.next()?;
            if p.is(TokenKind::Star) {
                p.next()?;
            }
            if p.tok.kind.is_identifier_name() {
                p.next()?;
            }
            p.ts_skip_type_params(TP_CONST)?;
            if !p.is(TokenKind::LParen) {
                return Ok(false);
            }
            p.ts_skip_balanced()?;
            if p.is(TokenKind::Colon) {
                p.next()?;
                p.ts_skip_return_type()?;
            }
            Ok(!p.is(TokenKind::LBrace))
        }
    }

    /// `type T<P> = ...;` at `type`.
    pub(super) fn ts_type_alias(&mut self) -> PResult<()> {
        self.next()?;
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "identifier expected");
        }
        self.next()?;
        self.ts_skip_type_params(TP_IN_OUT | TP_CONST | TP_EMPTY)?;
        self.ts_expect(TokenKind::Assign)?;
        self.ts_skip_type()?;
        self.expect_semicolon()
    }

    /// `interface I<P> extends J { ... }` at `interface`.
    pub(super) fn ts_interface(&mut self) -> PResult<()> {
        self.next()?;
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "identifier expected");
        }
        self.next()?;
        self.ts_skip_type_params(TP_IN_OUT | TP_EMPTY)?;
        for kw in [Kw::None, Kw::Implements] {
            let at = if kw == Kw::None {
                self.is(TokenKind::Extends)
            } else {
                self.ts_at(kw)
            };
            if at {
                loop {
                    self.next()?;
                    self.ts_skip_type()?;
                    if !self.is(TokenKind::Comma) {
                        break;
                    }
                }
            }
        }
        self.ts_skip_object_type()
    }

    /// `Name`, `A.B.C` or a module name string, then a balanced body or `;`.
    fn ts_skip_ambient_body(&mut self) -> PResult<()> {
        if self.is(TokenKind::String) {
            self.next()?;
        } else {
            loop {
                if !self.is(TokenKind::Ident) {
                    return syntax(self.tok.start, "identifier expected");
                }
                self.next()?;
                if !self.is(TokenKind::Dot) {
                    break;
                }
                self.next()?;
            }
        }
        if self.is(TokenKind::LBrace) {
            return self.ts_skip_balanced();
        }
        self.expect_semicolon()
    }

    /// `declare ...` at `declare`: ambient declarations emit nothing. In namespace
    /// `ns`, `export declare let x` makes `x` a member (assigned from outside).
    pub(super) fn ts_declare(&mut self, ns: Option<u32>) -> PResult<()> {
        self.ts_type_only = true;
        self.next()?;
        let tok = self.tok;
        let is_let = tok.is_contextual(Kw::Let);
        if matches!(tok.kind, TokenKind::Var | TokenKind::Const) || is_let {
            self.next()?;
            if self.is(TokenKind::Enum) {
                return self.ts_skip_ambient_enum();
            }
            loop {
                if let Some(d) = ns
                    && self.is(TokenKind::Ident)
                {
                    let a = self.ident_atom()?;
                    self.ts_declare_member(d, a);
                    self.ts_type_only = false;
                }
                self.ts_skip_binding()?;
                if self.is(TokenKind::Bang) {
                    self.next()?;
                }
                if self.is(TokenKind::Colon) {
                    self.next()?;
                    self.ts_skip_type()?;
                }
                if self.is(TokenKind::Assign) {
                    self.next()?;
                    self.ts_skip_expr(true)?;
                }
                if !self.is(TokenKind::Comma) {
                    break;
                }
                self.next()?;
            }
            return self.expect_semicolon();
        }
        if tok.kind == TokenKind::Function || tok.is_contextual(Kw::Async) {
            if !self.ts_function_signature()? {
                return syntax(self.tok.start, "an ambient function cannot have a body");
            }
            return Ok(());
        }
        if tok.is_contextual(Kw::Abstract) {
            self.next()?;
        }
        if self.is(TokenKind::Class) {
            self.next()?;
            if self.is(TokenKind::Ident) {
                self.next()?;
            }
            self.ts_skip_type_params(TP_IN_OUT | TP_CONST)?;
            if self.is(TokenKind::Extends) {
                // The heritage is an expression (`extends Mixin(Base)`), with type
                // arguments.
                self.next()?;
                loop {
                    match self.tok.kind {
                        _ if self.ts_at(Kw::Implements) => break,
                        TokenKind::Ident | TokenKind::Dot => self.next()?,
                        TokenKind::LParen => self.ts_skip_balanced()?,
                        TokenKind::Lt => {
                            self.ts_skip_type_args(false)?;
                        }
                        _ => break,
                    }
                }
            }
            if self.ts_at(Kw::Implements) {
                loop {
                    self.next()?;
                    self.ts_skip_type()?;
                    if !self.is(TokenKind::Comma) {
                        break;
                    }
                }
            }
            if !self.is(TokenKind::LBrace) {
                return syntax(self.tok.start, "expecting '{'");
            }
            return self.ts_skip_balanced();
        }
        if self.is(TokenKind::Enum) {
            return self.ts_skip_ambient_enum();
        }
        let tok = self.tok;
        if tok.is_contextual(Kw::Namespace) || tok.is_contextual(Kw::Module) {
            self.next()?;
            return self.ts_skip_ambient_body();
        }
        if tok.is_contextual(Kw::Global) {
            self.next()?;
            if !self.is(TokenKind::LBrace) {
                return syntax(self.tok.start, "expecting '{'");
            }
            return self.ts_skip_balanced();
        }
        if tok.is_contextual(Kw::Type) {
            return self.ts_type_alias();
        }
        if tok.is_contextual(Kw::Interface) {
            return self.ts_interface();
        }
        syntax(tok.start, "unexpected token after 'declare'")
    }

    /// `enum E { ... }` of a `declare` (at `enum`).
    fn ts_skip_ambient_enum(&mut self) -> PResult<()> {
        self.next()?;
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "identifier expected");
        }
        self.next()?;
        if !self.is(TokenKind::LBrace) {
            return syntax(self.tok.start, "expecting '{'");
        }
        self.ts_skip_balanced()
    }

    // ---- Annotations ----------------------------------------------------------------------

    /// An optional `: type` annotation (erased).
    pub(super) fn ts_annotation(&mut self) -> PResult<()> {
        if !self.ts || !self.is(TokenKind::Colon) {
            return Ok(());
        }
        let start = self.tok.start;
        self.next()?;
        self.ts_skip_type()?;
        self.ts_erase(start);
        Ok(())
    }

    /// An optional `: type` return type (erased).
    pub(super) fn ts_return_annotation(&mut self) -> PResult<()> {
        if !self.ts || !self.is(TokenKind::Colon) {
            return Ok(());
        }
        let start = self.tok.start;
        self.next()?;
        self.ts_skip_return_type()?;
        self.ts_erase(start);
        Ok(())
    }

    /// The type parameters of a function, method or arrow (erased). If the list and the
    /// `(` span lines, ts-blank-space writes the `(` in place of the `<` (so a `return`
    /// before it keeps its operand) and blanks the original.
    pub(super) fn ts_fn_type_params(&mut self) -> PResult<()> {
        if !self.ts || !self.is(TokenKind::Lt) {
            return Ok(());
        }
        let start = self.tok.start;
        self.ts_skip_type_params(TP_CONST)?;
        let paren = self.tok;
        let spans = paren.kind == TokenKind::LParen
            && self.src[start as usize + 1..paren.end as usize].contains(&b'\n');
        if spans {
            self.ts_erase_kind(start, self.prev_end, ER_OPEN_PAREN);
            self.ts_erase_kind(paren.start, paren.end, ER_BLANK);
        } else {
            self.ts_erase(start);
        }
        Ok(())
    }

    /// An arrow's return type (erased). If a line break separates the parameters from
    /// `=>`, ts-blank-space moves the `)` to the end of the type (`=>` must stay on the
    /// line of the `)`).
    pub(super) fn ts_arrow_return_annotation(&mut self) -> PResult<()> {
        if !self.ts || !self.is(TokenKind::Colon) {
            return Ok(());
        }
        let params_end = self.ts_params_end as usize;
        let paren = self.prev_end - 1;
        let start = self.tok.start;
        self.next()?;
        self.ts_skip_return_type()?;
        let end = self.prev_end;
        if self.src[params_end..end as usize].contains(&b'\n') {
            self.ts_erase_kind(paren, end - 1, ER_BLANK);
            self.ts_erase_kind(end - 1, end, ER_CLOSE_PAREN);
        } else {
            self.ts_erase(start);
        }
        Ok(())
    }

    /// An optional type parameter list (erased).
    pub(super) fn ts_type_params(&mut self, flags: u8) -> PResult<()> {
        if !self.ts || !self.is(TokenKind::Lt) {
            return Ok(());
        }
        let start = self.tok.start;
        self.ts_skip_type_params(flags)?;
        self.ts_erase(start);
        Ok(())
    }

    /// At the start of a formal parameter: a `this` parameter (type only: erased with
    /// its comma, returns true) or parameter decorators (legacy, into `dp`) and
    /// accessibility / `readonly` / `override` modifiers (erased, but for `strip_types`
    /// not in a constructor; parameter properties are found by `ts_scan_param_props`
    /// beforehand).
    pub(super) fn ts_param_prefix(
        &mut self,
        dp: Option<&mut super::decorators::DecParams>,
    ) -> PResult<bool> {
        let start = self.tok.start;
        if self.is(TokenKind::This)
            && self.f.param_count == 0
            && self.f.kind != super::func::FuncKind::Arrow
        {
            self.next()?;
            if self.is(TokenKind::Colon) {
                self.next()?;
                self.ts_skip_type()?;
            }
            if !self.is(TokenKind::RParen) {
                self.expect(TokenKind::Comma, ",")?;
            }
            self.ts_erase(start);
            return Ok(true);
        }
        if self.is(TokenKind::At) {
            self.parse_param_decorators(dp)?;
        }
        // (After the decorators: ts-blank-space keeps them.)
        let start = self.tok.start;
        let mut any = false;
        while self.ts_is_param_modifier() {
            self.next()?;
            any = true;
        }
        if any {
            if matches!(
                self.f.kind,
                super::func::FuncKind::ClassCtor | super::func::FuncKind::DerivedCtor
            ) {
                // A parameter property: `public x` also means `this.x = x`, so it is not
                // erasable. `strip_types` keeps it, as it keeps an enum, and the text
                // does not compile as JavaScript (plain dumps stay TypeScript, 0209).
                let erased = self.erased.take();
                self.ts_erase(start);
                self.erased = erased;
            } else {
                self.ts_erase(start);
            }
        }
        Ok(false)
    }

    /// At the `(` of a class constructor: the names of its parameter properties
    /// (parameters with a modifier), found before the parameters are parsed: a base
    /// constructor defines their fields at entry.
    pub(super) fn ts_scan_param_props(&mut self) -> PResult<Vec<crate::heap::Atom>> {
        let mut names: Vec<Token> = Vec::new();
        self.ts_lookahead(|p| {
            p.ts_expect(TokenKind::LParen)?;
            while !p.is(TokenKind::RParen) {
                while p.is(TokenKind::At) {
                    p.next()?;
                    p.ts_skip_decorator()?;
                }
                let mut modifier = false;
                while p.ts_is_param_modifier() {
                    modifier = true;
                    p.next()?;
                }
                if p.is(TokenKind::Ellipsis) {
                    p.next()?;
                }
                if modifier && p.is(TokenKind::Ident) {
                    names.push_out(p.tok);
                }
                p.ts_skip_binding()?;
                if p.is(TokenKind::Question) {
                    p.next()?;
                }
                if p.is(TokenKind::Colon) {
                    p.next()?;
                    p.ts_skip_type()?;
                }
                if p.is(TokenKind::Assign) {
                    p.next()?;
                    p.ts_skip_expr(false)?;
                }
                if !p.is(TokenKind::Comma) {
                    break;
                }
                p.next()?;
            }
            Ok(true)
        });
        let mut atoms = crate::buf::vec_with_capacity(names.len());
        for t in names {
            let bytes = self.lx.ident_value(&t, &mut self.buf8);
            let s = core::str::from_utf8(bytes).unwrap_or("\u{FFFD}");
            atoms.push_out(self.heap.atom_str(s)?);
        }
        Ok(atoms)
    }

    /// A decorator expression after its `@` (parameter property lookahead).
    fn ts_skip_decorator(&mut self) -> PResult<()> {
        if self.is(TokenKind::LParen) {
            return self.ts_skip_balanced();
        }
        if !self.is(TokenKind::Ident) {
            return self.ts_error();
        }
        self.next()?;
        loop {
            match self.tok.kind {
                TokenKind::Dot => {
                    self.next()?;
                    if !self.tok.kind.is_identifier_name() && !self.is(TokenKind::PrivateName) {
                        return self.ts_error();
                    }
                    self.next()?;
                }
                TokenKind::LParen => self.ts_skip_balanced()?,
                _ => return Ok(()),
            }
        }
    }

    /// An accessibility, `readonly` or `override` modifier of a parameter (otherwise it
    /// is the parameter's name).
    fn ts_is_param_modifier(&self) -> bool {
        let t = self.tok;
        let modifier = t.is_contextual(Kw::Public)
            || t.is_contextual(Kw::Private)
            || t.is_contextual(Kw::Protected)
            || t.is_contextual(Kw::Readonly)
            || t.is_contextual(Kw::Override);
        modifier
            && matches!(
                self.peek().kind,
                TokenKind::Ident | TokenKind::LBrace | TokenKind::LBracket
            )
    }

    /// Does the token after the current one start a class member name (so the current
    /// identifier is a modifier)? With `same_line`, not across a line break.
    fn ts_next_is_member_name(&self, same_line: bool) -> bool {
        let next = self.peek();
        if same_line && next.newline_before() {
            return false;
        }
        next.kind.is_identifier_name()
            || matches!(
                next.kind,
                TokenKind::String
                    | TokenKind::Number
                    | TokenKind::BigInt
                    | TokenKind::LBracket
                    | TokenKind::PrivateName
                    | TokenKind::Star
            )
    }

    fn ts_is_member_modifier(&self) -> bool {
        let t = self.tok;
        t.is_contextual(Kw::Public)
            || t.is_contextual(Kw::Private)
            || t.is_contextual(Kw::Protected)
            || t.is_contextual(Kw::Readonly)
            || t.is_contextual(Kw::Override)
    }

    /// Erases the accessibility, `readonly` and `override` modifiers of a class member.
    /// Returns the erasure of the first modifier if it starts the member (ts-blank-space
    /// turns it into a `;` when the member's name is computed).
    pub(super) fn ts_member_modifiers(&mut self) -> PResult<Option<usize>> {
        let mut first = None;
        while self.ts_is_member_modifier() && self.ts_next_is_member_name(false) {
            let start = self.tok.start;
            self.next()?;
            if first.is_none() {
                first = self.erased.as_ref().map(|e| e.len());
            }
            self.ts_erase(start);
        }
        Ok(first)
    }

    /// A class member that emits no code (an index signature, a method overload
    /// signature, a `declare` or `abstract` member) is skipped: returns true.
    pub(super) fn ts_skip_class_member(&mut self) -> PResult<bool> {
        let start = self.tok.start;
        let mut field = false;
        let ok = self.ts_try(|p| {
            let mut no_code = false;
            // Modifiers.
            loop {
                let t = p.tok;
                if t.kind != TokenKind::Ident || t.has_escape() {
                    break;
                }
                match t.kw {
                    Kw::Declare | Kw::Abstract => {
                        if !p.ts_next_is_member_name(true) {
                            break;
                        }
                        no_code = true;
                    }
                    Kw::Public
                    | Kw::Private
                    | Kw::Protected
                    | Kw::Readonly
                    | Kw::Override
                    | Kw::Static
                    | Kw::Async
                    | Kw::Get
                    | Kw::Set
                    | Kw::Accessor => {
                        if !p.ts_next_is_member_name(false) {
                            break;
                        }
                    }
                    _ => break,
                }
                p.next()?;
            }
            if p.is(TokenKind::Star) {
                p.next()?;
            }
            // The member name.
            match p.tok.kind {
                TokenKind::LBracket => {
                    p.next()?;
                    if p.is(TokenKind::Ident) && p.peek().kind == TokenKind::Colon {
                        // An index signature `[key: K]: T`.
                        p.next()?;
                        p.next()?;
                        p.ts_skip_type()?;
                        p.ts_expect(TokenKind::RBracket)?;
                        if p.is(TokenKind::Colon) {
                            p.next()?;
                            p.ts_skip_type()?;
                        }
                        return Ok(true);
                    }
                    p.ts_skip_tokens(0)?;
                    if !p.is(TokenKind::RBracket) {
                        return Ok(false);
                    }
                    p.next()?;
                }
                k if k.is_identifier_name()
                    || matches!(
                        k,
                        TokenKind::String
                            | TokenKind::Number
                            | TokenKind::BigInt
                            | TokenKind::PrivateName
                    ) =>
                {
                    p.next()?;
                }
                _ => return Ok(false),
            }
            if matches!(p.tok.kind, TokenKind::Question | TokenKind::Bang) {
                p.next()?;
            }
            if matches!(p.tok.kind, TokenKind::Lt | TokenKind::LParen) {
                // A method: no code without a body.
                p.ts_skip_type_params(TP_CONST)?;
                if !p.is(TokenKind::LParen) {
                    return Ok(false);
                }
                p.ts_skip_balanced()?;
                if p.is(TokenKind::Colon) {
                    p.next()?;
                    p.ts_skip_return_type()?;
                }
                if !p.is(TokenKind::LBrace) {
                    no_code = true;
                }
            } else if no_code {
                // A `declare` or `abstract` field.
                field = true;
                if p.is(TokenKind::Colon) {
                    p.next()?;
                    p.ts_skip_type()?;
                }
                if p.is(TokenKind::Assign) {
                    p.next()?;
                    p.ts_skip_expr(true)?;
                }
            }
            Ok(no_code)
        });
        if !ok {
            return Ok(false);
        }
        if self.is(TokenKind::Semicolon) {
            self.next()?;
        }
        // ts-blank-space blanks a declared field like a statement, signatures exactly.
        if field {
            self.ts_erase_stmt(start);
        } else {
            self.ts_erase(start);
        }
        Ok(true)
    }

    /// Erases the current token if it is `kind` (`?` of an optional parameter, `!` of a
    /// definite assignment).
    pub(super) fn ts_eat_marker(&mut self, kind: TokenKind) -> PResult<bool> {
        if !self.ts || !self.is(kind) {
            return Ok(false);
        }
        let start = self.tok.start;
        self.next()?;
        self.ts_erase(start);
        Ok(true)
    }
}

/// Erasure kinds: blanks; blanks after a `;` in place of the first character; an
/// `as`/`satisfies` (blanks, or `;` if it turns out to end its statement).
pub(super) const ER_BLANK: u8 = 0;
pub(super) const ER_SEMI: u8 = 1;
pub(super) const ER_AS: u8 = 2;
/// Blanks after a `(` in place of the first character, or a `)` in place of it.
pub(super) const ER_OPEN_PAREN: u8 = 3;
pub(super) const ER_CLOSE_PAREN: u8 = 4;

/// The source with the erased `ranges` blanked like ts-blank-space: one space per
/// UTF-16 unit, line feeds and carriage returns kept.
pub(super) fn blank(src: &[u8], mut ranges: super::parser::Erased) -> Vec<u8> {
    crate::sort::sort_by(&mut ranges, |a, b| a.0 < b.0);
    let mut out = crate::buf::vec_with_capacity(src.len());
    let mut pos = 0usize;
    for (start, end, kind) in ranges {
        let (start, end) = (start as usize, end as usize);
        if end <= pos {
            continue;
        }
        let mut start = start.max(pos);
        out.extend_from_slice(&src[pos..start]);
        let first = match kind {
            ER_SEMI => Some(b';'),
            ER_OPEN_PAREN => Some(b'('),
            ER_CLOSE_PAREN => Some(b')'),
            _ => None,
        };
        if let Some(c) = first {
            out.push_out(c);
            start += 1;
        }
        for &b in &src[start..end] {
            match b {
                b'\n' | b'\r' => out.push_out(b),
                // UTF-8 continuation bytes add no unit; a 4-byte sequence is two.
                0x80..=0xBF => {}
                0xF0..=0xFF => out.extend_from_slice(b"  "),
                _ => out.push_out(b' '),
            }
        }
        pos = end;
    }
    out.extend_from_slice(&src[pos..]);
    out
}
