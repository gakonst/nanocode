//! Functions: declarations, expressions, arrows, parameters and bodies
//! (docs/compiler.md 5.5, 5.7, 6.3).
use crate::PushOut;

use super::PResult;
use super::decorators::DecParams;
use super::expr::Expr;
use super::func::{BODY_SCOPE, FuncKind, HandlerLabels, ScopeKind, VarKind};
use super::parser::{Parser, syntax};
use super::pattern::Binding;
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

impl Parser<'_, '_> {
    /// A binding name in a function context: parameters and function names have
    /// their own QuickJS messages for `eval`/`arguments`.
    fn function_binding_name(&mut self, restricted_msg: &'static str) -> PResult<Atom> {
        if matches!(self.tok.kw, Kw::Eval | Kw::Arguments) && self.tok.kind == TokenKind::Ident {
            return syntax(self.tok.start, restricted_msg);
        }
        self.binding_name()
    }

    /// `function name(params) { body }` as a declaration (hoisted), with the current
    /// token at `function`. `start` is the `async` keyword of an async function.
    pub(super) fn parse_function_declaration(&mut self, start: u32, is_async: bool) -> PResult<()> {
        self.next()?;
        let is_gen = self.eat(TokenKind::Star)?;
        if !self.is(TokenKind::Ident) || self.tok.kw == Kw::Yield {
            return syntax(self.tok.start, "function name expected");
        }
        let pos = self.tok.start;
        let name = self.function_binding_name("invalid function name in strict code")?;
        self.next()?;
        let flavor = (is_async, is_gen);
        let child = self.parse_function_rest(FuncKind::Normal, name, start, false, flavor)?;
        self.declare_function(name, child, pos)
    }

    /// A function expression in primary position, with the current token at
    /// `function`. `hint` names an anonymous one (`let f = function () {}`); `start` is
    /// the `async` keyword of an async function.
    pub(super) fn parse_function_expression(
        &mut self,
        hint: Atom,
        start: u32,
        is_async: bool,
    ) -> PResult<Expr> {
        self.next()?;
        let is_gen = self.eat(TokenKind::Star)?;
        let (name, own) = if self.is(TokenKind::Ident) {
            // The name of an expression is bound inside it: `await` is an operator in
            // an async one, an identifier in a plain one (even inside an async
            // function).
            let n = if self.tok.is_contextual(Kw::Await) {
                if is_async {
                    return syntax(self.tok.start, "expecting '('");
                }
                self.ident_atom()?
            } else {
                self.function_binding_name("invalid function name in strict code")?
            };
            self.next()?;
            (n, true)
        } else {
            (hint, false)
        };
        let flavor = (is_async, is_gen);
        let child = self.parse_function_rest(FuncKind::Normal, name, start, own, flavor)?;
        self.note_call(child);
        self.code().fclosure(child);
        if !own {
            self.last_anon_fn = Some((start, self.prev_end));
            if !hint.is_null() {
                self.named_fn = Some(self.f.children[child as usize]);
            }
        }
        Ok(Expr::Value)
    }

    /// A method, getter or setter of an object literal: parameters and body after the
    /// key. Getters take no parameters, setters exactly one. `flavor` is (async,
    /// generator).
    pub(super) fn parse_method_function(
        &mut self,
        kind: FuncKind,
        name: Atom,
        start: u32,
        flavor: (bool, bool),
    ) -> PResult<u32> {
        self.parse_method_function_dp(kind, name, start, flavor, None)
    }

    /// [`Parser::parse_method_function`] of a class method with legacy decorators: its
    /// parameter decorators and types go to `dp` (compiler/decorators.rs).
    pub(super) fn parse_method_function_dp(
        &mut self,
        kind: FuncKind,
        name: Atom,
        start: u32,
        flavor: (bool, bool),
        mut dp: Option<&mut DecParams>,
    ) -> PResult<u32> {
        self.enter()?;
        self.begin_function(kind)?;
        self.start_flavor(flavor)?;
        self.f.name = name;
        self.f.src_start = start;
        #[cfg(feature = "lazy")]
        {
            self.f.lazy_ok = dp.is_none();
            self.f.head = self.tok.start;
        }
        self.ts_fn_type_params()?;
        let params = self.tok.start;
        self.expect(TokenKind::LParen, "(")?;
        self.parse_params_dp(dp.as_deref_mut())?;
        self.syn_part(super::smap_syntax::Role::Params, params, self.prev_end);
        // qjs reports a wrong accessor arity at the `)`.
        let paren = self.prev_end - 1;
        let ok = match kind {
            FuncKind::Getter => self.f.param_count == 0,
            FuncKind::Setter => self.f.param_count == 1 && self.f.rest.is_none(),
            _ => true,
        };
        if !ok {
            return syntax(paren, "invalid number of arguments for getter or setter");
        }
        if let Some(dp) = dp
            && dp.metadata
            && self.is(TokenKind::Colon)
        {
            dp.ret = Some(self.ts_meta_annotation());
        }
        self.ts_return_annotation()?;
        self.parse_function_body()?;
        self.f.src_end = self.prev_end;
        let child = self.end_function()?;
        self.leave();
        Ok(child)
    }

    /// Parameters and body after the name. With `self_binding`, the name is bound
    /// inside the function to the function itself (named function expressions).
    pub(super) fn parse_function_rest(
        &mut self,
        kind: FuncKind,
        name: Atom,
        start: u32,
        self_binding: bool,
        flavor: (bool, bool),
    ) -> PResult<u32> {
        self.enter()?;
        self.begin_function(kind)?;
        self.start_flavor(flavor)?;
        self.f.name = name;
        self.f.src_start = start;
        if self_binding {
            let s = self
                .f
                .new_scope(BODY_SCOPE, ScopeKind::FuncName, self.tok.start);
            // The name scope sits outside the body: parameters and body declarations
            // shadow it.
            self.f.scopes[s as usize].parent = None;
            self.f.scopes[BODY_SCOPE as usize].parent = Some(s);
            let v = self.f.add_var(name, VarKind::FuncName, s);
            self.f.func_name_var = Some(v);
            self.syn_decl(v, super::smap_syntax::DeclKind::FuncName);
        }
        #[cfg(feature = "lazy")]
        {
            self.f.lazy_ok = self.may_wait(start);
            self.f.head = self.tok.start;
        }
        self.ts_fn_type_params()?;
        let params = self.tok.start;
        self.expect(TokenKind::LParen, "(")?;
        self.parse_params()?;
        self.syn_part(super::smap_syntax::Role::Params, params, self.prev_end);
        self.ts_return_annotation()?;
        self.parse_function_body()?;
        self.f.src_end = self.prev_end;
        let child = self.end_function()?;
        self.leave();
        Ok(child)
    }

    /// Whether a function (expression) starting at `start` may wait for its first call:
    /// one that is a call's argument or parenthesized usually runs at once, and stays
    /// eager (D2995). (Both are in the parent's text, as `resolve::lazy_roots` needs.)
    #[cfg(feature = "lazy")]
    fn may_wait(&self, start: u32) -> bool {
        start != self.arg_start && start != self.paren_inner
    }

    /// After a function expression or arrow `child`: whether a call follows it right
    /// away (`fn(`, `(fn)(`), which keeps it eager (D2990).
    fn note_call(&mut self, child: u32) {
        #[cfg(feature = "lazy")]
        self.note_call_lazy(child);
        #[cfg(not(feature = "lazy"))]
        let _ = child;
    }

    #[cfg(feature = "lazy")]
    fn note_call_lazy(&mut self, child: u32) {
        let end = if self.is(TokenKind::LParen) {
            self.tok.end
        } else if self.is(TokenKind::RParen) && self.peek().kind == TokenKind::LParen {
            self.peek().end
        } else {
            return;
        };
        let id = self.f.children[child as usize];
        if let Some(f) = self.done[id as usize].as_mut() {
            f.call_end = end;
        }
    }

    /// Formal parameters up to and including the `)`. Simple lists keep the
    /// parameters in their argument slots; as soon as a default or a rest parameter
    /// appears, every parameter becomes a local initialized in order, with TDZ between
    /// them (docs/compiler.md 5.5).
    pub(super) fn parse_params(&mut self) -> PResult<()> {
        self.parse_params_dp(None)
    }

    /// [`Parser::parse_params`] with the legacy parameter decorators and types going to
    /// `dp` (compiler/decorators.rs).
    pub(super) fn parse_params_dp(&mut self, dp: Option<&mut DecParams>) -> PResult<()> {
        self.f.in_params = true;
        self.parse_params_inner(dp)?;
        self.f.in_params = false;
        Ok(())
    }

    fn parse_params_inner(&mut self, mut dp: Option<&mut DecParams>) -> PResult<()> {
        let close = TokenKind::RParen;
        let mut seen_default = false;
        while !self.is(close) {
            if self.ts && self.ts_param_prefix(dp.as_deref_mut())? {
                continue;
            }
            let rest = self.eat(TokenKind::Ellipsis)?;
            if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                if let Some(dp) = dp.as_deref_mut()
                    && dp.metadata
                {
                    let m = self.ts_meta_pattern_annotation();
                    dp.types.push_out(if rest { m.rest_elem() } else { m });
                }
                self.parse_pattern_param(rest, &mut seen_default)?;
                if rest {
                    break;
                }
                if self.is(close) {
                    break;
                }
                self.expect(TokenKind::Comma, ",")?;
                continue;
            }
            if self.tok.kind != TokenKind::Ident
                || self.tok.is_escaped_reserved()
                || self.tok.kw.is_strict_reserved()
                || (self.tok.kw == Kw::Await && self.await_reserved())
            {
                return syntax(self.tok.start, "missing formal parameter");
            }
            let pos = self.tok.start;
            let name = self.function_binding_name("invalid argument name in strict code")?;
            self.next()?;
            // qjs reports duplicates at the token after the name.
            let v = self.declare_param(name, self.tok.start)?;
            self.ts_eat_marker(TokenKind::Question)?;
            if let Some(dp) = dp.as_deref_mut()
                && dp.metadata
            {
                let m = self.ts_meta_annotation();
                dp.types.push_out(if rest { m.rest_elem() } else { m });
            }
            self.ts_annotation()?;
            let index = self.f.vars[v as usize].param;
            let has_default = !rest && self.is(TokenKind::Assign);
            if rest || has_default {
                self.make_params_non_simple(index, pos);
            }
            if rest {
                self.f.rest = Some(index);
                self.code().rest(index);
            } else if has_default {
                seen_default = true;
                self.next()?;
                self.code().get_arg(index);
                self.parse_named(name, |p| p.parse_param_default())?;
            } else if !self.f.simple_params {
                self.code().get_arg(index);
            }
            if !self.f.simple_params {
                let r = self.init_ref(name, pos);
                self.code().name_init(r);
            }
            self.f.vars[v as usize].init_end = self.prev_end;
            if !rest && !seen_default {
                self.f.length += 1;
            }
            if rest || self.is(close) {
                break;
            }
            // qjs: `function f(a b)` expects a comma.
            self.expect(TokenKind::Comma, ",")?;
        }
        self.ts_params_end = self.prev_end;
        self.expect(close, ")")
    }

    /// The list turns non-simple at parameter `index`: the parameters before it are
    /// copied from their argument slots now, in order.
    fn make_params_non_simple(&mut self, index: u16, pos: u32) {
        if !self.f.simple_params {
            return;
        }
        self.f.simple_params = false;
        for i in 0..index {
            let p = self.f.params[i as usize];
            let pname = self.f.vars[p as usize].name;
            self.code().get_arg(i);
            let r = self.init_ref(pname, pos);
            self.code().name_init(r);
        }
    }

    /// `= default` of a parameter whose argument is on the stack.
    fn parse_param_default(&mut self) -> PResult<()> {
        let l = self.new_label();
        self.code().dup();
        self.code().is_undefined();
        self.code().jump_if_false(l);
        self.code().drop();
        self.parse_assign(false)?;
        self.place(l);
        Ok(())
    }

    /// A destructuring parameter (`{a, b}`, `[x] = []`, `...[y]`): an argument slot
    /// without a binding of its own; the pattern binds parameters.
    fn parse_pattern_param(&mut self, rest: bool, seen_default: &mut bool) -> PResult<()> {
        if self.f.param_count == u16::MAX {
            return syntax(self.tok.start, "too many parameters");
        }
        let index = self.f.param_count;
        self.f.param_count += 1;
        let pos = self.tok.start;
        self.make_params_non_simple(index, pos);
        let mark = self.pattern_vars.len();
        if rest {
            self.f.rest = Some(index);
            self.code().rest(index);
            self.parse_pattern(Binding::Param)?;
            self.ts_annotation()?;
        } else {
            let cp = self.skip_pattern(Binding::Param)?;
            self.ts_eat_marker(TokenKind::Question)?;
            self.ts_annotation()?;
            self.code().get_arg(index);
            if self.eat(TokenKind::Assign)? {
                *seen_default = true;
                self.parse_param_default()?;
            }
            let end = self.checkpoint();
            self.rewind(cp);
            self.parse_pattern(Binding::Param)?;
            self.rewind(end);
            if !*seen_default {
                self.f.length += 1;
            }
        }
        let end = self.prev_end;
        self.end_pattern_bindings(mark, end);
        Ok(())
    }

    /// `{ body }` of a function: the directive prologue, then the statements.
    pub(super) fn parse_function_body(&mut self) -> PResult<()> {
        self.expect(TokenKind::LBrace, "{")?;
        if self.f.is_generator {
            // After the parameters: their errors throw at the call (docs/compiler.md 7.7).
            self.code().generator_start(0);
        }
        self.code().enter_scope(BODY_SCOPE as u16);
        // With a non-simple parameter list the body's `var`s live in a scope of their
        // own; a `var` named like a parameter starts with its value (`Scope::copies`,
        // done by its `enter_scope`).
        let separate = if self.f.simple_params {
            None
        } else {
            let vs = self
                .f
                .new_scope(BODY_SCOPE, ScopeKind::Block, self.tok.start);
            self.f.var_scope = vs;
            self.scope = vs;
            self.code().enter_scope(vs as u16);
            Some(vs)
        };
        // strip-types: a function body starts a new statement context.
        let outer_semi = core::mem::replace(&mut self.ts_semi, false);
        // Directive prologue: everything is strict already, but `"use strict"` with
        // non-simple parameters is an error.
        let mut directives = true;
        let using = self.begin_using_list(true);
        while !self.is(TokenKind::RBrace) {
            if self.is(TokenKind::Eof) {
                // qjs: a statement expected at the end of input (D2071).
                return self.unexpected();
            }
            if directives {
                if self.is(TokenKind::String) {
                    let tok = self.tok;
                    let next = self.peek();
                    let ends = matches!(
                        next.kind,
                        TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
                    ) || next.newline_before();
                    let use_strict =
                        ends && !tok.has_escape() && self.lx.string_content(&tok) == b"use strict";
                    self.f.use_strict |= use_strict;
                    if use_strict && !self.f.simple_params {
                        return syntax(
                            tok.start,
                            "\"use strict\" not allowed in function with default or destructuring parameter",
                        );
                    }
                    if !ends {
                        directives = false;
                    }
                } else {
                    directives = false;
                }
            }
            self.parse_statement_list_item()?;
        }
        self.next()?;
        self.end_using_list(using);
        self.ts_semi = outer_semi;
        if let Some(vs) = separate {
            self.code().leave_scope(vs as u16);
            let mut copies = alloc::vec::Vec::new();
            for &v in &self.f.scopes[vs as usize].vars {
                let var = &self.f.vars[v as usize];
                if var.kind != VarKind::Var {
                    continue;
                }
                if let Some(p) = self.f.find_in_scope(BODY_SCOPE, var.name)
                    && self.f.vars[p as usize].kind == VarKind::Param
                {
                    copies.push_out((v, p));
                }
            }
            self.f.scopes[vs as usize].copies = copies;
            self.scope = BODY_SCOPE;
        }
        self.code().leave_scope(BODY_SCOPE as u16);
        if self.f.ctor_return.is_some() {
            // Falling off a derived constructor: its checked `this`, as the epilogue
            // returns it for `undefined` but without testing the value (0236).
            let pos = self.prev_end;
            self.emit_this(pos);
            self.code().return_();
        } else {
            self.emit_return_undef();
        }
        Ok(())
    }

    /// Makes the current (just begun) function async and/or a generator: its state
    /// local, and for async functions `async_start` and the start of the region whose
    /// exceptions reject the promise (docs/compiler.md 7.7; closed by
    /// [`Parser::end_async_region`]).
    pub(super) fn start_flavor(&mut self, (is_async, is_gen): (bool, bool)) -> PResult<()> {
        self.f.is_async = is_async;
        self.f.is_generator = is_gen;
        if !is_async && !is_gen {
            return Ok(());
        }
        let atom = crate::heap::atoms::STATE;
        let v = self.f.add_var(atom, VarKind::Internal, BODY_SCOPE);
        self.f.state_var = Some(v);
        if is_async && !is_gen {
            self.code().async_start(0);
            let l = self.new_label();
            self.place(l);
            self.f.async_region = Some(l);
        }
        Ok(())
    }

    /// Ends an async function's body: the catch-all handler that rejects the promise.
    /// It is the outermost region, so it goes last.
    pub(super) fn end_async_region(&mut self) {
        let Some(start) = self.f.async_region.take() else {
            return;
        };
        let end = self.new_label();
        self.place(end);
        self.code().async_reject(0);
        self.f.handlers.push_out(HandlerLabels {
            start,
            end,
            target: end,
            depth: 0,
            depth_from: None,
        });
    }

    /// Returns the value on top of the stack (after any finally blocks ran; nothing is
    /// under it at statement level).
    pub(super) fn emit_return_value(&mut self) {
        if let Some((epilogue, ret)) = self.f.ctor_return {
            // A derived constructor's result is checked by [[Construct]], outside any
            // try statement of the body: the epilogue (`emit_ctor_epilogue`).
            let r = self.name_ref(ret, self.prev_end);
            self.code().name_put(r);
            self.code().jump(epilogue);
            self.f.ctor_return_used = true;
            return;
        }
        if self.f.is_async && !self.f.is_generator {
            self.code().async_return(0);
        } else {
            self.code().return_();
        }
    }

    /// The end of a derived constructor, where its returns jump with the value in a
    /// local: an object is the result, `undefined` means the (checked) `this`, anything
    /// else is a TypeError.
    pub(super) fn emit_ctor_epilogue(&mut self) {
        let Some((epilogue, ret)) = self.f.ctor_return else {
            return;
        };
        if !self.f.ctor_return_used {
            return;
        }
        let pos = self.prev_end;
        self.place(epilogue);
        let r = self.name_ref(ret, pos);
        self.code().name_get(r);
        let l = self.new_label();
        self.mark(pos);
        self.code().check_ctor_return();
        self.code().dup();
        self.code().is_undefined();
        self.code().jump_if_false(l);
        self.code().drop();
        self.emit_this(pos);
        self.place(l);
        self.code().return_();
    }

    /// Returns `undefined`.
    pub(super) fn emit_return_undef(&mut self) {
        if self.f.ctor_return.is_some() {
            self.code().undefined();
            self.emit_return_value();
            return;
        }
        if self.f.is_async && !self.f.is_generator {
            self.code().undefined();
            self.code().async_return(0);
        } else {
            self.code().return_undef();
        }
    }

    /// An arrow function, with the current token at its parameters (an identifier or
    /// `(`) or at `async`. The caller checked that `=>` follows (for `async x`, that an
    /// identifier follows).
    /// `after_q`: the arrow is the `b` of `a ? b : c` (TypeScript: its expression body
    /// is too, as in tsc's `allowReturnTypeInArrowFunction`).
    pub(super) fn parse_arrow(&mut self, no_in: bool, hint: Atom, after_q: bool) -> PResult<Expr> {
        let start = self.tok.start;
        let is_async =
            self.tok.is_contextual(Kw::Async) && !matches!(self.peek().kind, TokenKind::Arrow);
        if is_async {
            self.next()?;
        }
        self.enter()?;
        self.begin_function(FuncKind::Arrow)?;
        self.start_flavor((is_async, false))?;
        self.f.name = hint;
        self.f.src_start = start;
        // `super()` and `this` in a constructor's arrows depend on the constructor's
        // state (D2990): those stay eager.
        #[cfg(feature = "lazy")]
        {
            self.f.head = start;
            self.f.after_q = after_q;
            self.f.owner = self.own_this_kind().unwrap_or(FuncKind::Normal);
            self.f.lazy_ok = !matches!(self.f.owner, FuncKind::ClassCtor | FuncKind::DerivedCtor)
                && self.may_wait(start);
        }
        self.ts_fn_type_params()?;
        let params = self.tok.start;
        if self.is(TokenKind::Ident) {
            self.f.in_params = true;
            if self.tok.kw == Kw::Await && self.await_reserved() {
                return syntax(self.tok.start, "missing formal parameter");
            }
            self.f.in_params = false;
            let name = self.function_binding_name("invalid argument name in strict code")?;
            self.next()?;
            self.syn_part(super::smap_syntax::Role::Params, params, self.prev_end);
            let v = self.declare_param(name, self.tok.start)?;
            self.f.vars[v as usize].init_end = self.prev_end;
            self.f.length = 1;
        } else {
            self.expect(TokenKind::LParen, "(")?;
            self.parse_params()?;
            self.syn_part(super::smap_syntax::Role::Params, params, self.prev_end);
            self.ts_arrow_return_annotation()?;
        }
        if !self.is(TokenKind::Arrow) || self.tok.newline_before() {
            return self.unexpected();
        }
        self.next()?;
        if self.is(TokenKind::LBrace) {
            self.parse_function_body()?;
        } else {
            self.code().enter_scope(BODY_SCOPE as u16);
            self.after_q = after_q;
            let body = self.tok.start;
            self.parse_assign(no_in)?;
            self.syn_part(super::smap_syntax::Role::Operand, body, self.prev_end);
            self.emit_return_value();
            self.code().leave_scope(BODY_SCOPE as u16);
        }
        self.f.src_end = self.prev_end;
        let child = self.end_function()?;
        self.leave();
        self.note_call(child);
        self.code().fclosure(child);
        self.last_anon_fn = Some((start, self.prev_end));
        if !hint.is_null() {
            self.named_fn = Some(self.f.children[child as usize]);
        }
        Ok(Expr::Value)
    }
}
