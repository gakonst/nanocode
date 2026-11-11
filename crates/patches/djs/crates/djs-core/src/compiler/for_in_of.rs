//! `for-in` and `for-of` loops (docs/compiler.md 3.8, 4.5, 7.4).
//!
//! The left side is a per-iteration assignment target that appears before the
//! iterated expression in the source but runs after it, every iteration. Code is
//! emitted in source order, so the layout jumps around it. With an expression on the
//! left (`for (a.b of xs)`), as in QuickJS:
//!
//! ```text
//!     jump L_rhs
//! L_assign:                      ; for-of: it next result / for-in: it key
//!     [get_field value]          ; for-of: it next value
//!     <lhs reference; store>     ; (for-of: region 1 of the close handler)
//!     jump L_body
//! L_rhs:
//!     <rhs>; <setup>             ; for-of: it next / for-in: it
//!     jump L_step
//!     [L_throw: ...]             ; for-of: the close handler, see below
//! L_body:
//!     <body>                     ; (for-of: region 2 of the close handler)
//! L_step:                        ; the continue target
//!     <step>; jump_if_false L_assign
//!     <done: drop the loop state>
//! ```
//!
//! With a declaration on the left (`for (const x of xs)`) nothing of the left side runs
//! before the right side, so the store moves next to the body and the extra jump goes:
//! `<rhs>; <setup>; jump L_step; [L_throw]; L_assign: <store>; <body>; L_step: ...`.
//!
//! for-of keeps `it next` in temporaries (`for-of.iterator@S`, `for-of.next@S`, D2732).
//! Setup is `call_intrinsic %GetIterator 1; dup; get_field next; put next; put it`, the
//! step is `get it; get next; iter_next L_value; iter_check_object; dup; get_field done`
//! (D1290; for-await: `get next; get it; call 0; await; ...`). `iter_next` jumps to
//! `L_value`, right after `get_field value`, with the value itself when it needs no
//! result object (D1800). The lhs store and the body are covered by a handler
//! (`L_throw`) that closes the iterator with `%IteratorCloseThrow` and rethrows; errors
//! from the iterator itself (`next`, `done`, `value`) are not covered, so they do not
//! close it. `break`, `continue` and `return` leaving the loop (and a generator's return
//! from the store) close it in the `L_close` block (`BlockKind::Iter`), entered with
//! their completion (compiler/completion.rs), which sits outside the handler so that an
//! error from `return()` propagates instead of closing twice. A `break` of the loop
//! itself goes to `L_break`, which closes inline (it is outside the handler too).
//!
//! for-in keeps the key iterator of `for_in_start` in a temporary; its step is
//! `for_in_next` and leaving the loop just leaves it.
use crate::PushOut;

use alloc::vec::Vec;

use super::PResult;
use super::func::{HandlerLabels, ScopeKind, VarKind};
use super::parser::{BlockKind, Checkpoint, Parser, syntax};
use super::pattern::Binding;
use super::scan::Follow;
use super::smap::SiteKind;
use super::smap_syntax::{DeclKind, NodeKind, Role};
use crate::bytecode::Label;
use crate::bytecode::intrinsics::intrinsic_id_of;
use crate::heap::{Atom, atoms};
use crate::lexer::{Kw, TokenKind};

/// The left side of a for-in/of head, as far as the store needs it.
#[derive(Clone, Copy)]
enum Target<'a> {
    /// `var x` or `let x` / `const x`: a store to a declared name.
    Decl { name: Atom, pos: u32, lexical: bool },
    /// `var [x]` or `let {x}`: a pattern, skipped; it is parsed at the store.
    Pattern { cp: Checkpoint<'a>, b: Binding },
    /// An expression target: the store was emitted at `L_assign` already.
    Expr,
}

impl<'a> Parser<'a, '_> {
    /// A for-in/of loop after `for (`, with the current token at the start of the
    /// head. `for_pos` is the `for` keyword (errors from the iteration report there,
    /// as in QuickJS).
    // Out of line: its frame is not on the stack while a C-style loop's body nests
    // (D1941).
    #[inline(never)]
    pub(super) fn parse_for_in_of(
        &mut self,
        mut labels: Vec<Atom>,
        for_pos: u32,
        is_await: bool,
    ) -> PResult<()> {
        let outer = self.scope;
        let mut scoped = false;
        let l_assign = self.new_label();
        // for-of: after `get_field value`, where `iter_next` jumps with the value.
        let l_value = self.new_label();
        let l_body = self.new_label();
        // Handler regions of the for-of close handler: [start, end) pairs.
        let mut regions: Vec<(Label, Label)> = Vec::new();
        // `for (using x of xs)`: Some(is_await).
        let mut using_head = None;
        // The loop's iterator and (for-of) `next` temporaries (D2732), once its kind is
        // known.
        let mut temps: Option<(Atom, Option<Atom>)> = None;
        // for-of: the block that closes the iterator when a jump or a return leaves the
        // loop, entered with a completion (compiler/completion.rs). Its env is pushed
        // while the target is stored, where a generator's return can leave the loop too.
        let l_close = self.new_label();
        let mut iter_env = None;
        let head = self.tok.start;
        let target = match self.tok.kind {
            TokenKind::Ident if self.at_using(true) || self.at_await_using() => {
                let is_await = self.tok.kw == Kw::Await;
                if is_await {
                    self.next()?;
                }
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                scoped = true;
                if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                    return syntax(
                        self.tok.start,
                        "binding patterns are not allowed in using declarations",
                    );
                }
                using_head = Some(is_await);
                let (name, pos) = self.for_binding()?;
                let v = self.declare_lexical(name, VarKind::Const, pos)?;
                self.syn_decl(
                    v,
                    if is_await {
                        DeclKind::AwaitUsing
                    } else {
                        DeclKind::Using
                    },
                );
                Target::Decl {
                    name,
                    pos,
                    lexical: true,
                }
            }
            TokenKind::Var => {
                self.next()?;
                if let Some(t) = self.for_pattern(Binding::Var)? {
                    t
                } else {
                    let (name, pos) = self.for_binding()?;
                    self.declare_var(name, self.tok.start)?;
                    Target::Decl {
                        name,
                        pos,
                        lexical: false,
                    }
                }
            }
            TokenKind::Const => {
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                scoped = true;
                if let Some(t) = self.for_pattern(Binding::Lexical(VarKind::Const))? {
                    t
                } else {
                    let (name, pos) = self.for_binding()?;
                    self.declare_lexical(name, VarKind::Const, pos)?;
                    Target::Decl {
                        name,
                        pos,
                        lexical: true,
                    }
                }
            }
            TokenKind::Ident if self.tok.is_contextual(Kw::Let) => {
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                scoped = true;
                if let Some(t) = self.for_pattern(Binding::Lexical(VarKind::Let))? {
                    t
                } else {
                    let (name, pos) = self.for_binding()?;
                    self.declare_lexical(name, VarKind::Let, pos)?;
                    Target::Decl {
                        name,
                        pos,
                        lexical: true,
                    }
                }
            }
            _ => {
                let is_of = is_await || self.for_expr_head_is_of()?;
                temps = Some(self.loop_temps(is_of, is_await, for_pos));
                let l_rhs = self.new_label();
                self.code().jump(l_rhs);
                self.place(l_assign);
                if is_of {
                    self.code().get_field(atoms::VALUE, 0);
                    self.place(l_value);
                } else {
                    // `it key` (for_in_next keeps the iterator): the key alone.
                    self.code().nip();
                }
                let start = self.new_label();
                let end = self.new_label();
                self.place(start);
                if is_of {
                    self.push_completion_block(BlockKind::Iter, l_close, for_pos);
                }
                if self.starts_assignment_pattern(Follow::InOf) {
                    // `for ([a, b] of xs)`: the pattern takes the value.
                    self.parse_pattern(Binding::Assign)?;
                } else {
                    // `it next value` (for-in: `it key`): the reference goes on top of
                    // the value, and the store wants the value on top of the reference.
                    let e = self.parse_lhs(false)?;
                    if self.is(TokenKind::Assign) {
                        return self.for_initializer_error();
                    }
                    let lv = self.lvalue(e, "invalid for in/of left hand-side")?;
                    match lv.refs() {
                        0 => {}
                        1 => self.code().swap(),
                        2 => self.code().rot3l(),
                        // `v this base key`: store a copy, then drop the value.
                        _ => self.code().pick(3),
                    }
                    self.emit_store(lv, false);
                    if lv.refs() == 3 {
                        self.code().drop();
                    }
                }
                if is_of {
                    iter_env = self.blocks.pop();
                }
                self.place(end);
                regions.push_out((start, end));
                self.code().jump(l_body);
                self.place(l_rhs);
                Target::Expr
            }
        };
        if matches!(target, Target::Decl { .. } | Target::Pattern { .. })
            && self.is(TokenKind::Assign)
        {
            return self.for_initializer_error();
        }
        let is_of = self.tok.is_contextual(Kw::Of);
        self.syn_kind(match (is_await, is_of) {
            (true, _) => NodeKind::ForAwait,
            (false, true) => NodeKind::ForOf,
            (false, false) => NodeKind::ForIn,
        });
        self.syn_part(Role::Head, head, self.prev_end);
        if using_head.is_some() && self.is(TokenKind::In) {
            return syntax(
                self.tok.start,
                "using declaration not allowed in for-in loop",
            );
        }
        if is_await && !is_of {
            return syntax(self.tok.start, "'for await' loop should be used with 'of'");
        }
        if !is_of && !self.is(TokenKind::In) {
            return syntax(
                self.tok.start,
                "expected 'of' or 'in' in for control expression",
            );
        }
        let (it, next_t) = match temps {
            Some(t) => t,
            None => self.loop_temps(is_of, is_await, for_pos),
        };
        self.next()?;
        // The right side: for-of takes an AssignmentExpression, for-in an Expression.
        // Lexical bindings are in their TDZ here (`for (let x of x)`).
        let iterable = self.tok.start;
        if is_of {
            self.parse_assign(false)?;
        } else {
            self.parse_expr(false)?;
        }
        self.syn_part(Role::Iterable, iterable, self.prev_end);
        self.expect(TokenKind::RParen, ")")?;
        let head_end = self.prev_end;
        if let Target::Decl {
            name,
            lexical: true,
            ..
        } = target
            && let Some(v) = self.f.find_in_scope(self.scope, name)
        {
            // Initialized per iteration before the body runs.
            self.f.vars[v as usize].init_end = self.prev_end;
        }
        self.mark(for_pos);
        if is_of {
            let get_iterator = if is_await {
                intrinsic_id_of!("GetAsyncIterator")
            } else {
                intrinsic_id_of!("GetIterator")
            };
            self.code().call_intrinsic(get_iterator, 1);
            self.code().dup();
            self.code().get_field(atoms::NEXT, 0);
            if let Some(n) = next_t {
                self.temp_put(n);
            }
            self.temp_put(it);
        } else {
            self.code().for_in_start();
            self.temp_put(it);
        }
        let l_step = self.new_label();
        self.code().jump(l_step);
        let l_throw = self.new_label();
        if is_of {
            // `exc` -> `exc`, after closing the iterator (errors from `return()` lose to
            // the body's exception).
            self.place(l_throw);
            if is_await {
                // The body's exception waits in a temporary of its own while the close
                // is awaited, so the suspended frame keeps nothing on its stack.
                let exc = self.temp("for-await.exception", for_pos);
                self.temp_put(exc);
                self.temp_get(it);
                // A promise that always fulfills (errors from `return()` are dropped).
                let close = intrinsic_id_of!("AsyncIteratorCloseThrow");
                self.code().call_intrinsic(close, 1);
                self.site(SiteKind::ForAwaitClose, for_pos);
                self.code().await_(0);
                self.code().drop();
                self.temp_get(exc);
            } else {
                self.temp_get(it);
                let close = intrinsic_id_of!("IteratorCloseThrow");
                self.code().call_intrinsic(close, 1);
                self.code().drop();
            }
            self.code().throw();
            match iter_env.take() {
                Some(env) => self.blocks.push_out(env),
                None => self.push_completion_block(BlockKind::Iter, l_close, for_pos),
            }
        }
        let mut using = None;
        match target {
            Target::Decl { name, pos, lexical } => {
                self.place(l_assign);
                if is_of {
                    self.code().get_field(atoms::VALUE, 0);
                    self.place(l_value);
                } else {
                    self.code().nip();
                }
                let start = self.new_label();
                self.place(start);
                if let Some(is_await) = using_head {
                    // Each iteration's resource is disposed at its end (compiler/using.rs).
                    let r = self.using_region_start(is_await, pos);
                    self.mark(pos);
                    self.emit_using_add(r.rec, is_await);
                    using = Some(r);
                }
                if lexical {
                    // A fresh binding (Box) per iteration.
                    let s = self.scope as u16;
                    self.code().enter_scope(s);
                    let r = self.init_ref(name, pos);
                    self.mark(pos);
                    self.code().name_init(r);
                } else {
                    let r = self.name_ref(name, pos);
                    self.mark(pos);
                    self.code().name_put(r);
                }
                regions.push_out((start, l_step));
                self.place(l_body);
            }
            Target::Pattern { cp, b } => {
                self.place(l_assign);
                if is_of {
                    self.code().get_field(atoms::VALUE, 0);
                    self.place(l_value);
                } else {
                    self.code().nip();
                }
                let start = self.new_label();
                self.place(start);
                if matches!(b, Binding::Lexical(_)) {
                    // Fresh bindings (Boxes) per iteration.
                    let s = self.scope as u16;
                    self.code().enter_scope(s);
                }
                let mark = self.pattern_vars.len();
                let end = self.checkpoint();
                self.rewind(cp);
                self.parse_pattern(b)?;
                self.rewind(end);
                // Initialized per iteration before the body runs.
                self.end_pattern_bindings(mark, head_end);
                regions.push_out((start, l_step));
                self.place(l_body);
            }
            Target::Expr => {
                self.place(l_body);
                let start = self.new_label();
                self.place(start);
                regions.push_out((start, l_step));
            }
        }

        // The body.
        let l_break = self.new_label();
        self.push_block(&mut labels, BlockKind::Loop, l_break, Some(l_step), for_pos);
        if let Some(r) = &using {
            self.using_env(r);
        }
        self.ts_semi = true;
        // On an error the parser state is abandoned (a nested function's may not be
        // restored), so only pop on success.
        let body = self.tok.start;
        self.parse_statement()?;
        self.syn_part(Role::Body, body, self.prev_end);
        if let Some(r) = using {
            self.close_using(r);
        }
        let loop_used = self.pop_block();
        let iter_env = if is_of { self.blocks.pop() } else { None };

        self.place(l_step);
        if is_of {
            for &(start, end) in &regions {
                self.f.handlers.push_out(HandlerLabels {
                    start,
                    end,
                    target: l_throw,
                    depth: 0,
                    depth_from: None,
                });
            }
            let next_t = next_t.expect("a for-of's next");
            self.mark(for_pos);
            if is_await {
                self.temp_get(next_t);
                self.temp_get(it);
                self.code().call(0);
                self.site(SiteKind::ForAwaitNext, for_pos);
                self.code().await_(0);
            } else {
                self.temp_get(it);
                self.temp_get(next_t);
                self.code().iter_next(l_value);
            }
            self.code().iter_check_object();
            self.code().dup();
            self.code().get_field(atoms::DONE, 0);
            self.code().jump_if_false(l_assign);
            self.code().drop();
            let close = iter_env.and_then(|e| e.completion);
            let close_used = close.as_ref().is_some_and(|c| c.has_exits());
            let l_end = self.new_label();
            if close_used || loop_used {
                self.code().jump(l_end);
            }
            let close_fn = intrinsic_id_of!("IteratorClose");
            if let Some(c) = close.as_ref().filter(|c| c.has_exits()) {
                // Closes the iterator, then continues the exit.
                self.place(l_close);
                self.temp_get(it);
                self.mark(for_pos);
                if is_await {
                    self.emit_async_close_normal((SiteKind::ForAwaitClose, for_pos));
                } else {
                    self.code().call_intrinsic(close_fn, 1);
                    self.code().drop();
                }
                self.end_completion(c, None, false);
            }
            if loop_used {
                // Closes the iterator.
                self.place(l_break);
                self.temp_get(it);
                self.mark(for_pos);
                if is_await {
                    self.emit_async_close_normal((SiteKind::ForAwaitClose, for_pos));
                } else {
                    self.code().call_intrinsic(close_fn, 1);
                    self.code().drop();
                }
            }
            self.place(l_end);
        } else {
            // `it key done`: `it key` to the store, which drops `it`.
            self.temp_get(it);
            self.code().for_in_next();
            self.code().jump_if_false(l_assign);
            self.code().drop();
            self.code().drop();
            self.place(l_break);
        }
        if scoped {
            self.pop_scope();
        }
        debug_assert_eq!(self.scope, outer);
        Ok(())
    }

    /// The temporaries of a loop's iterator record (D2732): the iterator (for-in: the key
    /// iterator) and, for for-of and for-await, its `next` method, named by the `for`.
    fn loop_temps(&mut self, is_of: bool, is_await: bool, for_pos: u32) -> (Atom, Option<Atom>) {
        let (it, next) = match (is_of, is_await) {
            (false, _) => ("for-in.iterator", None),
            (true, false) => ("for-of.iterator", Some("for-of.next")),
            (true, true) => ("for-await.iterator", Some("for-await.next")),
        };
        let it = self.temp(it, for_pos);
        (it, next.map(|n| self.temp(n, for_pos)))
    }

    /// A pattern in a declaration head (`for (const [k, v] of ...)`): skipped, as it
    /// runs after the right side.
    fn for_pattern(&mut self, b: Binding) -> PResult<Option<Target<'a>>> {
        if !matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
            return Ok(None);
        }
        let cp = self.skip_pattern(b)?;
        Ok(Some(Target::Pattern { cp, b }))
    }

    /// The binding of a declaration in a for-in/of head and its position.
    fn for_binding(&mut self) -> PResult<(Atom, u32)> {
        let pos = self.tok.start;
        let name = self.binding_name()?;
        self.next()?;
        Ok((name, pos))
    }

    /// For an expression head: whether it is for-of, found by a scan for the `of` or
    /// `in` that ends the left side (it must be known before the left side is
    /// emitted, to emit the `get_field value`). A head without either is for-in here;
    /// the parser reports the error at the real token.
    fn for_expr_head_is_of(&mut self) -> PResult<bool> {
        if self.tok.is_contextual(Kw::Async) && self.peek().is_contextual(Kw::Of) {
            return syntax(
                self.tok.start,
                "'for of' expression cannot start with 'async'",
            );
        }
        Ok(super::scan::for_lhs_ends_with_of(self.lx, self.tok))
    }

    /// `for (x = 1 of ...)`: QuickJS parses the initializer and reports at the token
    /// after it.
    fn for_initializer_error<T>(&mut self) -> PResult<T> {
        self.next()?;
        self.parse_assign(true)?;
        let msg = if self.tok.is_contextual(Kw::Of) {
            "a declaration in the head of a for-of loop can't have an initializer"
        } else if self.is(TokenKind::In) {
            "a declaration in the head of a for-in loop can't have an initializer"
        } else {
            "expected 'of' or 'in' in for control expression"
        };
        syntax(self.tok.start, msg)
    }
}
