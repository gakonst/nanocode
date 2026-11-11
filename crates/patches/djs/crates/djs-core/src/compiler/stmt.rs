//! Statements.
use crate::PushOut;

use alloc::vec::Vec;

use super::PResult;
use super::completion::Exit;
use super::func::{FuncKind, HandlerLabels, ScopeKind, VarKind};
use super::parser::{BlockKind, Parser, syntax};
use super::pattern::Binding;
use super::smap::SiteKind;
use super::smap_syntax::{DeclKind, NodeKind, Role};
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

impl Parser<'_, '_> {
    /// StatementListItem: declarations are allowed here, not in single-statement
    /// positions.
    pub(super) fn parse_statement_list_item(&mut self) -> PResult<()> {
        // A statement of the list (suspension maps, D2850).
        let start = self.tok.start;
        let role = if self.is(TokenKind::Function) || self.at_async_function() {
            Role::FuncDecl
        } else {
            Role::Stmt
        };
        self.parse_statement_list_item_inner()?;
        self.syn_part(role, start, self.prev_end);
        Ok(())
    }

    fn parse_statement_list_item_inner(&mut self) -> PResult<()> {
        // Statements that cannot be declarations (TS ones included) go straight to
        // `parse_statement`: nested blocks recurse through here, and the declaration
        // parsers inlined below made this frame 368 bytes in wasm, 70% of a nesting level
        // (D1941).
        if !matches!(
            self.tok.kind,
            TokenKind::Const
                | TokenKind::Function
                | TokenKind::Class
                | TokenKind::At
                | TokenKind::Import
                | TokenKind::Export
                | TokenKind::Enum
                | TokenKind::Ident
        ) {
            return self.parse_statement();
        }
        self.parse_declaration_item()
    }

    #[inline(never)]
    fn parse_declaration_item(&mut self) -> PResult<()> {
        if self.ts && self.ts_statement()? {
            if !self.ts_type_only {
                self.ts_note_stmt_end();
            }
            return Ok(());
        }
        match self.tok.kind {
            TokenKind::Const => self.parse_lexical_declaration(VarKind::Const)?,
            TokenKind::Ident if self.tok.is_contextual(Kw::Let) => {
                self.parse_lexical_declaration(VarKind::Let)?
            }
            TokenKind::Function => self.parse_function_declaration(self.tok.start, false)?,
            TokenKind::Ident if self.at_async_function() => {
                let start = self.tok.start;
                self.next()?;
                self.parse_function_declaration(start, true)?
            }
            TokenKind::Class => self.parse_class(true, Atom::NULL)?,
            TokenKind::At => return self.parse_decorated_statement(),
            TokenKind::Ident if self.at_using(false) => self.parse_using_declaration(false)?,
            TokenKind::Ident if self.at_await_using() => self.parse_using_declaration(true)?,
            TokenKind::Import | TokenKind::Export if self.strip && self.strip_at_module_item() => {
                return self.strip_module_item();
            }
            TokenKind::Import | TokenKind::Export
                if crate::MODULES
                    && self.at_module_top()
                    && self.scope == super::func::BODY_SCOPE
                    && self.strip_at_module_item() =>
            {
                return self.module_item();
            }
            _ => return self.parse_statement(),
        }
        self.ts_note_stmt_end();
        self.ts_type_only = false;
        Ok(())
    }

    pub(super) fn parse_statement(&mut self) -> PResult<()> {
        self.enter()?;
        let r = self.parse_statement_inner();
        self.leave();
        r
    }

    fn parse_statement_inner(&mut self) -> PResult<()> {
        if self.ts && self.ts_statement()? {
            if !self.ts_type_only {
                self.ts_note_stmt_end();
            }
            return Ok(());
        }
        self.parse_js_statement()?;
        self.ts_note_stmt_end();
        self.ts_type_only = false;
        Ok(())
    }

    fn parse_js_statement(&mut self) -> PResult<()> {
        let tok = self.tok;
        match tok.kind {
            TokenKind::LBrace => self.parse_block(),
            TokenKind::Semicolon => self.next(),
            TokenKind::Var => {
                self.syn_open(NodeKind::Decl(DeclKind::Var), tok.start);
                self.next()?;
                self.parse_var_declarations(false)?;
                self.expect_semicolon()?;
                self.syn_close();
                Ok(())
            }
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(Vec::new()),
            TokenKind::Do => self.parse_do_while(Vec::new()),
            TokenKind::For => self.parse_for(Vec::new()),
            TokenKind::Break | TokenKind::Continue => self.parse_break_continue(),
            TokenKind::Return => self.parse_return(),
            TokenKind::Throw => self.parse_throw(),
            TokenKind::Switch => self.parse_switch(Vec::new()),
            TokenKind::Try => self.parse_try(),
            #[cfg(feature = "dump")]
            TokenKind::Debugger if self.debugger_awaits && self.f.is_async => {
                self.parse_debugger_await()
            }
            TokenKind::Debugger => {
                self.next()?;
                self.code().debugger();
                self.expect_semicolon()
            }
            TokenKind::With => syntax(tok.start, "invalid keyword: with"),
            TokenKind::Import | TokenKind::Export if self.strip => {
                if self.strip_at_module_item() {
                    return self.strip_module_item();
                }
                self.parse_expression_statement()
            }
            // `import(...)` and `import.meta` start expression statements.
            TokenKind::Import if matches!(self.peek().kind, TokenKind::LParen | TokenKind::Dot) => {
                self.parse_expression_statement()
            }
            TokenKind::Import | TokenKind::Export | TokenKind::Enum => {
                let text = self.tok_text();
                syntax(tok.start, crate::text::cat!("unsupported keyword: ", text))
            }
            TokenKind::Const => syntax(
                tok.start,
                "lexical declarations can't appear in single-statement context",
            ),
            TokenKind::Ident
                if tok.is_contextual(Kw::Let) || self.at_using(false) || self.at_await_using() =>
            {
                syntax(
                    tok.start,
                    "lexical declarations can't appear in single-statement context",
                )
            }
            TokenKind::Function => syntax(
                tok.start,
                "function declarations can't appear in single-statement context",
            ),
            TokenKind::Ident if self.at_async_function() => syntax(
                tok.start,
                "function declarations can't appear in single-statement context",
            ),
            TokenKind::Class | TokenKind::At => syntax(
                tok.start,
                "class declarations can't appear in single-statement context",
            ),
            TokenKind::Ident
                if self.peek().kind == TokenKind::Colon
                    && !(tok.kw == Kw::Await && self.await_reserved()) =>
            {
                self.parse_labeled()
            }
            _ => self.parse_expression_statement(),
        }
    }

    /// `debugger;` with `CompileOptions::debugger_awaits` in an async function (D3050):
    /// the code and syntax of the statement `await debugger();`, where `debugger` is
    /// the global a host binds. To the suspension map and the dump writer it is an
    /// ordinary await of a call; its operand's text is the keyword, which a dump never
    /// copies (the site's operand is the awaited value), while a later `debugger;` in a
    /// continuation's text is copied as written and pauses again.
    #[cfg(feature = "dump")]
    fn parse_debugger_await(&mut self) -> PResult<()> {
        let start = self.tok.start;
        self.syn_open(NodeKind::Expr, start);
        self.syn_open(NodeKind::Await, start);
        let atom = self.ident_atom()?;
        self.next()?;
        let r = self.name_ref(atom, start);
        self.mark(start);
        self.code().name_get(r);
        self.code().undefined();
        self.code().call(0);
        self.syn_part(Role::Operand, start, self.prev_end);
        self.mark(start);
        self.site(SiteKind::Await, start);
        self.code().await_(0);
        self.syn_close();
        self.syn_part(Role::Operand, start, self.prev_end);
        self.emit_completion();
        self.expect_semicolon()?;
        self.syn_close();
        Ok(())
    }

    fn parse_expression_statement(&mut self) -> PResult<()> {
        let start = self.tok.start;
        self.syn_open(NodeKind::Expr, start);
        self.parse_expr(false)?;
        self.syn_part(Role::Operand, start, self.prev_end);
        self.emit_completion();
        self.expect_semicolon()?;
        self.syn_close();
        Ok(())
    }

    /// The completion value local, in the top-level code of a Script or eval (not in
    /// nested functions).
    fn completion_local(&self) -> Option<Atom> {
        if self.stack.is_empty() {
            self.completion
        } else {
            None
        }
    }

    /// The end of an expression statement: its value becomes the completion value of
    /// Script and eval code (as qjs's `eval_ret`), elsewhere it is dropped.
    fn emit_completion(&mut self) {
        match self.completion_local() {
            Some(a) => {
                let r = self.name_ref(a, self.prev_end);
                self.code().name_put(r);
            }
            None => self.code().drop(),
        }
    }

    /// `if`, loops, `switch` and `try` complete with `undefined` when their body
    /// produces no value (UpdateEmpty(…, undefined)): reset before them.
    pub(super) fn reset_completion(&mut self) {
        if let Some(a) = self.completion_local() {
            self.code().undefined();
            let r = self.name_ref(a, self.prev_end);
            self.code().name_put(r);
        }
    }

    fn parse_block(&mut self) -> PResult<()> {
        self.syn_open(NodeKind::Block, self.tok.start);
        self.next()?;
        self.push_scope(ScopeKind::Block)?;
        self.syn_scope();
        let using = self.begin_using_list(true);
        while !self.is(TokenKind::RBrace) {
            if self.is(TokenKind::Eof) {
                // qjs: a statement expected at the end of input (D2071).
                return self.unexpected();
            }
            self.parse_statement_list_item()?;
        }
        self.next()?;
        self.end_using_list(using);
        self.pop_scope();
        self.syn_close();
        Ok(())
    }

    // ---- Declarations ---------------------------------------------------------------

    fn parse_lexical_declaration(&mut self, kind: VarKind) -> PResult<()> {
        let dk = if kind == VarKind::Const {
            DeclKind::Const
        } else {
            DeclKind::Let
        };
        self.syn_open(NodeKind::Decl(dk), self.tok.start);
        self.next()?;
        self.parse_lexical_bindings(kind, false)?;
        self.expect_semicolon()?;
        self.syn_close();
        Ok(())
    }

    /// `let`/`const` bindings after the keyword. In a `for` head, stops before `in`/`of`
    /// without requiring a `const` initializer (the caller rejects for-in/of in P0).
    pub(super) fn parse_lexical_bindings(&mut self, kind: VarKind, in_for: bool) -> PResult<()> {
        loop {
            if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                self.parse_pattern_declarator(Binding::Lexical(kind), in_for)?;
                if !self.eat(TokenKind::Comma)? {
                    return Ok(());
                }
                continue;
            }
            let pos = self.tok.start;
            let name = self.binding_name()?;
            self.next()?;
            self.syn_part(Role::Target, pos, self.prev_end);
            self.ts_eat_marker(TokenKind::Bang)?;
            self.ts_annotation()?;
            let v = self.declare_lexical(name, kind, self.tok.start)?;
            if self.eat(TokenKind::Assign)? {
                let init = self.tok.start;
                self.parse_named(name, |p| p.parse_assign(in_for))?;
                self.syn_part(Role::Init, init, self.prev_end);
            } else if in_for && (self.is(TokenKind::In) || self.tok.is_contextual(Kw::Of)) {
                return Ok(());
            } else if kind == VarKind::Const {
                return syntax(self.tok.start, "missing initializer for const variable");
            } else {
                self.code().undefined();
            }
            let r = self.init_ref(name, pos);
            self.code().name_init(r);
            self.f.vars[v as usize].init_end = self.prev_end;
            if !self.eat(TokenKind::Comma)? {
                return Ok(());
            }
        }
    }

    /// `var` bindings after the keyword.
    pub(super) fn parse_var_declarations(&mut self, in_for: bool) -> PResult<()> {
        loop {
            if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                self.parse_pattern_declarator(Binding::Var, in_for)?;
                if !self.eat(TokenKind::Comma)? {
                    return Ok(());
                }
                continue;
            }
            let pos = self.tok.start;
            let name = self.binding_name()?;
            self.next()?;
            self.syn_part(Role::Target, pos, self.prev_end);
            self.ts_eat_marker(TokenKind::Bang)?;
            self.ts_annotation()?;
            self.declare_var(name, self.tok.start)?;
            if self.eat(TokenKind::Assign)? {
                let init = self.tok.start;
                self.parse_named(name, |p| p.parse_assign(in_for))?;
                self.syn_part(Role::Init, init, self.prev_end);
                let r = self.name_ref(name, pos);
                self.mark(pos);
                self.code().name_put(r);
            }
            if !self.eat(TokenKind::Comma)? {
                return Ok(());
            }
        }
    }

    /// `PATTERN = value` in a declaration: the value runs first (compiler/pattern.rs).
    fn parse_pattern_declarator(&mut self, b: Binding, in_for: bool) -> PResult<()> {
        let mark = self.pattern_vars.len();
        let start = self.tok.start;
        self.deferred_pattern(b, |p| {
            p.syn_part(Role::Target, start, p.prev_end);
            let typed = p.ts && p.is(TokenKind::Colon);
            p.ts_annotation()?;
            if typed && !p.is(TokenKind::Assign) {
                // As dquickts: `let [a]: T;` destructures `undefined`.
                p.code().undefined();
                return Ok(());
            }
            if !p.is(TokenKind::Assign) {
                // As qjs: without an initializer the pattern is not one.
                return syntax(start, "variable name expected");
            }
            p.next()?;
            let init = p.tok.start;
            p.parse_assign(in_for)?;
            p.syn_part(Role::Init, init, p.prev_end);
            Ok(())
        })?;
        let end = self.prev_end;
        self.end_pattern_bindings(mark, end);
        Ok(())
    }

    // ---- Control flow ---------------------------------------------------------------

    /// `( expr )`: the expression is the innermost node's part `role`.
    fn parse_paren_expr(&mut self, role: Role) -> PResult<()> {
        self.expect(TokenKind::LParen, "(")?;
        let start = self.tok.start;
        self.parse_expr(false)?;
        self.syn_part(role, start, self.prev_end);
        self.ts_semi = true;
        self.expect(TokenKind::RParen, ")")
    }

    fn parse_if(&mut self) -> PResult<()> {
        self.syn_open(NodeKind::If, self.tok.start);
        self.next()?;
        self.reset_completion();
        self.parse_paren_expr(Role::Test)?;
        let l_else = self.new_label();
        self.code().jump_if_false(l_else);
        let then = self.tok.start;
        self.parse_statement()?;
        self.syn_part(Role::Then, then, self.prev_end);
        if self.eat(TokenKind::Else)? {
            let l_end = self.new_label();
            self.code().jump(l_end);
            self.place(l_else);
            let other = self.tok.start;
            // `else if` chains loop here instead of recursing (docs/compiler.md 2.4).
            if self.is(TokenKind::If) {
                self.parse_if_chain(l_end)?;
            } else {
                self.parse_statement()?;
            }
            self.syn_part(Role::Else, other, self.prev_end);
            self.place(l_end);
        } else {
            self.place(l_else);
        }
        self.syn_close();
        Ok(())
    }

    /// The `if` of an `else if`, sharing the chain's end label.
    fn parse_if_chain(&mut self, l_end: crate::bytecode::Label) -> PResult<()> {
        // The `if`s of the chain, each the `else` of the one before (D2850).
        let mut ifs: Vec<u32> = Vec::new();
        loop {
            if self.syn_on() {
                ifs.push(self.tok.start);
            }
            self.syn_open(NodeKind::If, self.tok.start);
            self.next()?;
            self.parse_paren_expr(Role::Test)?;
            let l_else = self.new_label();
            self.code().jump_if_false(l_else);
            let then = self.tok.start;
            self.parse_statement()?;
            self.syn_part(Role::Then, then, self.prev_end);
            if !self.eat(TokenKind::Else)? {
                self.place(l_else);
                break;
            }
            self.code().jump(l_end);
            self.place(l_else);
            if !self.is(TokenKind::If) {
                let other = self.tok.start;
                self.parse_statement()?;
                self.syn_part(Role::Else, other, self.prev_end);
                break;
            }
        }
        // Inner `if`s end with the chain: each is its parent's `else`.
        for k in (0..ifs.len()).rev() {
            self.syn_close();
            if k > 0 {
                self.syn_part(Role::Else, ifs[k], self.prev_end);
            }
        }
        Ok(())
    }

    fn push_loop(
        &mut self,
        mut labels: Vec<Atom>,
        pos: u32,
    ) -> (crate::bytecode::Label, crate::bytecode::Label) {
        let l_break = self.new_label();
        let l_cont = self.new_label();
        self.push_block(&mut labels, BlockKind::Loop, l_break, Some(l_cont), pos);
        (l_break, l_cont)
    }

    fn parse_while(&mut self, labels: Vec<Atom>) -> PResult<()> {
        let pos = self.tok.start;
        self.syn_open(NodeKind::While, pos);
        self.next()?;
        self.reset_completion();
        let (l_break, l_cont) = self.push_loop(labels, pos);
        // Rotated (0086): `jump L_cont; L_body: body; L_cont: test; jump_if_true
        // L_body; L_break:`, one jump per iteration. Emitted in source order, then the
        // body is moved in front of the test.
        let l_body = self.new_label();
        self.code().jump(l_cont);
        let t0 = self.f.code.pos();
        self.place(l_cont);
        self.parse_paren_expr(Role::Test)?;
        let b0 = self.f.code.pos();
        self.place(l_body);
        let body = self.tok.start;
        self.parse_statement()?;
        self.syn_part(Role::Body, body, self.prev_end);
        let b1 = self.f.code.pos();
        self.rotate_code(t0, b0, b1);
        self.code().jump_if_true(l_body);
        self.place(l_break);
        self.pop_block();
        self.syn_close();
        Ok(())
    }

    fn parse_do_while(&mut self, labels: Vec<Atom>) -> PResult<()> {
        let pos = self.tok.start;
        self.syn_open(NodeKind::DoWhile, pos);
        self.next()?;
        self.reset_completion();
        let (l_break, l_cont) = self.push_loop(labels, pos);
        let l_top = self.new_label();
        self.place(l_top);
        let body = self.tok.start;
        self.parse_statement()?;
        self.syn_part(Role::Body, body, self.prev_end);
        self.expect(TokenKind::While, "while")?;
        self.place(l_cont);
        self.parse_paren_expr(Role::Test)?;
        self.code().jump_if_true(l_top);
        self.place(l_break);
        self.pop_block();
        // `do ... while (x)` takes an optional `;` even without a newline.
        self.eat(TokenKind::Semicolon)?;
        self.syn_close();
        Ok(())
    }

    /// `for (init; test; update) body`, laid out rotated (0086) as `init; jump L_test;
    /// L_body: body; L_cont: update; L_test: test; jump_if_true L_body; L_break:`, one
    /// jump per iteration (without a test: `init; L_body: body; L_cont: update; jump
    /// L_body`). The parts are emitted in source order, then the body is moved in
    /// front of the update and the test.
    fn parse_for(&mut self, labels: Vec<Atom>) -> PResult<()> {
        self.enter_n(super::parser::FOR_WEIGHT)?;
        self.syn_open(NodeKind::For, self.tok.start);
        let r = self.parse_for_inner(labels);
        if r.is_ok() {
            self.syn_close();
        }
        self.leave_n(super::parser::FOR_WEIGHT);
        r
    }

    fn parse_for_inner(&mut self, labels: Vec<Atom>) -> PResult<()> {
        let for_pos = self.tok.start;
        self.next()?;
        self.reset_completion();
        let is_await = self.tok.is_contextual(Kw::Await) && self.await_operator();
        if is_await {
            self.next()?;
        }
        if !self.is(TokenKind::LParen) {
            return syntax(self.tok.start, "expecting '('");
        }
        if is_await || !super::scan::for_head_is_c_style(self.lx, self.tok) {
            self.next()?;
            return self.parse_for_in_of(labels, for_pos, is_await);
        }
        self.expect(TokenKind::LParen, "(")?;
        let outer = self.scope;
        let init = self.tok.start;
        let (scoped, using) = self.parse_for_init()?;
        if init != self.tok.start {
            self.syn_part(Role::Init, init, self.prev_end);
        }
        self.expect(TokenKind::Semicolon, ";")?;
        // Per-iteration copies of `let` bindings (CreatePerIterationEnvironment) only
        // matter for captured bindings, which need closures (P1-1: `rebox`).
        let (l_break, l_cont) = self.push_loop(labels, for_pos);
        let l_test = self.new_label();
        let l_body = self.new_label();
        // Per-iteration copies of captured `let` bindings (docs/compiler.md 5.9):
        // after the init and before each update.
        let iter_scope = if scoped {
            Some(self.scope as u16)
        } else {
            None
        };
        if let Some(s) = iter_scope {
            self.code().iter_scope(s);
        }
        let has_test = !self.is(TokenKind::Semicolon);
        if has_test {
            self.code().jump(l_test);
        }
        let t0 = self.f.code.pos();
        self.place(l_test);
        if has_test {
            let test = self.tok.start;
            self.parse_expr(false)?;
            self.syn_part(Role::Test, test, self.prev_end);
        }
        self.expect(TokenKind::Semicolon, ";")?;
        let u0 = self.f.code.pos();
        self.place(l_cont);
        if let Some(s) = iter_scope {
            self.code().iter_scope(s);
        }
        if !self.is(TokenKind::RParen) {
            let update = self.tok.start;
            self.parse_expr(false)?;
            self.syn_part(Role::Update, update, self.prev_end);
            self.code().drop();
        }
        self.expect(TokenKind::RParen, ")")?;
        let b0 = self.f.code.pos();
        self.place(l_body);
        self.ts_semi = true;
        let body = self.tok.start;
        self.parse_statement()?;
        self.syn_part(Role::Body, body, self.prev_end);
        let b1 = self.f.code.pos();
        // [test][update][body] -> [update][body][test] -> [body][update][test]
        self.rotate_code(t0, u0, b1);
        self.rotate_code(t0, t0 + (b0 - u0), t0 + (b1 - u0));
        if has_test {
            self.code().jump_if_true(l_body);
        } else {
            self.code().jump(l_body);
        }
        self.place(l_break);
        self.pop_block();
        if let Some(r) = using {
            self.close_using(r);
        }
        if scoped {
            self.pop_scope();
        }
        debug_assert_eq!(self.scope, outer);
        Ok(())
    }

    /// The init clause of a C-style `for`: whether it opened a scope, and its `using`
    /// region. Out of line, so its locals are not part of `parse_for`'s frame while the
    /// loop body nests (D1941).
    #[inline(never)]
    fn parse_for_init(&mut self) -> PResult<(bool, Option<super::using::UsingRegion>)> {
        let mut scoped = false;
        let mut using = None;
        match self.tok.kind {
            TokenKind::Semicolon => {}
            TokenKind::Ident if self.at_using(false) || self.at_await_using() => {
                // The resources are disposed when the whole loop ends.
                let is_await = self.tok.kw == Kw::Await;
                let start = self.tok.start;
                if is_await {
                    self.next()?;
                }
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                let dk = if is_await {
                    DeclKind::AwaitUsing
                } else {
                    DeclKind::Using
                };
                self.syn_open(NodeKind::Decl(dk), start);
                scoped = true;
                let r = self.using_region_start(is_await, self.tok.start);
                self.using_env(&r);
                using = Some(r);
                self.parse_using_bindings(r.rec, is_await, true)?;
                self.syn_close();
            }
            TokenKind::Var => {
                self.syn_open(NodeKind::Decl(DeclKind::Var), self.tok.start);
                self.next()?;
                self.parse_var_declarations(true)?;
                self.syn_close();
            }
            TokenKind::Const => {
                let start = self.tok.start;
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                self.syn_open(NodeKind::Decl(DeclKind::Const), start);
                scoped = true;
                self.parse_lexical_bindings(VarKind::Const, true)?;
                self.syn_close();
            }
            TokenKind::Ident if self.tok.is_contextual(Kw::Let) => {
                let start = self.tok.start;
                self.next()?;
                self.push_scope(ScopeKind::For)?;
                self.syn_scope();
                self.syn_open(NodeKind::Decl(DeclKind::Let), start);
                scoped = true;
                self.parse_lexical_bindings(VarKind::Let, true)?;
                self.syn_close();
            }
            _ => {
                self.parse_expr(true)?;
                self.code().drop();
            }
        }
        Ok((scoped, using))
    }

    fn parse_break_continue(&mut self) -> PResult<()> {
        let is_break = self.is(TokenKind::Break);
        let pos = self.tok.start;
        self.next()?;
        let label = if self.is(TokenKind::Ident) && !self.tok.newline_before() {
            let a = self.ident_atom()?;
            self.next()?;
            Some(a)
        } else {
            None
        };
        let mut found = None;
        for (i, b) in self.blocks.iter().enumerate().rev() {
            if matches!(b.kind, BlockKind::Finally | BlockKind::Iter) {
                continue;
            }
            let target = match label {
                Some(l) if b.labels.contains(&l) => {
                    if is_break {
                        Some(b.break_label)
                    } else {
                        // `continue L` to a non-loop label is not found.
                        b.continue_label
                    }
                }
                Some(_) => continue,
                None if is_break && b.kind != BlockKind::Labeled => Some(b.break_label),
                None if !is_break && b.kind == BlockKind::Loop => b.continue_label,
                None => continue,
            };
            found = target.map(|_| i);
            break;
        }
        let Some(index) = found else {
            let msg = match (label, is_break) {
                (Some(_), _) => "break/continue label not found",
                (None, true) => "break must be inside loop or switch",
                (None, false) => "continue must be inside loop",
            };
            return syntax(pos, msg);
        };
        if is_break {
            self.blocks[index].used = true;
        }
        // Run the finally blocks and close the iterators being left, innermost first
        // (docs/compiler.md 7.4).
        self.emit_unwind(
            index + 1,
            Exit::Jump {
                block: index,
                cont: !is_break,
            },
        );
        self.expect_semicolon()
    }

    /// `async function` (no line break after `async`).
    pub(super) fn at_async_function(&self) -> bool {
        if !self.tok.is_contextual(Kw::Async) {
            return false;
        }
        let next = self.peek();
        next.kind == TokenKind::Function && !next.newline_before()
    }

    fn has_unwind(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::Finally | BlockKind::Iter))
    }

    /// `return f()`: qjs makes a call that a plain return follows into a tail call,
    /// which reports the position of the `return` statement instead of the call's
    /// (D2031). Matches its conditions: a normal function (not async, generator or
    /// derived constructor), no finally, iterator, `using` or try/catch to leave first.
    fn mark_tail_call(&mut self, pos: u32) {
        let Some(insn) = self.f.code.last_insn() else {
            return;
        };
        if self.f.tail_call != Some(insn.pc)
            || self.f.is_async
            || self.f.is_generator
            || self.f.ctor_return.is_some()
            || self.f.open_trys > 0
            || self.has_unwind()
        {
            return;
        }
        // The entry for the call: it is the last instruction, so no later entry exists
        // but one for the position after it (a `mark` with nothing emitted yet).
        let lines = &mut self.f.lines;
        match lines.iter().rposition(|&(pc, _)| pc <= insn.pc) {
            Some(i) if lines[i].0 == insn.pc => lines[i].1 = pos,
            i => lines.insert(i.map_or(0, |i| i + 1), (insn.pc, pos)),
        }
    }

    fn parse_return(&mut self) -> PResult<()> {
        let pos = self.tok.start;
        if matches!(self.f.kind, FuncKind::Script | FuncKind::Eval)
            || (crate::MODULES && self.f.kind == FuncKind::Module)
            || self.f.ns.is_some()
        {
            return syntax(pos, "return not in a function");
        }
        if self.f.kind == FuncKind::StaticBlock {
            return syntax(pos, "return in a static initializer block");
        }
        self.syn_open(NodeKind::Return, pos);
        self.next()?;
        let bare = self.is(TokenKind::Semicolon)
            || self.is(TokenKind::RBrace)
            || self.is(TokenKind::Eof)
            || self.tok.newline_before();
        if bare && !self.has_unwind() {
            self.emit_return_undef();
        } else {
            if bare {
                self.code().undefined();
            } else {
                let operand = self.tok.start;
                self.parse_expr(false)?;
                self.syn_part(Role::Operand, operand, self.prev_end);
                self.mark_tail_call(pos);
                if self.f.is_async && self.f.is_generator {
                    // An async generator awaits the operand of `return`.
                    self.mark(pos);
                    self.site(SiteKind::AsyncReturn, pos);
                    self.code().await_(0);
                }
            }
            self.emit_unwind(0, Exit::Return);
        }
        self.expect_semicolon()?;
        self.syn_close();
        Ok(())
    }

    /// `try` (docs/compiler.md 7.4, D1084), with a finally block's completion in
    /// `kind`/`value` (compiler/completion.rs):
    ///
    /// ```text
    /// L_try:   <try block>
    /// L_try_end:
    ///          finally: kind = 0 [; jump L_fin if a catch follows]
    ///          no finally: jump L_end
    /// L_catch: (handler for [L_try, L_try_end), stack: exc) <bind or drop>; <catch block>
    /// L_catch_end:
    ///          finally: kind = 0; no finally: jump L_end
    /// L_fin:   <finally block>
    ///          get kind; jump_if_false L_end; <the exits that entered it>
    ///          get value; value = undefined; throw
    /// L_exc:   (handler for the try or catch region) value = exc; kind = 1; jump L_fin
    /// L_end:
    /// ```
    fn parse_try(&mut self) -> PResult<()> {
        let try_pos = self.tok.start;
        self.syn_open(NodeKind::Try, try_pos);
        self.next()?;
        if !self.is(TokenKind::LBrace) {
            return syntax(self.tok.start, "expecting '{'");
        }
        let has_finally = super::scan::try_has_finally(self.lx, self.tok);
        let l_fin = self.new_label();
        let l_end = self.new_label();
        let fin = self.blocks.len();
        if has_finally {
            self.push_completion_block(BlockKind::Finally, l_fin, try_pos);
        }
        let l_try = self.new_label();
        let l_try_end = self.new_label();
        self.reset_completion();
        self.place(l_try);
        self.f.open_trys += 1;
        let block = self.tok.start;
        self.parse_block()?;
        self.syn_part(Role::Block, block, self.prev_end);
        self.f.open_trys -= 1;
        self.place(l_try_end);
        let has_catch = self.is(TokenKind::Catch);
        if has_finally {
            self.complete_normally(fin);
            if has_catch {
                self.code().jump(l_fin);
            }
        } else {
            self.code().jump(l_end);
        }
        let mut exc_region = (l_try, l_try_end);
        if has_catch {
            self.next()?;
            let l_catch = self.new_label();
            self.place(l_catch);
            self.f.handlers.push_out(HandlerLabels {
                start: l_try,
                end: l_try_end,
                target: l_catch,
                depth: 0,
                depth_from: None,
            });
            self.push_scope(ScopeKind::Catch)?;
            self.syn_scope();
            let mut body_scope = false;
            if self.eat(TokenKind::LParen)? {
                let param = self.tok.start;
                if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                    body_scope = true;
                    // A pattern binds like `let` (in its TDZ until initialized).
                    let mark = self.pattern_vars.len();
                    self.parse_pattern(Binding::Lexical(VarKind::Let))?;
                    self.syn_part(Role::CatchParam, param, self.prev_end);
                    self.ts_annotation()?;
                    let end = self.prev_end;
                    self.end_pattern_bindings(mark, end);
                    self.expect(TokenKind::RParen, ")")?;
                } else {
                    if self.tok.kind != TokenKind::Ident {
                        return syntax(self.tok.start, "identifier expected");
                    }
                    let pos = self.tok.start;
                    let name = self.binding_name()?;
                    self.next()?;
                    self.syn_part(Role::CatchParam, param, self.prev_end);
                    let v = self.f.add_var(name, VarKind::CatchParam, self.scope);
                    self.syn_decl(v, DeclKind::CatchParam);
                    self.ts_annotation()?;
                    self.expect(TokenKind::RParen, ")")?;
                    let r = self.init_ref(name, pos);
                    self.code().name_init(r);
                }
            } else {
                self.code().drop();
            }
            // The catch block's declarations share the clause's scope, so they
            // conflict with the parameter (`catch (e) { let e }`); after a pattern they
            // get a scope of their own (`ScopeKind::CatchBody`).
            let body = self.tok.start;
            self.syn_open(NodeKind::Block, body);
            // (Without a pattern the block's declarations are in the clause's scope.)
            self.syn_scope();
            self.expect(TokenKind::LBrace, "{")?;
            if body_scope {
                self.push_scope(ScopeKind::CatchBody)?;
                self.syn_scope();
            }
            self.reset_completion();
            let using = self.begin_using_list(true);
            self.f.open_trys += 1;
            while !self.is(TokenKind::RBrace) {
                if self.is(TokenKind::Eof) {
                    // qjs: a statement expected at the end of input (D2071).
                    return self.unexpected();
                }
                self.parse_statement_list_item()?;
            }
            self.f.open_trys -= 1;
            self.next()?;
            self.end_using_list(using);
            self.syn_close();
            self.syn_part(Role::CatchBody, body, self.prev_end);
            if body_scope {
                self.pop_scope();
            }
            self.pop_scope();
            let l_catch_end = self.new_label();
            self.place(l_catch_end);
            // (The finally block follows.)
            if has_finally {
                self.complete_normally(fin);
            } else {
                self.code().jump(l_end);
            }
            exc_region = (l_catch, l_catch_end);
        } else if !has_finally {
            return syntax(self.tok.start, "expecting catch or finally");
        }
        if has_finally {
            // The env stays while the finally block is parsed (its completion is not
            // kept in this frame, which every nested statement has, D1941), unlabeled,
            // so nothing targets or leaves it there.
            self.blocks[fin].kind = BlockKind::Labeled;
            self.expect(TokenKind::Finally, "finally")?;
            self.place(l_fin);
            self.save_completion_value(fin, try_pos);
            let block = self.tok.start;
            self.parse_block()?;
            self.syn_part(Role::Finally, block, self.prev_end);
            self.end_try_finally(l_fin, l_end, exc_region.0, exc_region.1);
        }
        self.place(l_end);
        self.syn_close();
        Ok(())
    }

    /// A finally block keeps the completion value of a normal try statement in Script
    /// and eval code: saved in a temporary of the try's completion (`blocks[fin]`) while
    /// it runs. (Out of line, as [`Parser::end_try_finally`]: this frame is in every
    /// nested statement's, D1941.)
    #[inline(never)]
    fn save_completion_value(&mut self, fin: usize, try_pos: u32) {
        let Some(a) = self.completion_local() else {
            return;
        };
        let t = self.temp("completion.saved", try_pos);
        let r = self.name_ref(a, self.prev_end);
        self.code().name_get(r);
        self.temp_put(t);
        self.reset_completion();
        self.blocks[fin]
            .completion
            .as_mut()
            .expect("a completion")
            .saved = t;
    }

    /// After a finally block: the completion value back, the dispatch, and the
    /// exception handler of the try (and catch) region `[start, end)`.
    #[inline(never)]
    fn end_try_finally(
        &mut self,
        l_fin: crate::bytecode::Label,
        l_end: crate::bytecode::Label,
        start: crate::bytecode::Label,
        end: crate::bytecode::Label,
    ) {
        let c = self
            .blocks
            .pop()
            .and_then(|e| e.completion)
            .expect("the finally env");
        if let Some(a) = self.completion_local().filter(|_| !c.saved.is_null()) {
            self.temp_get(c.saved);
            let r = self.name_ref(a, self.prev_end);
            self.code().name_put(r);
        }
        self.end_completion(&c, Some(l_end), true);
        let l_exc = self.new_label();
        self.place(l_exc);
        self.f.handlers.push_out(HandlerLabels {
            start,
            end,
            target: l_exc,
            depth: 0,
            depth_from: None,
        });
        self.complete_throwing(&c, l_fin);
    }

    fn parse_throw(&mut self) -> PResult<()> {
        let pos = self.tok.start;
        self.next()?;
        if self.tok.newline_before() {
            return syntax(self.tok.start, "line terminator not allowed after throw");
        }
        self.syn_open(NodeKind::Throw, pos);
        let operand = self.tok.start;
        self.parse_expr(false)?;
        self.syn_part(Role::Operand, operand, self.prev_end);
        self.mark(pos);
        self.code().throw();
        self.expect_semicolon()?;
        self.syn_close();
        Ok(())
    }

    /// `switch`: the discriminant stays on the stack during the case tests. Each test
    /// is placed right before its body in source order:
    /// `L_test_i: get d; <case>; strict_eq; jump_if_false L_test_i+1; L_body_i: <body>;
    /// jump L_body_i+1`, where `d` is the discriminant's temporary
    /// (`switch.discriminant@S`, D2732). When no case matches: jump to `default` or the
    /// end.
    fn parse_switch(&mut self, mut labels: Vec<Atom>) -> PResult<()> {
        let switch_pos = self.tok.start;
        self.syn_open(NodeKind::Switch, switch_pos);
        self.next()?;
        self.reset_completion();
        self.parse_paren_expr(Role::Discriminant)?;
        let d = self.temp("switch.discriminant", switch_pos);
        self.temp_put(d);
        self.expect(TokenKind::LBrace, "{")?;
        self.push_scope(ScopeKind::Switch)?;
        self.syn_scope();
        // Not in the statement lists of the clauses (a block in one is fine).
        let using = self.begin_using_list(false);
        let l_break = self.new_label();
        self.push_block(&mut labels, BlockKind::Switch, l_break, None, switch_pos);
        let mut next_test = self.new_label();
        let mut next_body: Option<crate::bytecode::Label> = None;
        let mut default_body = None;
        self.code().jump(next_test);
        while !self.is(TokenKind::RBrace) {
            let body = next_body.take().unwrap_or_else(|| self.new_label());
            match self.tok.kind {
                TokenKind::Case => {
                    let case_pos = self.tok.start;
                    self.next()?;
                    self.place(next_test);
                    next_test = self.new_label();
                    self.temp_get(d);
                    // The value the test compares is named by its `case`.
                    self.names_push(case_pos, &["binary.left"]);
                    let test = self.tok.start;
                    self.parse_expr(false)?;
                    self.syn_part(Role::Case, test, self.prev_end);
                    self.names_pop(1);
                    self.code().strict_eq();
                    self.code().jump_if_false(next_test);
                    self.ts_semi = true;
                    self.expect(TokenKind::Colon, ":")?;
                }
                TokenKind::Default => {
                    if default_body.is_some() {
                        return syntax(self.tok.start, "duplicate default");
                    }
                    self.syn_part(Role::Default, self.tok.start, self.tok.end);
                    self.next()?;
                    self.expect(TokenKind::Colon, ":")?;
                    default_body = Some(body);
                }
                _ => return syntax(self.tok.start, "invalid switch statement"),
            }
            self.place(body);
            while !matches!(
                self.tok.kind,
                TokenKind::Case | TokenKind::Default | TokenKind::RBrace
            ) {
                if self.is(TokenKind::Eof) {
                    // qjs: a statement expected at the end of input (D2071).
                    return self.unexpected();
                }
                self.parse_statement_list_item()?;
            }
            // Fall through into the next body, skipping its test.
            let nb = self.new_label();
            self.code().jump(nb);
            next_body = Some(nb);
        }
        self.next()?;
        // No case matched.
        self.place(next_test);
        self.code().jump(default_body.unwrap_or(l_break));
        if let Some(nb) = next_body {
            self.place(nb);
        }
        self.place(l_break);
        self.pop_block();
        self.end_using_list(using);
        self.pop_scope();
        self.syn_close();
        Ok(())
    }

    fn parse_labeled(&mut self) -> PResult<()> {
        let pos = self.tok.start;
        let mut labels = Vec::new();
        // The labels' starts (D2850): each label's body starts at the next one.
        let mut starts: Vec<u32> = Vec::new();
        while self.is(TokenKind::Ident) && self.peek().kind == TokenKind::Colon {
            let tok = self.tok;
            if self.syn_on() {
                starts.push(tok.start);
            }
            self.syn_open(NodeKind::Label, tok.start);
            self.syn_part(Role::Label, tok.start, tok.end);
            if tok.is_escaped_reserved() || tok.kw.is_strict_reserved() {
                return self.unexpected();
            }
            let a = self.ident_atom()?;
            if labels.contains(&a) || self.blocks.iter().any(|b| b.labels.contains(&a)) {
                return syntax(tok.start, "duplicate label name");
            }
            labels.push_out(a);
            self.next()?;
            self.next()?;
        }
        let body = self.tok.start;
        match self.tok.kind {
            TokenKind::While => self.parse_while(labels)?,
            TokenKind::Do => self.parse_do_while(labels)?,
            TokenKind::For => self.parse_for(labels)?,
            _ => {
                let l_break = self.new_label();
                self.push_block(&mut labels, BlockKind::Labeled, l_break, None, pos);
                if self.is(TokenKind::Function) {
                    return syntax(
                        self.tok.start,
                        "function declarations can't appear in single-statement context",
                    );
                }
                self.parse_statement()?;
                self.place(l_break);
                self.pop_block();
            }
        }
        for k in (0..starts.len()).rev() {
            let from = starts.get(k + 1).copied().unwrap_or(body);
            self.syn_part(Role::Body, from, self.prev_end);
            self.syn_close();
        }
        Ok(())
    }
}
