//! `yield`, `yield*` and the return path of a resumed generator (docs/compiler.md 7.7).
//!
//! A `yield` sits anywhere in an expression, with operands of the enclosing expression
//! on the stack, which the parser does not count. When `return(x)` resumes it, the
//! function must return `x` through its finally blocks, which run with an empty operand
//! stack. So the return path throws `x` inside a one-instruction handler region whose
//! handler (absolute depth 0) cuts the stack and then runs the ordinary return sequence. The region is the innermost one, so its handler goes first.

use super::PResult;
use super::expr::Expr;
use super::func::HandlerLabels;
use super::parser::{OpenIter, Parser, syntax};
use super::smap::SiteKind;
use crate::bytecode::intrinsics::intrinsic_id_of;
use crate::heap::atoms;
use crate::lexer::TokenKind;

impl Parser<'_, '_> {
    /// `yield`, `yield expr` or `yield* expr` in a generator, with the current token
    /// at `yield`.
    pub(super) fn parse_yield(&mut self, no_in: bool) -> PResult<Expr> {
        let tok = self.tok;
        if self.f.in_params {
            return syntax(tok.start, "yield in default expression");
        }
        self.next()?;
        let is_async = self.f.is_async;
        use super::smap_syntax::{NodeKind, Role};
        if self.is(TokenKind::Star) && !self.tok.newline_before() {
            self.syn_open(NodeKind::YieldStar, tok.start);
            self.next()?;
            let operand = self.tok.start;
            self.parse_assign(no_in)?;
            self.syn_part(Role::Operand, operand, self.prev_end);
            self.emit_yield_star(tok.start, is_async)?;
            self.syn_close();
            return Ok(Expr::Value);
        }
        self.syn_open(NodeKind::Yield, tok.start);
        // Without an operand: at the end of the expression or the line (qjs's set).
        let bare = self.tok.newline_before()
            || matches!(
                self.tok.kind,
                TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
                    | TokenKind::Comma
                    | TokenKind::Semicolon
                    | TokenKind::Colon
                    | TokenKind::Eof
            );
        if bare {
            self.code().undefined();
        } else {
            let operand = self.tok.start;
            self.parse_assign(no_in)?;
            self.syn_part(Role::Operand, operand, self.prev_end);
        }
        self.mark(tok.start);
        if is_async {
            self.site(SiteKind::AsyncYieldAwait, tok.start);
            self.code().await_(0);
            self.site(SiteKind::AsyncYield, tok.start);
        } else {
            self.site(SiteKind::Yield, tok.start);
        }
        self.code().yield_(0);
        let l_next = self.new_label();
        self.code().jump_if_false(l_next);
        if is_async {
            // AsyncGeneratorUnwrapYieldResumption: `return(x)` awaits x.
            self.site(SiteKind::AsyncYieldReturn, tok.start);
            self.code().await_(0);
        }
        self.emit_return_from_expr();
        self.place(l_next);
        self.syn_close();
        Ok(Expr::Value)
    }

    /// Returns the value on top of the stack from inside an expression: see the module
    /// docs.
    pub(super) fn emit_return_from_expr(&mut self) {
        // Close the open pattern iterators, innermost first: each throws `x` to a handler
        // that cuts the stack down to its iterator. (No suspension is on this path: the
        // name stack still describes the enclosing expression's operands.)
        let open: alloc::vec::Vec<OpenIter> = self.open_iters.iter().rev().copied().collect();
        for OpenIter { base, it } in open {
            let (l_throw, l_end, l_close) = (self.new_label(), self.new_label(), self.new_label());
            self.place(l_throw);
            self.code().throw();
            self.place(l_end);
            self.place(l_close);
            self.f.handlers.insert(
                0,
                HandlerLabels {
                    start: l_throw,
                    end: l_end,
                    target: l_close,
                    depth: 0,
                    depth_from: Some(base),
                },
            );
            // `x` -> `x`, closing a not-done iterator. It is done first: the pattern's
            // close handler, which covers this code, must not close it again if
            // `return()` throws.
            let l_skip = self.new_label();
            self.temp_get(it);
            self.code().dup();
            self.code().is_undefined();
            self.code().jump_if_true(l_skip);
            self.code().undefined();
            self.temp_put(it);
            self.code()
                .call_intrinsic(intrinsic_id_of!("IteratorClose"), 1);
            self.place(l_skip);
            self.code().drop();
        }
        let (l_throw, l_end, l_ret) = (self.new_label(), self.new_label(), self.new_label());
        self.place(l_throw);
        self.code().throw();
        self.place(l_end);
        self.place(l_ret);
        self.f.handlers.insert(
            0,
            HandlerLabels {
                start: l_throw,
                end: l_end,
                target: l_ret,
                depth: 0,
                depth_from: None,
            },
        );
        // Finally blocks and for-of loops (a for-of whose target is being stored
        // included) are left with a return completion.
        self.emit_unwind(0, super::completion::Exit::Return);
    }

    /// `yield* v` with `v` on the stack (ECMA-262 15.5.5): delegation to v's iterator,
    /// forwarding `next`/`throw`/`return`. The inner iterator and its `next` are
    /// temporaries (`yield*.iterator@S`, `yield*.next@S`, D2732). Sync
    /// generators pass the inner result objects through with `yield_star`; async ones
    /// await every inner result and yield its `value`.
    fn emit_yield_star(&mut self, pos: u32, is_async: bool) -> PResult<()> {
        let l_base = self.new_label();
        let l_loop = self.new_label();
        let l_check = self.new_label();
        let l_yield = self.new_label();
        let (r_start, r_end) = (self.new_label(), self.new_label());
        let l_throw = self.new_label();
        let l_nothrow = self.new_label();
        let l_retnone = self.new_label();
        let l_done = self.new_label();
        let l_end = self.new_label();
        let get_iterator = if is_async {
            intrinsic_id_of!("GetAsyncIterator")
        } else {
            intrinsic_id_of!("GetIterator")
        };
        self.mark(pos);
        self.code().call_intrinsic(get_iterator, 1);
        self.code().dup();
        self.code().get_field(atoms::NEXT, 0);
        let it = self.temp("yield*.iterator", pos);
        let next = self.temp("yield*.next", pos);
        self.temp_put(next);
        self.temp_put(it);
        self.place(l_base);
        // `recv` -> `next it recv` -> call.
        self.code().undefined();
        self.place(l_loop);
        self.temp_get(next);
        self.temp_get(it);
        self.code().rot3l();
        self.code().call(1);
        self.emit_inner_result(is_async);
        self.place(l_check);
        // `res`
        self.code().dup();
        self.code().get_field(atoms::DONE, 0);
        self.code().jump_if_true(l_done);
        self.place(l_yield);
        // Only the yield itself is covered: errors from reading `value` propagate.
        if is_async {
            self.code().get_field(atoms::VALUE, 0);
            self.place(r_start);
            self.site(SiteKind::YieldStar, pos);
            self.code().yield_(0);
        } else {
            self.place(r_start);
            self.site(SiteKind::YieldStar, pos);
            self.code().yield_star(0);
        }
        self.place(r_end);
        // `recv isReturn`: the throw handler covers the yield.
        self.f.handlers.insert(
            0,
            HandlerLabels {
                start: r_start,
                end: r_end,
                target: l_throw,
                depth: 0,
                depth_from: Some(l_base),
            },
        );
        self.code().jump_if_false(l_loop);
        // return(x): `x`.
        if is_async {
            self.site(SiteKind::YieldStarAwait, pos);
            self.code().await_(0);
        }
        self.emit_inner_method(it, atoms::RETURN, l_retnone);
        self.emit_inner_result(is_async);
        self.code().dup();
        self.code().get_field(atoms::DONE, 0);
        self.code().jump_if_false(l_yield);
        self.code().get_field(atoms::VALUE, 0);
        if is_async {
            self.site(SiteKind::YieldStarAwait, pos);
            self.code().await_(0);
        }
        self.emit_return_from_expr();
        // No `return` method: `x ret` -> return x.
        self.place(l_retnone);
        self.code().drop();
        if is_async {
            self.site(SiteKind::YieldStarAwait, pos);
            self.code().await_(0);
        }
        self.emit_return_from_expr();
        // throw(e) at the yield: `e`.
        self.place(l_throw);
        self.emit_inner_method(it, atoms::THROW, l_nothrow);
        self.emit_inner_result(is_async);
        self.code().jump(l_check);
        // No `throw` method: close the iterator, then a TypeError.
        self.place(l_nothrow);
        self.code().drop();
        self.code().drop();
        self.temp_get(it);
        self.mark(pos);
        if is_async {
            self.emit_async_close_normal((SiteKind::YieldStarAwait, pos));
        } else {
            self.code()
                .call_intrinsic(intrinsic_id_of!("IteratorClose"), 1);
            self.code().drop();
        }
        let msg: alloc::vec::Vec<u16> = "iterator does not have a throw method"
            .encode_utf16()
            .collect();
        let idx = self.string_const(&msg)?;
        self.code().push_const(idx);
        self.code()
            .call_intrinsic(intrinsic_id_of!("NewTypeError"), 1);
        self.mark(pos);
        self.code().throw();
        // Done: the result's value is the value of `yield*`.
        self.place(l_done);
        self.code().get_field(atoms::VALUE, 0);
        self.place(l_end);
        Ok(())
    }

    /// `v` -> `res` by calling the inner iterator's (temporary `it`) method `name`
    /// (GetMethod: undefined or null means none) with `v`; without one, jumps to
    /// `none` with `v m`.
    fn emit_inner_method(
        &mut self,
        it: crate::heap::Atom,
        name: crate::heap::Atom,
        none: crate::bytecode::Label,
    ) {
        self.temp_get(it);
        self.code().get_field(name, 0);
        self.code().dup();
        self.code().is_nullish();
        self.code().jump_if_true(none);
        // `v m` -> `m it v`
        self.code().swap();
        self.temp_get(it);
        self.code().swap();
        self.code().call(1);
    }

    /// An inner iterator result: awaited in async generators, then it must be an object.
    fn emit_inner_result(&mut self, is_async: bool) {
        if is_async {
            self.site(SiteKind::YieldStarAwait, self.prev_end);
            self.code().await_(0);
        }
        self.code().iter_check_object();
    }

    /// AsyncIteratorClose with a normal completion: `it` -> ``, awaiting at `site`.
    pub(super) fn emit_async_close_normal(&mut self, site: (SiteKind, u32)) {
        let l_skip = self.new_label();
        let l_closed = self.new_label();
        self.code().dup();
        self.code()
            .call_intrinsic(intrinsic_id_of!("AsyncIteratorReturnMethod"), 1);
        // `it ret`
        self.code().dup();
        self.code().is_undefined();
        self.code().jump_if_true(l_skip);
        self.code().swap();
        self.code().call(0);
        self.site(site.0, site.1);
        self.code().await_(0);
        self.code().iter_check_object();
        self.code().drop();
        self.code().jump(l_closed);
        self.place(l_skip);
        self.code().drop();
        self.code().drop();
        self.place(l_closed);
    }
}
