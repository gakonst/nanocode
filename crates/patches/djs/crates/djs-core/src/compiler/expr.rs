//! Expressions (docs/compiler.md 2.3, 3.7, 4.5).
use crate::PushOut;

use super::PResult;
use super::func::FuncKind;
use super::parser::{Parser, syntax};
use super::pattern::Binding;
use super::scan::Follow;
use super::smap::SiteKind;
use super::smap_syntax::{NodeKind, Role};
use crate::bytecode::{CodeBuf, Label, Phase1};
use crate::heap::Atom;
use crate::lexer::{Kw, TokenKind};

/// What an expression left on the stack, as far as the parser needs to know: whether
/// its last instruction loads a reference that an assignment, a call or `delete` can
/// turn into something else (docs/compiler.md 3.7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Expr {
    /// A plain value.
    Value,
    /// `name_get r` was the last instruction. `restricted`: `eval` or `arguments`,
    /// which strict code cannot assign.
    Name {
        r: u32,
        pos: u32,
        restricted: bool,
    },
    /// `get_field atom` was the last instruction (`pos`: the `.` or `[`).
    Field {
        atom: Atom,
        pos: u32,
    },
    /// `get_elem` was the last instruction.
    Elem {
        pos: u32,
    },
    /// A string literal (`push_const idx` was the last instruction).
    Str {
        idx: u32,
    },
    /// `obj.#x`: `get_private_field` was the last instruction (`obj sym` under it).
    Private {
        pos: u32,
    },
    /// `super.x`: `get_super_field atom` was the last instruction (`this base`).
    SuperField {
        atom: Atom,
        pos: u32,
    },
    /// `super[k]`: `get_super_elem` was the last instruction (`this base key`).
    SuperElem {
        pos: u32,
    },
    /// A bare private name, only valid as the left operand of `in` (`name_get r` of
    /// its symbol was the last instruction).
    PrivateName {
        pos: u32,
    },
    ObjectLit,
    ArrayLit,
    /// An optional chain: never an assignment target.
    OptChain,
    /// An optional chain under `delete` that already did the delete.
    Deleted,
    /// A parenthesized optional chain that is called (`(a?.b)()`): the callee and
    /// `this` are on the stack, as `prepare_call` leaves them.
    Callee,
}

/// How call arguments ended up on the stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Args {
    Count(u16),
    /// One array (there was a spread).
    Array,
}

/// Whose arguments `parse_args_as` parses: their slot names in suspension maps.
#[derive(Clone, Copy)]
pub(super) enum ArgsOf {
    Call,
    New,
    Super,
    Intrinsic,
}

impl ArgsOf {
    #[inline]
    fn arg(self) -> &'static str {
        match self {
            ArgsOf::Call => "call.arg",
            ArgsOf::New => "new.arg",
            ArgsOf::Super => "super.arg",
            ArgsOf::Intrinsic => "intrinsic.arg",
        }
    }

    /// The array the arguments go into from a spread on.
    #[inline]
    fn array(self) -> &'static str {
        match self {
            ArgsOf::New => "new.args",
            ArgsOf::Super => "super.args",
            ArgsOf::Call | ArgsOf::Intrinsic => "call.args",
        }
    }
}

/// An assignment target, after lvalue conversion removed the load.
#[derive(Clone, Copy, Debug)]
pub(super) enum LValue {
    Name {
        r: u32,
        pos: u32,
    },
    /// The object is on the stack.
    Field {
        atom: Atom,
        pos: u32,
    },
    /// The object and key are on the stack.
    Elem {
        pos: u32,
    },
    /// The object and the private symbol are on the stack.
    Private {
        pos: u32,
    },
    /// `this` and the super base are on the stack.
    SuperField {
        atom: Atom,
        pos: u32,
    },
    /// `this`, the super base and the key are on the stack.
    SuperElem {
        pos: u32,
    },
}

impl LValue {
    /// The slot names' constructs of the values the target keeps (suspension maps).
    pub fn stack_names(self) -> &'static [&'static str] {
        match self {
            LValue::Name { .. } => &[],
            LValue::Field { .. } => &["assign.object"],
            LValue::Elem { .. } | LValue::Private { .. } => &["assign.object", "assign.key"],
            LValue::SuperField { .. } => &["super.this", "super.base"],
            LValue::SuperElem { .. } => &["super.this", "super.base", "assign.key"],
        }
    }

    /// Values the target keeps on the stack until its store.
    pub fn refs(self) -> u8 {
        match self {
            LValue::Name { .. } => 0,
            LValue::Field { .. } => 1,
            LValue::Elem { .. } | LValue::Private { .. } | LValue::SuperField { .. } => 2,
            LValue::SuperElem { .. } => 3,
        }
    }
}

#[derive(Clone, Copy)]
enum BinOp {
    Nullish,
    Or,
    And,
    Pow,
    In,
    Simple(fn(&mut CodeBuf<Phase1>)),
}

/// `??` / `||` `&&` seen in an unparenthesized chain (they cannot be mixed).
const MIX_NULLISH: u8 = 1;
const MIX_ANDOR: u8 = 2;

fn binop(kind: TokenKind) -> Option<(u8, BinOp)> {
    use BinOp::Simple as S;
    use TokenKind::*;
    type B = CodeBuf<Phase1>;
    Some(match kind {
        QuestionQuestion => (1, BinOp::Nullish),
        PipePipe => (2, BinOp::Or),
        AmpAmp => (3, BinOp::And),
        Pipe => (4, S(B::bor)),
        Caret => (5, S(B::bxor)),
        Amp => (6, S(B::band)),
        Eq => (7, S(B::eq)),
        Ne => (7, S(B::ne)),
        StrictEq => (7, S(B::strict_eq)),
        StrictNe => (7, S(B::strict_ne)),
        Lt => (8, S(B::lt)),
        Gt => (8, S(B::gt)),
        Le => (8, S(B::le)),
        Ge => (8, S(B::ge)),
        Instanceof => (8, S(B::instanceof)),
        In => (8, BinOp::In),
        Shl => (9, S(B::shl)),
        Shr => (9, S(B::sar)),
        UShr => (9, S(B::shr)),
        Plus => (10, S(B::add)),
        Minus => (10, S(B::sub)),
        Star => (11, S(B::mul)),
        Slash => (11, S(B::div)),
        Percent => (11, S(B::mod_)),
        StarStar => (12, BinOp::Pow),
        _ => return None,
    })
}

/// The binary operator of a compound assignment.
fn compound_op(kind: TokenKind) -> Option<fn(&mut CodeBuf<Phase1>)> {
    use TokenKind::*;
    type B = CodeBuf<Phase1>;
    Some(match kind {
        PlusAssign => B::add,
        MinusAssign => B::sub,
        StarAssign => B::mul,
        SlashAssign => B::div,
        PercentAssign => B::mod_,
        StarStarAssign => B::pow,
        ShlAssign => B::shl,
        ShrAssign => B::sar,
        UShrAssign => B::shr,
        AmpAssign => B::band,
        PipeAssign => B::bor,
        CaretAssign => B::bxor,
        _ => return None,
    })
}

impl Parser<'_, '_> {
    /// Expression: comma lists.
    pub(super) fn parse_expr(&mut self, no_in: bool) -> PResult<Expr> {
        let (start, mark) = (self.tok.start, self.syn_mark());
        let mut e = self.parse_assign(no_in)?;
        if self.is(TokenKind::Comma) {
            self.syn_open_late(NodeKind::Sequence, start, mark, Role::Expr, self.prev_end);
            while self.is(TokenKind::Comma) {
                self.next()?;
                self.code().drop();
                let at = self.tok.start;
                self.parse_assign(no_in)?;
                self.syn_part(Role::Expr, at, self.prev_end);
            }
            self.syn_close();
            e = Expr::Value;
        }
        Ok(e)
    }

    /// AssignmentExpression.
    pub(super) fn parse_assign(&mut self, no_in: bool) -> PResult<Expr> {
        self.enter()?;
        let r = self.parse_assign_inner(no_in);
        self.leave();
        r
    }

    fn parse_assign_inner(&mut self, no_in: bool) -> PResult<Expr> {
        let after_q = core::mem::take(&mut self.after_q);
        if self.f.is_generator && self.tok.kw == Kw::Yield && !self.tok.has_escape() {
            return self.parse_yield(no_in);
        }
        if let Some(arrow) = self.try_arrow(no_in, after_q)? {
            return Ok(arrow);
        }
        // Where the assignment starts: it names the slots its target keeps.
        let start = self.tok.start;
        let mark = self.syn_mark();
        if self.starts_assignment_pattern(Follow::Assign) {
            // `[a, b] = [b, a]`: the right side runs first; its value is the result.
            self.deferred_pattern(Binding::Assign, |p| {
                p.syn_open_late(NodeKind::Assign, start, mark, Role::Left, p.prev_end);
                p.syn_part(Role::Op, p.tok.start, p.tok.end);
                p.next()?;
                let right = p.tok.start;
                p.parse_assign(no_in)?;
                p.syn_part(Role::Right, right, p.prev_end);
                p.code().dup();
                p.names_push(start, &["assign.value"]);
                Ok(())
            })?;
            self.syn_close();
            self.names_pop(1);
            return Ok(Expr::Value);
        }
        // `(f) = function () {}` names nothing: the target is not an identifier.
        let parenthesized = self.is(TokenKind::LParen);
        let lhs = self.parse_conditional(no_in, after_q)?;
        let op = self.tok;
        let assigns = op.kind == TokenKind::Assign
            || compound_op(op.kind).is_some()
            || matches!(
                op.kind,
                TokenKind::AmpAmpAssign
                    | TokenKind::PipePipeAssign
                    | TokenKind::QuestionQuestionAssign
            );
        if !assigns {
            return Ok(lhs);
        }
        self.syn_open_late(NodeKind::Assign, start, mark, Role::Left, self.prev_end);
        self.syn_part(Role::Op, op.start, op.end);
        let right = self.peek().start;
        let r = self.parse_assign_rest(no_in, start, lhs, op, parenthesized);
        if r.is_ok() {
            self.syn_part(Role::Right, right, self.prev_end);
            self.syn_close();
        }
        r
    }

    /// An assignment after its target `lhs`, at the operator `op`.
    fn parse_assign_rest(
        &mut self,
        no_in: bool,
        start: u32,
        lhs: Expr,
        op: crate::lexer::Token,
        parenthesized: bool,
    ) -> PResult<Expr> {
        if op.kind == TokenKind::Assign {
            let lv = self.lvalue(lhs, "invalid assignment left-hand side")?;
            self.names_push(start, lv.stack_names());
            self.next()?;
            if let LValue::Name { r, .. } = lv
                && !parenthesized
            {
                // NamedEvaluation: `f = function () {}` names the function `f`.
                let name = self.f.refs[r as usize].atom;
                self.parse_named(name, |p| p.parse_assign(no_in))?;
            } else {
                self.parse_assign(no_in)?;
            }
            self.names_pop(lv.stack_names().len());
            self.emit_store(lv, true);
            return Ok(Expr::Value);
        }
        if let Some(emit_op) = compound_op(op.kind) {
            let lv = self.lvalue(lhs, "invalid assignment left-hand side")?;
            self.names_push(start, lv.stack_names());
            self.emit_load(lv);
            self.names_push(start, &["assign.old"]);
            self.next()?;
            self.parse_assign(no_in)?;
            self.names_pop(lv.stack_names().len() + 1);
            self.mark(op.start);
            emit_op(self.code());
            self.emit_store(lv, true);
            return Ok(Expr::Value);
        }
        if matches!(
            op.kind,
            TokenKind::AmpAmpAssign | TokenKind::PipePipeAssign | TokenKind::QuestionQuestionAssign
        ) {
            // Short-circuit: the store only happens when the right side is evaluated.
            let lv = self.lvalue(lhs, "invalid assignment left-hand side")?;
            self.emit_load(lv);
            self.next()?;
            let l_keep = self.new_label();
            let l_end = self.new_label();
            self.code().dup();
            match op.kind {
                TokenKind::AmpAmpAssign => self.code().jump_if_false(l_keep),
                TokenKind::PipePipeAssign => self.code().jump_if_true(l_keep),
                _ => {
                    self.code().is_nullish();
                    self.code().jump_if_false(l_keep);
                }
            }
            self.code().drop();
            self.names_push(start, lv.stack_names());
            if let LValue::Name { r, .. } = lv
                && !parenthesized
            {
                // NamedEvaluation, as for `=`.
                let name = self.f.refs[r as usize].atom;
                self.parse_named(name, |p| p.parse_assign(no_in))?;
            } else {
                self.parse_assign(no_in)?;
            }
            self.names_pop(lv.stack_names().len());
            self.emit_store(lv, true);
            self.code().jump(l_end);
            self.place(l_keep);
            // Keep the current value; drop the references under it.
            for _ in 0..lv.refs() {
                self.code().nip();
            }
            self.place(l_end);
            return Ok(Expr::Value);
        }
        Ok(lhs)
    }

    /// Arrow functions start here (docs/compiler.md 3.1). Returns the arrow if there
    /// is one; async arrows and functions are rejected with a clear message.
    fn try_arrow(&mut self, no_in: bool, after_q: bool) -> PResult<Option<Expr>> {
        let tok = self.tok;
        let hint = match self.name_hint {
            Some((a, p)) if p == tok.start => a,
            _ => Atom::NULL,
        };
        match tok.kind {
            // In async functions `await` is an operator, never an arrow parameter.
            // The parameters of an arrow in an async function cannot await (the arrow
            // itself is not async; qjs's message).
            TokenKind::Ident
                if tok.is_contextual(Kw::Await)
                    && !self.f.is_async
                    && self.f.in_params
                    && self.f.kind == FuncKind::Arrow
                    && self.stack.last().is_some_and(|s| s.f.is_async) =>
            {
                return syntax(tok.start, "unexpected 'await' keyword");
            }
            TokenKind::Ident if tok.is_contextual(Kw::Await) && self.await_operator() => {}
            TokenKind::Ident => {
                let next = self.peek();
                if next.kind == TokenKind::Arrow && !next.newline_before() {
                    self.name_hint = None;
                    return self.parse_arrow(no_in, hint, after_q).map(Some);
                }
                if tok.is_contextual(Kw::Async) && !next.newline_before() {
                    let arrow = match next.kind {
                        // `async x => ...`
                        TokenKind::Ident if next.is_contextual(Kw::Await) => {
                            return syntax(next.start, "expecting '('");
                        }
                        TokenKind::Ident => true,
                        TokenKind::LParen => {
                            let mut lx = self.lx;
                            matches!(lx.next_token(), Ok(open) if super::scan::is_arrow_head(lx, open))
                                || (self.ts
                                    && self.ts_lookahead(|p| {
                                        p.next()?;
                                        Ok(p.ts_is_arrow_paren(false))
                                    }))
                        }
                        // `async <T>(x: T) => x`
                        TokenKind::Lt if self.ts => self.ts_lookahead(|p| {
                            p.next()?;
                            Ok(p.ts_is_generic_arrow())
                        }),
                        _ => false,
                    };
                    if arrow {
                        self.name_hint = None;
                        return self.parse_arrow(no_in, hint, after_q).map(Some);
                    }
                }
            }
            TokenKind::LParen
                if super::scan::is_arrow_head(self.lx, tok)
                    || (self.ts && self.ts_is_arrow_paren(after_q)) =>
            {
                self.name_hint = None;
                return self.parse_arrow(no_in, hint, after_q).map(Some);
            }
            // A generic arrow `<T>(x: T) => x`.
            TokenKind::Lt if self.ts && self.ts_is_generic_arrow() => {
                self.name_hint = None;
                return self.parse_arrow(no_in, hint, after_q).map(Some);
            }
            _ => {}
        }
        Ok(None)
    }

    /// Lvalue conversion: removes the load that `e` ended with.
    pub(super) fn lvalue(&mut self, e: Expr, msg: &'static str) -> PResult<LValue> {
        let pos = self.tok.start;
        let lv = match e {
            Expr::Name { r, pos, restricted } => {
                if restricted {
                    return syntax(pos, "invalid lvalue in strict mode");
                }
                LValue::Name { r, pos }
            }
            Expr::Field { atom, pos } => LValue::Field { atom, pos },
            Expr::Elem { pos } => LValue::Elem { pos },
            Expr::Private { pos } => LValue::Private { pos },
            Expr::SuperField { atom, pos } => LValue::SuperField { atom, pos },
            Expr::SuperElem { pos } => LValue::SuperElem { pos },
            _ => return syntax(pos, msg),
        };
        self.truncate_last();
        Ok(lv)
    }

    /// Loads the current value of a target for a compound operation, keeping the target
    /// on the stack under it.
    fn emit_load(&mut self, lv: LValue) {
        match lv {
            LValue::Name { r, pos } => {
                self.mark(pos);
                self.code().name_get(r);
            }
            LValue::Field { atom, pos } => {
                self.code().dup();
                self.mark(pos);
                self.code().get_field(atom, 0);
            }
            LValue::Elem { pos } => {
                self.mark(pos);
                self.code().get_elem2();
            }
            LValue::Private { pos } => {
                self.code().dup2();
                self.mark(pos);
                self.code().get_private_field();
            }
            LValue::SuperField { atom, pos } => {
                self.code().dup2();
                self.mark(pos);
                self.code().get_super_field(atom);
            }
            LValue::SuperElem { pos } => {
                for _ in 0..3 {
                    self.code().pick(2);
                }
                self.mark(pos);
                self.code().get_super_elem();
            }
        }
    }

    /// Before a stored value that stays on the stack: a copy under the references.
    fn emit_keep(&mut self, lv: LValue) {
        match lv.refs() {
            0 => self.code().dup(),
            1 => self.code().insert2(),
            2 => self.code().insert3(),
            _ => self.code().insert4(),
        }
    }

    /// Stores the value on top of the stack into the target. With `keep`, the value
    /// stays on the stack.
    pub(super) fn emit_store(&mut self, lv: LValue, keep: bool) {
        match lv {
            LValue::Name { r, pos } => {
                self.mark(pos);
                if keep {
                    self.code().name_set(r);
                } else {
                    self.code().name_put(r);
                }
            }
            LValue::Field { atom, pos } => {
                if keep {
                    self.code().insert2();
                }
                self.mark(pos);
                self.code().put_field(atom, 0);
            }
            LValue::Elem { pos } => {
                if keep {
                    self.code().insert3();
                }
                self.mark(pos);
                self.code().put_elem();
            }
            LValue::Private { pos } => {
                if keep {
                    self.code().insert3();
                }
                self.mark(pos);
                self.code().put_private_field();
            }
            LValue::SuperField { atom, pos } => {
                if keep {
                    self.code().insert3();
                }
                self.mark(pos);
                self.code().put_super_field(atom);
            }
            LValue::SuperElem { pos } => {
                if keep {
                    self.code().insert4();
                }
                self.mark(pos);
                self.code().put_super_elem();
            }
        }
    }

    /// `after_q`: the expression is itself the `b` of an enclosing `a ? b : c`, which the
    /// `c` here inherits (tsc's `allowReturnTypeInArrowFunction`, compiler/ts.rs).
    fn parse_conditional(&mut self, no_in: bool, after_q: bool) -> PResult<Expr> {
        let (start, mark) = (self.tok.start, self.syn_mark());
        let (e, _) = self.parse_binary(1, no_in)?;
        if !self.is(TokenKind::Question) {
            return Ok(e);
        }
        self.syn_open_late(
            NodeKind::Conditional,
            start,
            mark,
            Role::Test,
            self.prev_end,
        );
        self.next()?;
        let l_else = self.new_label();
        let l_end = self.new_label();
        self.code().jump_if_false(l_else);
        self.after_q = self.ts;
        let then = self.tok.start;
        self.parse_assign(false)?;
        self.syn_part(Role::Then, then, self.prev_end);
        self.expect(TokenKind::Colon, ":")?;
        self.code().jump(l_end);
        self.place(l_else);
        self.after_q = after_q;
        let other = self.tok.start;
        self.parse_assign(no_in)?;
        self.syn_part(Role::Else, other, self.prev_end);
        self.place(l_end);
        self.syn_close();
        Ok(Expr::Value)
    }

    /// Precedence climbing over the binary operators (docs/compiler.md 2.3). Returns
    /// the expression and which logical operators its unparenthesized chain contains.
    fn parse_binary(&mut self, min: u8, no_in: bool) -> PResult<(Expr, u8)> {
        // Where the left operand starts: it names its slot.
        let start = self.tok.start;
        let mark = self.syn_mark();
        // `#x in obj`: a private name only as the left operand of a relational `in`.
        let (mut lhs, mut lhs_unary) = if self.is(TokenKind::PrivateName)
            && min <= 8
            && !no_in
            && self.peek().kind == TokenKind::In
        {
            let pos = self.tok.start;
            let r = self.private_ref(pos)?;
            self.next()?;
            self.code().name_get(r);
            (Expr::PrivateName { pos }, false)
        } else {
            self.parse_unary()?
        };
        let mut mix = 0u8;
        loop {
            self.operator_tok();
            if self.ts
                && min <= 8
                && !self.tok.newline_before()
                && (self.tok.is_contextual(Kw::As) || self.tok.is_contextual(Kw::Satisfies))
            {
                // `x as T`, `x as const`, `x satisfies T`: a relational-level postfix,
                // so higher operators apply to the result (`x as T * 2` is
                // `(x as T) * 2`). The expression stays what it was: `(x as T) = 1`
                // assigns `x`.
                // ts-blank-space blanks from the end of the expression.
                let start = self.prev_end;
                self.next()?;
                self.ts_skip_type()?;
                self.ts_erase_kind(start, self.prev_end, super::ts::ER_AS);
                self.extend_anon_fn(start);
                lhs_unary = false;
                continue;
            }
            let Some((prec, op)) = binop(self.tok.kind) else {
                break;
            };
            if prec < min || (matches!(op, BinOp::In) && no_in) {
                break;
            }
            let op_tok = self.tok;
            if matches!(op, BinOp::Pow) && lhs_unary {
                return syntax(
                    op_tok.start,
                    "unparenthesized unary expression can't appear on the left-hand side of '**'",
                );
            }
            let logical = matches!(op, BinOp::Nullish | BinOp::Or | BinOp::And);
            let kind = if logical {
                NodeKind::Logical
            } else {
                NodeKind::Binary
            };
            self.syn_open_late(kind, start, mark, Role::Left, self.prev_end);
            self.syn_part(Role::Op, op_tok.start, op_tok.end);
            self.next()?;
            let right = self.tok.start;
            let l_short = self.new_label();
            match op {
                BinOp::Nullish => {
                    self.code().dup();
                    self.code().is_nullish();
                    self.code().jump_if_false(l_short);
                    self.code().drop();
                }
                BinOp::Or => {
                    self.code().dup();
                    self.code().jump_if_true(l_short);
                    self.code().drop();
                }
                BinOp::And => {
                    self.code().dup();
                    self.code().jump_if_false(l_short);
                    self.code().drop();
                }
                _ => {}
            }
            // `**` is right-associative.
            let next_min = if matches!(op, BinOp::Pow) {
                prec
            } else {
                prec + 1
            };
            // A logical operator dropped its left operand on this path.
            let keeps = !matches!(op, BinOp::Nullish | BinOp::Or | BinOp::And);
            if keeps {
                self.names_push(start, &["binary.left"]);
            }
            // A chain of `**` recurses once per operator (the other operators climb a
            // bounded number of levels): it counts toward the nesting limit, or a few
            // thousand of them overflow the native stack.
            let chained = matches!(op, BinOp::Pow);
            if chained {
                self.enter()?;
            }
            let (_, rmix) = self.parse_binary(next_min, no_in)?;
            if chained {
                self.leave();
            }
            self.syn_part(Role::Right, right, self.prev_end);
            if keeps {
                self.names_pop(1);
            }
            let this = match op {
                BinOp::Nullish => MIX_NULLISH,
                BinOp::Or | BinOp::And => MIX_ANDOR,
                _ => 0,
            };
            let all = mix | rmix;
            if (this == MIX_NULLISH && all & MIX_ANDOR != 0)
                || (this == MIX_ANDOR && all & MIX_NULLISH != 0)
            {
                return syntax(op_tok.start, "cannot mix ?? with && or ||");
            }
            mix = all | this;
            match op {
                BinOp::Nullish | BinOp::Or | BinOp::And => self.place(l_short),
                BinOp::Pow => {
                    self.mark(op_tok.start);
                    self.code().pow();
                }
                BinOp::In => {
                    self.mark(op_tok.start);
                    if matches!(lhs, Expr::PrivateName { .. }) {
                        self.code().private_in();
                    } else {
                        self.code().in_();
                    }
                }
                BinOp::Simple(emit) => {
                    self.mark(op_tok.start);
                    emit(self.code());
                }
            }
            self.syn_close();
            lhs = Expr::Value;
            lhs_unary = false;
        }
        Ok((lhs, mix))
    }

    /// UnaryExpression. Returns the expression and whether it was a unary operator
    /// (for the `**` rule).
    fn parse_unary(&mut self) -> PResult<(Expr, bool)> {
        self.enter()?;
        // A prefix operator is a node (D2850): `Op Operand`, an update's target as
        // `Parser::syn_target` makes it.
        let tok = self.tok;
        let kind = match tok.kind {
            TokenKind::Delete
            | TokenKind::Void
            | TokenKind::Typeof
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Tilde
            | TokenKind::Bang => Some(NodeKind::Unary),
            TokenKind::PlusPlus | TokenKind::MinusMinus => Some(NodeKind::Update),
            TokenKind::Ident if tok.is_contextual(Kw::Await) && self.await_operator() => {
                Some(NodeKind::Await)
            }
            _ => None,
        };
        let r = match kind {
            Some(kind) if self.syn_on() => {
                self.syn_open(kind, tok.start);
                if kind != NodeKind::Await {
                    self.syn_part(Role::Op, tok.start, tok.end);
                }
                let r = self.parse_unary_inner();
                if r.is_ok() {
                    self.syn_close();
                }
                r
            }
            _ => self.parse_unary_inner(),
        };
        self.leave();
        r
    }

    /// The operand of a prefix operator, from `start` to here.
    fn syn_operand(&mut self, update: bool, start: u32) {
        if update {
            self.syn_target(start, self.prev_end, Role::Operand);
        } else {
            self.syn_part(Role::Operand, start, self.prev_end);
        }
    }

    fn parse_unary_inner(&mut self) -> PResult<(Expr, bool)> {
        let tok = self.tok;
        // Set by `delete` for its operand: an optional chain ends in a delete.
        let delete_operand = core::mem::take(&mut self.delete_operand);
        match tok.kind {
            TokenKind::Delete => {
                self.next()?;
                self.delete_operand = true;
                let operand = self.tok.start;
                let (e, _) = self.parse_unary()?;
                self.syn_operand(false, operand);
                self.delete_operand = false;
                match e {
                    Expr::Name { .. } => {
                        return syntax(
                            tok.start,
                            "cannot delete a direct reference in strict mode",
                        );
                    }
                    Expr::Field { atom, pos } => {
                        self.truncate_last();
                        self.code().push_atom(atom);
                        self.mark(pos);
                        self.code().delete();
                    }
                    Expr::Elem { pos } => {
                        self.truncate_last();
                        self.mark(pos);
                        self.code().delete();
                    }
                    Expr::Private { .. } => {
                        return syntax(self.tok.start, "cannot delete a private class field");
                    }
                    Expr::SuperField { pos, .. } | Expr::SuperElem { pos } => {
                        // The references are evaluated, then it is a ReferenceError.
                        self.truncate_last();
                        let n = if matches!(e, Expr::SuperField { .. }) {
                            2
                        } else {
                            3
                        };
                        for _ in 0..n {
                            self.code().drop();
                        }
                        let msg = self.heap.atom_str("unsupported reference to 'super'")?;
                        self.mark(pos);
                        self.code().throw_error(msg, 1);
                        self.code().push_true();
                    }
                    // `delete a?.b`: the chain did the delete.
                    Expr::Deleted => {}
                    _ => {
                        self.code().drop();
                        self.code().push_true();
                    }
                }
            }
            TokenKind::Void => {
                self.next()?;
                let operand = self.tok.start;
                self.parse_unary()?;
                self.syn_operand(false, operand);
                self.code().drop();
                self.code().undefined();
            }
            TokenKind::Typeof => {
                self.next()?;
                let operand = self.tok.start;
                let (e, _) = self.parse_unary()?;
                self.syn_operand(false, operand);
                if let Expr::Name { r, pos, .. } = e {
                    self.truncate_last();
                    self.mark(pos);
                    self.code().name_typeof(r);
                }
                self.code().typeof_();
            }
            TokenKind::Plus | TokenKind::Minus | TokenKind::Tilde | TokenKind::Bang => {
                self.next()?;
                let operand = self.tok.start;
                self.parse_unary()?;
                self.syn_operand(false, operand);
                self.mark(tok.start);
                match tok.kind {
                    TokenKind::Plus => self.code().plus(),
                    TokenKind::Minus => self.code().neg(),
                    TokenKind::Tilde => self.code().bnot(),
                    _ => self.code().not(),
                }
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                self.next()?;
                let operand = self.tok.start;
                let (e, _) = self.parse_unary()?;
                self.syn_operand(true, operand);
                let lv = self.lvalue(e, "invalid increment/decrement operand")?;
                self.emit_load(lv);
                self.mark(tok.start);
                if tok.kind == TokenKind::PlusPlus {
                    self.code().inc();
                } else {
                    self.code().dec();
                }
                self.emit_store(lv, true);
                // An UpdateExpression, not a UnaryExpression: `++a ** 2` is valid.
                return Ok((Expr::Value, false));
            }
            // The parameters of an arrow in an async function cannot await (the arrow
            // itself is not async; qjs's message).
            TokenKind::Ident
                if tok.is_contextual(Kw::Await)
                    && !self.f.is_async
                    && self.f.in_params
                    && self.f.kind == FuncKind::Arrow
                    && self.stack.last().is_some_and(|s| s.f.is_async) =>
            {
                return syntax(tok.start, "unexpected 'await' keyword");
            }
            TokenKind::Ident if tok.is_contextual(Kw::Await) && self.await_operator() => {
                if self.f.in_params {
                    return syntax(tok.start, "await in default expression");
                }
                self.next()?;
                let operand = self.tok.start;
                self.parse_unary()?;
                self.syn_operand(false, operand);
                self.mark(tok.start);
                self.site(SiteKind::Await, tok.start);
                self.code().await_(0);
            }
            // A type assertion `<T>x` (TypeScript): the operand stays what it was, so
            // `(<T>x)++` updates `x`.
            TokenKind::Lt if self.ts => {
                self.next()?;
                self.ts_skip_type()?;
                if !self.is(TokenKind::Gt) {
                    return self.ts_error();
                }
                self.next()?;
                self.ts_erase(tok.start);
                // NamedEvaluation sees through the assertion, as through parentheses.
                let operand = self.tok.start;
                if let Some((a, p)) = self.name_hint
                    && p == tok.start
                {
                    self.name_hint = Some((a, operand));
                }
                let (e, _) = self.parse_unary()?;
                if self.last_anon_fn == Some((operand, self.prev_end)) {
                    self.last_anon_fn = Some((tok.start, self.prev_end));
                }
                return Ok((e, true));
            }
            _ => return Ok((self.parse_postfix(delete_operand)?, false)),
        }
        Ok((Expr::Value, true))
    }

    fn parse_postfix(&mut self, delete_operand: bool) -> PResult<Expr> {
        let (start, mark) = (self.tok.start, self.syn_mark());
        let e = self.parse_lhs(delete_operand)?;
        let tok = self.tok;
        if !matches!(tok.kind, TokenKind::PlusPlus | TokenKind::MinusMinus) || tok.newline_before()
        {
            return Ok(e);
        }
        let lv = self.lvalue(e, "invalid increment/decrement operand")?;
        self.syn_open_late(NodeKind::Update, start, mark, Role::Operand, self.prev_end);
        self.syn_part(Role::Op, tok.start, tok.end);
        self.next()?;
        self.syn_close();
        // Leaves the old (numeric) value: `x++` is `old = +x; x = old + 1; old`.
        self.emit_load(lv);
        self.code().to_numeric();
        self.emit_keep(lv);
        self.mark(tok.start);
        if tok.kind == TokenKind::PlusPlus {
            self.code().inc();
        } else {
            self.code().dec();
        }
        self.emit_store(lv, false);
        Ok(Expr::Value)
    }

    /// LeftHandSideExpression: `new`, member access, calls, optional chains.
    pub(super) fn parse_lhs(&mut self, delete_operand: bool) -> PResult<Expr> {
        // The whole content of parentheses (`(a?.b)()` keeps the reference).
        let whole = self.paren_inner == self.tok.start;
        // Where the chain starts: it names the slots of its calls and indexing.
        let start = self.tok.start;
        let mark = self.syn_mark();
        let mut e = if self.is(TokenKind::New) {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        // Short-circuit pads of an optional chain, indexed by stack depth - 1.
        let mut pads: [Option<Label>; 2] = [None, None];
        loop {
            let tok = self.tok;
            // Where the chain so far ends (D2850).
            let before = self.prev_end;
            match tok.kind {
                TokenKind::Dot => {
                    let member = NodeKind::Member {
                        computed: false,
                        optional: false,
                    };
                    self.syn_open_late(member, start, mark, Role::Object, before);
                    self.next()?;
                    let name = self.tok.start;
                    e = self.parse_dot_name(tok.start)?;
                    self.syn_part(Role::Property, name, self.prev_end);
                    self.syn_close();
                }
                TokenKind::LBracket => {
                    let member = NodeKind::Member {
                        computed: true,
                        optional: false,
                    };
                    self.syn_open_late(member, start, mark, Role::Object, before);
                    self.next()?;
                    e = self.parse_index(tok.start, start)?;
                    self.syn_part(Role::Property, tok.start, self.prev_end);
                    self.syn_close();
                }
                TokenKind::LParen => {
                    // qjs compiles a call of the name `eval` to its own opcode, never
                    // a tail call.
                    let eval = matches!(e, Expr::Name { r, restricted: true, .. }
                        if self.heap.atom_name(self.f.refs[r as usize].atom) == "eval");
                    let call = NodeKind::Call { optional: false };
                    self.syn_open_late(call, start, mark, Role::Callee, before);
                    self.prepare_call(e, start);
                    let args = self.parse_args()?;
                    self.mark(tok.start);
                    if matches!(args, Args::Count(_)) && !eval {
                        self.f.tail_call = Some(self.f.code.pos());
                    }
                    self.emit_call(args);
                    self.syn_close();
                    e = Expr::Value;
                }
                TokenKind::QuestionDot => {
                    self.next()?;
                    match self.tok.kind {
                        TokenKind::LParen => {
                            let call = NodeKind::Call { optional: true };
                            self.syn_open_late(call, start, mark, Role::Callee, before);
                            // `f?.()` / `o.m?.()`: test the callee, keep `this`.
                            self.prepare_call(e, start);
                            let pad = *pads[1].get_or_insert_with(|| self.f.code.new_label());
                            self.code().swap();
                            self.code().dup();
                            self.code().is_nullish();
                            self.code().jump_if_true(pad);
                            self.code().swap();
                            let paren = self.tok.start;
                            let args = self.parse_args()?;
                            self.mark(paren);
                            self.emit_call(args);
                            self.syn_close();
                            e = Expr::Value;
                        }
                        _ => {
                            let member = NodeKind::Member {
                                computed: self.is(TokenKind::LBracket),
                                optional: true,
                            };
                            self.syn_open_late(member, start, mark, Role::Object, before);
                            let pad = *pads[0].get_or_insert_with(|| self.f.code.new_label());
                            self.code().dup();
                            self.code().is_nullish();
                            self.code().jump_if_true(pad);
                            let name = self.tok.start;
                            if self.is(TokenKind::LBracket) {
                                let pos = self.tok.start;
                                self.next()?;
                                e = self.parse_index(pos, start)?;
                            } else {
                                e = self.parse_dot_name(tok.start)?;
                            }
                            self.syn_part(Role::Property, name, self.prev_end);
                            self.syn_close();
                        }
                    }
                }
                // A non-null assertion `x!` (TypeScript).
                TokenKind::Bang if self.ts && !tok.newline_before() => {
                    let end = self.prev_end;
                    self.next()?;
                    self.ts_erase(tok.start);
                    self.extend_anon_fn(end);
                }
                // Type arguments: `f<T>(x)`, `` f<T>`t` ``, `f<T>` (or a comparison).
                k if self.ts && super::ts::is_lt(k) => {
                    if !self.ts_type_args_in_expr() {
                        break;
                    }
                }
                TokenKind::NoSubstTemplate | TokenKind::TemplateHead => {
                    if pads.iter().any(|p| p.is_some()) {
                        return syntax(
                            tok.start,
                            "template literal cannot appear in an optional chain",
                        );
                    }
                    let tagged = NodeKind::TaggedTemplate;
                    self.syn_open_late(tagged, start, mark, Role::Tag, before);
                    self.prepare_call(e, start);
                    let argc = self.parse_tagged_template()?;
                    self.mark(tok.start);
                    self.f.tail_call = Some(self.f.code.pos());
                    self.names_pop(2);
                    self.code().call(argc);
                    self.syn_close();
                    e = Expr::Value;
                }
                _ => break,
            }
        }
        if pads.iter().any(|p| p.is_some()) {
            // `(a?.b)()` and `(a?.b)?.()` call with `this` = `a`.
            let callee = whole && !delete_operand && self.is(TokenKind::RParen) && {
                let (t1, t2) = self.peek2();
                t1.kind == TokenKind::LParen
                    || (t1.kind == TokenKind::QuestionDot && t2.kind == TokenKind::LParen)
            };
            if callee {
                self.prepare_call(e, start);
                // The call follows the parentheses, past the chain's pads.
                self.names_pop(2);
            }
            if delete_operand {
                // `delete a?.b.c`: delete the chain's last reference.
                match e {
                    Expr::Field { atom, pos } => {
                        self.truncate_last();
                        self.code().push_atom(atom);
                        self.mark(pos);
                        self.code().delete();
                    }
                    Expr::Elem { pos } => {
                        self.truncate_last();
                        self.mark(pos);
                        self.code().delete();
                    }
                    Expr::Private { .. } => {
                        return syntax(self.tok.start, "cannot delete a private class field");
                    }
                    _ => {
                        self.code().drop();
                        self.code().push_true();
                    }
                }
            }
            // Short-circuited: drop what the chain had pushed and produce `undefined`
            // (`true` for a delete).
            let l_end = self.new_label();
            self.code().jump(l_end);
            if let Some(p2) = pads[1] {
                self.place(p2);
                self.code().drop();
            }
            let p1 = pads[0].unwrap_or_else(|| self.f.code.new_label());
            self.place(p1);
            self.code().drop();
            if delete_operand {
                self.code().push_true();
            } else {
                self.code().undefined();
                if callee {
                    self.code().undefined();
                }
            }
            self.place(l_end);
            return Ok(if delete_operand {
                Expr::Deleted
            } else if callee {
                Expr::Callee
            } else {
                Expr::OptChain
            });
        }
        Ok(e)
    }

    /// Before the arguments of a call: the callee and `this` on the stack.
    /// The call's slot names (`call.callee call.this`, named by `at`, where the callee
    /// starts) stay until `emit_call`.
    pub(super) fn prepare_call(&mut self, e: Expr, at: u32) {
        match e {
            Expr::Field { atom, pos } => {
                self.truncate_last();
                self.mark(pos);
                self.code().get_method(atom, 0);
                self.names_push(at, &["call.callee", "call.this"]);
            }
            Expr::Elem { pos } => {
                self.truncate_last();
                self.mark(pos);
                self.code().get_elem_method();
                self.names_push(at, &["call.callee", "call.this"]);
            }
            Expr::Private { pos } => {
                // `obj sym` -> `f obj`.
                self.truncate_last();
                self.code().pick(1);
                self.code().rot3r();
                self.mark(pos);
                self.code().get_private_field();
                self.code().swap();
                self.names_push(at, &["call.callee", "call.this"]);
            }
            // `super.m()` calls with `this` (read again: nothing can change it).
            Expr::SuperField { pos, .. } | Expr::SuperElem { pos } => {
                self.names_push(at, &["call.callee"]);
                self.emit_this(pos);
                self.names_push(at, &["call.this"]);
            }
            // `(a?.b)()`: the chain left both.
            Expr::Callee => self.names_push(at, &["call.callee", "call.this"]),
            _ => {
                self.names_push(at, &["call.callee"]);
                self.code().undefined();
                self.names_push(at, &["call.this"]);
                #[cfg(test)]
                if self.test_swap_call {
                    self.code().swap();
                    self.names_replace(2, at, &["call.this", "call.callee"]);
                }
            }
        }
    }

    /// The property name after `.` (or `?.`).
    pub(super) fn parse_dot_name(&mut self, pos: u32) -> PResult<Expr> {
        if self.is(TokenKind::PrivateName) {
            let tok = self.tok;
            let r = self.private_ref(tok.start)?;
            self.next()?;
            self.code().name_get(r);
            self.mark(pos);
            self.code().get_private_field();
            return Ok(Expr::Private { pos });
        }
        if !self.tok.kind.is_identifier_name() {
            return syntax(self.tok.start, "expecting field name");
        }
        let atom = self.ident_atom()?;
        self.next()?;
        self.mark(pos);
        self.code().get_field(atom, 0);
        Ok(Expr::Field { atom, pos })
    }

    /// `[expr]` after the `[`. A string literal key becomes a `get_field`. `at`, where
    /// the object starts, names its slot.
    fn parse_index(&mut self, pos: u32, at: u32) -> PResult<Expr> {
        self.names_push(at, &["member.object"]);
        let key = self.parse_expr(false)?;
        self.names_pop(1);
        self.expect(TokenKind::RBracket, "]")?;
        if let Expr::Str { idx } = key {
            let s = self.f.consts[idx as usize].as_string();
            if let Some(s) = s {
                self.truncate_last();
                let atom = self.heap.intern(s)?;
                self.mark(pos);
                self.code().get_field(atom, 0);
                return Ok(Expr::Field { atom, pos });
            }
        }
        self.mark(pos);
        self.code().get_elem();
        Ok(Expr::Elem { pos })
    }

    /// A TypeScript assertion after an expression ending at `end` (`x as T`, `x!`): an
    /// anonymous function that was the whole expression still is (tsc erases the
    /// assertion, so NamedEvaluation names it).
    fn extend_anon_fn(&mut self, end: u32) {
        if let Some((s, e)) = self.last_anon_fn
            && e == end
        {
            self.last_anon_fn = Some((s, self.prev_end));
        }
    }

    /// `( args )`: the arguments are pushed one by one, or, from the first spread
    /// on, collected into an array (`f(a, ...b)` -> `array_from 1; array_spread`).
    pub(super) fn parse_args(&mut self) -> PResult<Args> {
        self.parse_args_as(ArgsOf::Call)
    }

    /// [`Parser::parse_args`] with the arguments' slot names (suspension maps).
    pub(super) fn parse_args_as(&mut self, of: ArgsOf) -> PResult<Args> {
        // An argument's slot is named by its start, the array of a spread by the `(`.
        let paren = self.tok.start;
        self.syn_part(Role::Open, paren, self.tok.end);
        self.expect(TokenKind::LParen, "(")?;
        let mut argc: u32 = 0;
        let mut array = false;
        while !self.is(TokenKind::RParen) {
            if self.is(TokenKind::Ellipsis) {
                if !array {
                    self.code().array_from(argc as u16);
                    self.names_replace(argc as usize, paren, &[of.array()]);
                    array = true;
                }
                let spread = self.tok.start;
                self.next()?;
                self.parse_assign(false)?;
                self.syn_part(Role::Spread, spread, self.prev_end);
                self.code().array_spread();
            } else {
                let arg = self.tok.start;
                #[cfg(feature = "lazy")]
                {
                    self.arg_start = arg;
                }
                self.parse_assign(false)?;
                self.syn_part(Role::Arg, arg, self.prev_end);
                if array {
                    self.code().array_push();
                } else {
                    self.names_push(arg, &[of.arg()]);
                    argc += 1;
                    if argc > u16::MAX as u32 {
                        return syntax(self.tok.start, "Too many call arguments");
                    }
                }
            }
            if !self.eat(TokenKind::Comma)? {
                break;
            }
        }
        self.expect(TokenKind::RParen, ")")?;
        self.names_pop(if array { 1 } else { argc as usize });
        Ok(if array {
            Args::Array
        } else {
            Args::Count(argc as u16)
        })
    }

    /// The call after `prepare_call`.
    pub(super) fn emit_call(&mut self, args: Args) {
        self.names_pop(2);
        match args {
            Args::Count(n) => self.code().call(n),
            Args::Array => self.code().call_spread(),
        }
    }

    /// `new X(args)` / `new X`.
    fn parse_new(&mut self) -> PResult<Expr> {
        let pos = self.tok.start;
        self.next()?;
        if self.is(TokenKind::Dot) {
            self.next()?;
            if !self.tok.is_contextual(Kw::Target) {
                return syntax(self.tok.start, "expecting target");
            }
            self.next()?;
            let Some(r) = self.new_target_ref(pos) else {
                return syntax(pos, "new.target only allowed within functions");
            };
            self.syn_special(super::smap_syntax::Special::NewTarget, pos);
            self.code().name_get(r);
            return Ok(Expr::Value);
        }
        if self.is(TokenKind::Import) && self.peek().kind == TokenKind::LParen {
            return syntax(self.tok.start, "invalid use of 'import()'");
        }
        self.enter()?;
        self.syn_open(NodeKind::New, pos);
        let start = self.tok.start;
        let mark = self.syn_mark();
        let mut e = if self.is(TokenKind::New) {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        self.leave();
        // MemberExpression: no calls before the arguments of `new`.
        loop {
            let tok = self.tok;
            let before = self.prev_end;
            match tok.kind {
                TokenKind::Dot => {
                    let member = NodeKind::Member {
                        computed: false,
                        optional: false,
                    };
                    self.syn_open_late(member, start, mark, Role::Object, before);
                    self.next()?;
                    let name = self.tok.start;
                    e = self.parse_dot_name(tok.start)?;
                    self.syn_part(Role::Property, name, self.prev_end);
                    self.syn_close();
                }
                TokenKind::LBracket => {
                    let member = NodeKind::Member {
                        computed: true,
                        optional: false,
                    };
                    self.syn_open_late(member, start, mark, Role::Object, before);
                    self.next()?;
                    e = self.parse_index(tok.start, start)?;
                    self.syn_part(Role::Property, tok.start, self.prev_end);
                    self.syn_close();
                }
                TokenKind::QuestionDot => {
                    return syntax(
                        tok.start,
                        "new keyword cannot be used with an optional chain",
                    );
                }
                k if self.ts && super::ts::is_lt(k) => {
                    if !self.ts_type_args_in_expr() {
                        break;
                    }
                }
                TokenKind::NoSubstTemplate | TokenKind::TemplateHead => {
                    // `new tag`x`` constructs the result of the tagged call.
                    let tagged = NodeKind::TaggedTemplate;
                    self.syn_open_late(tagged, start, mark, Role::Tag, before);
                    self.prepare_call(e, start);
                    let argc = self.parse_tagged_template()?;
                    self.mark(tok.start);
                    self.names_pop(2);
                    self.code().call(argc);
                    self.syn_close();
                    e = Expr::Value;
                }
                _ => break,
            }
        }
        self.syn_part(Role::Callee, start, self.prev_end);
        // The position of `new` is the token after the constructor: its `(`, or what
        // follows `new X` (as in qjs).
        let at = self.tok.start;
        self.names_push(pos, &["new.callee"]);
        let args = if self.is(TokenKind::LParen) {
            self.parse_args_as(ArgsOf::New)?
        } else {
            Args::Count(0)
        };
        self.names_pop(1);
        self.mark(at);
        match args {
            Args::Count(n) => self.code().construct(n),
            Args::Array => self.code().construct_spread(),
        }
        self.syn_close();
        Ok(Expr::Value)
    }

    /// A regular expression literal (0033): the pattern is compiled here, for its early
    /// errors (qjs reports them at the literal) and once for all evaluations, each of
    /// which creates a fresh RegExp: `program pattern flags; call_intrinsic %RegExpCreate
    /// 3`.
    fn parse_regex_literal(&mut self) -> PResult<Expr> {
        let tok = self.lx.rescan_as_regex(self.tok)?;
        let text = self.lx.text(&tok);
        let close = text.iter().rposition(|&b| b == b'/').unwrap_or(0);
        let utf16 = |b: &[u8]| -> alloc::vec::Vec<u16> {
            core::str::from_utf8(b)
                .unwrap_or("\u{FFFD}")
                .encode_utf16()
                .collect()
        };
        let pattern = utf16(&text[1..close.max(1)]);
        let flags = utf16(&text[close + 1..]);
        let program = match crate::builtins::regexp::compile(&pattern, &flags) {
            Ok(program) => program,
            Err(e) => return syntax(tok.start, alloc::string::String::from(e.message())),
        };
        // The program is compiled once per literal and shared by the RegExp objects of
        // every evaluation (D1502); its cell sits in the constant pool.
        let cell = crate::builtins::regexp::program_cell(self.heap, &program)?;
        let prog = self.add_const(crate::value::Value::internal(cell));
        self.tok = tok;
        let p = self.string_const(&pattern)?;
        let f = self.string_const(&flags)?;
        self.next()?;
        self.code().push_const(prog);
        self.code().push_const(p);
        self.code().push_const(f);
        let create = const { crate::bytecode::intrinsics::const_id("RegExpCreate") };
        self.code().call_intrinsic(create, 3);
        Ok(Expr::Value)
    }

    pub(super) fn parse_primary(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.parse_primary_inner();
        self.leave();
        r
    }

    fn parse_primary_inner(&mut self) -> PResult<Expr> {
        let tok = self.tok;
        match tok.kind {
            TokenKind::Number => {
                let lit = self.lx.number_literal(&tok);
                let n = crate::num::literal_digits_value(lit.base.radix(), lit.digits);
                self.next()?;
                self.emit_number(n);
                Ok(Expr::Value)
            }
            TokenKind::BigInt => {
                self.emit_bigint_literal()?;
                self.next()?;
                Ok(Expr::Value)
            }
            TokenKind::String => {
                let idx = self.cooked_string_const()?;
                self.next()?;
                self.code().push_const(idx);
                Ok(Expr::Str { idx })
            }
            TokenKind::NoSubstTemplate | TokenKind::TemplateHead => self.parse_template(),
            TokenKind::Slash | TokenKind::SlashAssign => self.parse_regex_literal(),
            TokenKind::Ident if self.at_async_function() => {
                let hint = self.take_name_hint(tok.start);
                self.next()?;
                self.parse_function_expression(hint, tok.start, true)
            }
            TokenKind::Ident => self.parse_ident_ref(),
            TokenKind::This => {
                self.next()?;
                self.emit_this(tok.start);
                Ok(Expr::Value)
            }
            TokenKind::Null => {
                self.next()?;
                self.code().null();
                Ok(Expr::Value)
            }
            TokenKind::True => {
                self.next()?;
                self.code().push_true();
                Ok(Expr::Value)
            }
            TokenKind::False => {
                self.next()?;
                self.code().push_false();
                Ok(Expr::Value)
            }
            TokenKind::LParen => {
                self.next()?;
                if self.is(TokenKind::RParen) {
                    return self.unexpected();
                }
                // NamedEvaluation sees through parentheses: `x = (function () {})`.
                if let Some((a, p)) = self.name_hint
                    && p == tok.start
                {
                    self.name_hint = Some((a, self.tok.start));
                }
                let inner = self.tok.start;
                let outer = core::mem::replace(&mut self.paren_inner, inner);
                let e = self.parse_expr(false);
                self.paren_inner = outer;
                let e = e?;
                self.expect(TokenKind::RParen, ")")?;
                // A parenthesized anonymous function is still one (computed keys name
                // it at run time).
                if self
                    .last_anon_fn
                    .is_some_and(|(s, e)| s == inner && e + 1 == self.prev_end)
                {
                    self.last_anon_fn = Some((tok.start, self.prev_end));
                }
                // Parentheses keep references (`(a.b)()`, `(x) = 1`) but not
                // literals as patterns (`({a}) = 1` is an error).
                Ok(match e {
                    Expr::ObjectLit | Expr::ArrayLit | Expr::Str { .. } => Expr::Value,
                    e => e,
                })
            }
            TokenKind::LBracket => self.parse_array(),
            TokenKind::LBrace => self.parse_object(),
            TokenKind::Function => {
                let hint = self.take_name_hint(tok.start);
                self.parse_function_expression(hint, tok.start, false)
            }
            TokenKind::Class => {
                let hint = self.take_name_hint(tok.start);
                self.parse_class(false, hint)?;
                Ok(Expr::Value)
            }
            TokenKind::At => {
                let hint = self.take_name_hint(tok.start);
                self.parse_class_decorators()?;
                self.expect_decorated_class(false)?;
                self.parse_class(false, hint)?;
                Ok(Expr::Value)
            }
            TokenKind::Super => self.parse_super(),
            // strip-types parses modules: `import(...)` and `import.meta` compile to
            // placeholders there.
            TokenKind::Import if self.strip => {
                self.next()?;
                if self.eat(TokenKind::Dot)? {
                    // `import.meta`, `import.defer(...)`
                    self.next()?;
                    self.code().undefined();
                    if self.is(TokenKind::LParen) {
                        self.names_push(tok.start, &["call.callee"]);
                        self.code().undefined();
                        self.names_push(tok.start, &["call.this"]);
                        let args = self.parse_args()?;
                        self.emit_call(args);
                    }
                } else {
                    self.code().undefined();
                    self.prepare_call(Expr::Value, tok.start);
                    let args = self.parse_args()?;
                    self.emit_call(args);
                }
                Ok(Expr::Value)
            }
            TokenKind::Import => self.parse_import_call(),
            TokenKind::Percent if self.opts.privileged => self.parse_intrinsic_call(),
            _ => self.unexpected(),
        }
    }

    /// `import(specifier [, options])` (ImportCall, with import attributes): the call
    /// returns a promise that djs always rejects, having no module loader
    /// (builtins/import.ts). `import.meta` is module code only.
    fn parse_import_call(&mut self) -> PResult<Expr> {
        let pos = self.tok.start;
        self.next()?;
        if self.eat(TokenKind::Dot)? {
            if crate::MODULES
                && self.tok.is_contextual(Kw::Meta)
                && self.module.is_some()
                && !self.tok.has_escape()
            {
                self.import_meta(pos)?;
                return Ok(Expr::Value);
            }
            if self.tok.is_contextual(Kw::Meta) {
                return syntax(self.tok.start, "import.meta only valid in module code");
            }
            return syntax(self.tok.start, "meta expected");
        }
        self.syn_open(NodeKind::ImportCall, pos);
        self.expect(TokenKind::LParen, "(")?;
        let arg = self.tok.start;
        self.parse_assign(false)?;
        self.syn_part(Role::Arg, arg, self.prev_end);
        if self.eat(TokenKind::Comma)? && !self.is(TokenKind::RParen) {
            self.names_push(pos, &["import.specifier"]);
            let arg = self.tok.start;
            self.parse_assign(false)?;
            self.syn_part(Role::Arg, arg, self.prev_end);
            self.names_pop(1);
            self.eat(TokenKind::Comma)?;
        } else {
            self.code().undefined();
        }
        self.expect(TokenKind::RParen, ")")?;
        self.syn_close();
        self.mark(pos);
        if crate::MODULES {
            // `%ModuleOp(specifier, options, referrer)`: the import goes to the host's
            // loader, which resolves the specifier against this unit's module key or
            // script name (builtins/module.ts ImportCall).
            let units: alloc::vec::Vec<u16> = self.opts.filename.encode_utf16().collect();
            let referrer = self.string_const(&units)?;
            self.code().push_const(referrer);
            let id = crate::bytecode::intrinsics::intrinsic_id_of!("ModuleOp");
            self.code().call_intrinsic(id, 3);
        } else {
            let id = crate::bytecode::intrinsics::intrinsic_id_of!("DynamicImport");
            self.code().call_intrinsic(id, 2);
        }
        Ok(Expr::Value)
    }

    /// `super(args)`, `super.x`, `super[k]` (docs/compiler.md 7.9).
    fn parse_super(&mut self) -> PResult<Expr> {
        let pos = self.tok.start;
        self.syn_special(super::smap_syntax::Special::Super, pos);
        self.next()?;
        let tok = self.tok;
        let kind = self.own_this_kind();
        // `super` is the callee or object of the node it starts (D2850).
        let node = match tok.kind {
            TokenKind::LParen => NodeKind::Call { optional: false },
            _ => NodeKind::Member {
                computed: tok.kind == TokenKind::LBracket,
                optional: false,
            },
        };
        let role = if tok.kind == TokenKind::LParen {
            Role::Callee
        } else {
            Role::Object
        };
        let mark = self.syn_mark();
        self.syn_open_late(node, pos, mark, role, self.prev_end);
        let r = self.parse_super_inner(pos, tok, kind);
        if r.is_ok() {
            self.syn_close();
        }
        r
    }

    fn parse_super_inner(
        &mut self,
        pos: u32,
        tok: crate::lexer::Token,
        kind: Option<FuncKind>,
    ) -> PResult<Expr> {
        match tok.kind {
            TokenKind::LParen => {
                if kind != Some(FuncKind::DerivedCtor) {
                    return syntax(
                        tok.start,
                        "super() is only valid in a derived class constructor",
                    );
                }
                self.parse_super_call(pos)?;
                Ok(Expr::Value)
            }
            TokenKind::Dot | TokenKind::LBracket => {
                if !kind.is_some_and(FuncKind::is_method) {
                    return syntax(tok.start, "'super' is only valid in a method");
                }
                // MakeSuperPropertyReference: `this`, then the base from the home object,
                // then the key (converted at the access).
                self.emit_this(pos);
                self.declare_pseudo(super::func::VarKind::HomeObject);
                let home = self.atom_home;
                let r = self.name_ref(home, pos);
                self.code().name_get(r);
                self.code().get_super();
                self.next()?;
                if tok.kind == TokenKind::Dot {
                    if self.is(TokenKind::PrivateName) {
                        return syntax(self.tok.start, "private class field forbidden after super");
                    }
                    if !self.tok.kind.is_identifier_name() {
                        return syntax(self.tok.start, "expecting field name");
                    }
                    let atom = self.ident_atom()?;
                    let name = self.tok.start;
                    self.next()?;
                    self.syn_part(Role::Property, name, self.prev_end);
                    self.mark(tok.start);
                    self.code().get_super_field(atom);
                    Ok(Expr::SuperField {
                        atom,
                        pos: tok.start,
                    })
                } else {
                    self.names_push(pos, &["super.this", "super.base"]);
                    self.parse_expr(false)?;
                    self.names_pop(2);
                    self.expect(TokenKind::RBracket, "]")?;
                    self.syn_part(Role::Property, tok.start, self.prev_end);
                    self.mark(tok.start);
                    self.code().get_super_elem();
                    Ok(Expr::SuperElem { pos: tok.start })
                }
            }
            _ => syntax(tok.start, "invalid use of 'super'"),
        }
    }

    /// `super(args)` in a derived constructor (or an arrow in one): constructs the
    /// parent with this new.target, binds `this` (once) and runs the field
    /// initializers.
    fn parse_super_call(&mut self, pos: u32) -> PResult<()> {
        use super::func::VarKind;
        self.declare_pseudo(VarKind::ActiveFunc);
        let active = self.atom_active;
        let r = self.name_ref(active, pos);
        self.code().name_get(r);
        self.code().get_super();
        self.names_push(pos, &["super.callee"]);
        let paren = self.tok.start;
        let args = self.parse_args_as(ArgsOf::Super)?;
        self.names_pop(1);
        let Some(nt) = self.new_target_ref(pos) else {
            return Err(super::resolve::internal("super() without new.target"));
        };
        self.code().name_get(nt);
        self.mark(paren);
        match args {
            Args::Count(n) => self.code().super_call(n),
            Args::Array => self.code().super_call_spread(),
        }
        self.emit_bind_this(pos);
        Ok(())
    }

    /// After `super(...)` constructed `this` (on the stack, where it stays): binds it
    /// (BindThisValue: a ReferenceError if it is bound already) and runs the field
    /// initializers of the constructor's class (InitializeInstanceElements).
    pub(super) fn emit_bind_this(&mut self, pos: u32) {
        let this = self.this_ref(pos);
        self.mark(pos);
        self.code().name_get(this);
        self.code().check_this_uninit();
        self.code().dup();
        let this = self.this_ref(pos);
        self.code().name_put(this);
        let fields = match self.own_this_function() {
            Ok(None) => self.f.fields_atom,
            Ok(Some(i)) => self.stack[i].f.fields_atom,
            Err(()) => Atom::NULL,
        };
        self.emit_param_prop_decls(pos);
        self.emit_init_fields(fields, pos);
        self.emit_param_prop_assigns(pos);
    }

    /// `this` on the stack: calls the class's instance field initializer (its hidden
    /// binding `fields`, `undefined` without fields) with it; `this` stays.
    pub(super) fn emit_init_fields(&mut self, fields: Atom, pos: u32) {
        let r = self.name_ref(fields, pos);
        let skip = self.new_label();
        self.code().name_get(r);
        self.code().dup();
        self.code().is_undefined();
        self.code().jump_if_true(skip);
        self.code().pick(1);
        self.mark(pos);
        self.code().call(0);
        self.place(skip);
        self.code().drop();
    }

    /// `%Name(args)` in privileged code: `call_intrinsic id, argc` (D1270).
    fn parse_intrinsic_call(&mut self) -> PResult<Expr> {
        let pos = self.tok.start;
        self.next()?;
        if !self.is(TokenKind::Ident) {
            return syntax(self.tok.start, "intrinsic name expected");
        }
        let name = self.lx.text(&self.tok);
        let name = core::str::from_utf8(name).unwrap_or("");
        let Some(id) = crate::bytecode::intrinsics::intrinsic_id(name) else {
            return syntax(
                self.tok.start,
                crate::text::cat!("unknown intrinsic %", name),
            );
        };
        let info = crate::bytecode::intrinsics::intrinsic(id).expect("intrinsic id");
        self.next()?;
        let paren = self.tok.start;
        let argc = match self.parse_args_as(ArgsOf::Intrinsic)? {
            Args::Count(n) => n,
            Args::Array => return syntax(paren, "spread arguments in an intrinsic call"),
        };
        let ok = if info.variadic {
            argc >= u16::from(info.params)
        } else {
            argc == u16::from(info.params)
        };
        if !ok {
            return syntax(
                paren,
                crate::text::cat!("wrong number of arguments for %", name),
            );
        }
        self.mark(pos);
        self.code().call_intrinsic(id, argc);
        Ok(Expr::Value)
    }

    /// An identifier reference.
    pub(super) fn parse_ident_ref(&mut self) -> PResult<Expr> {
        let tok = self.tok;
        if tok.kw == Kw::Yield {
            return syntax(tok.start, "unexpected 'yield' keyword");
        }
        if tok.kw == Kw::Await && self.f.kind == FuncKind::StaticBlock && !tok.has_escape() {
            return syntax(tok.start, "unexpected 'await' keyword");
        }
        if tok.is_escaped_reserved()
            || tok.kw.is_strict_reserved()
            || (tok.kw == Kw::Await && self.await_reserved())
        {
            return self.unexpected();
        }
        let atom = self.ident_atom()?;
        if tok.kw == Kw::Arguments {
            self.note_arguments(tok.start)?;
        }
        self.next()?;
        let r = self.name_ref(atom, tok.start);
        self.mark(tok.start);
        self.code().name_get(r);
        Ok(Expr::Name {
            r,
            pos: tok.start,
            restricted: matches!(tok.kw, Kw::Eval | Kw::Arguments),
        })
    }

    /// An untagged template literal: the cooked parts and `to_string` of each
    /// substitution, joined by one `concat`.
    fn parse_template(&mut self) -> PResult<Expr> {
        // A part's slot is named by its start (a string's by its token's).
        let start = self.tok.start;
        self.syn_open(NodeKind::Template, start);
        let mut parts: u32 = 0;
        let mut units = alloc::vec::Vec::new();
        loop {
            let tok = self.tok;
            self.syn_part(Role::Quasi, tok.start, tok.end);
            units.clear();
            self.lx.cook_template(&tok, &mut units)?;
            if !units.is_empty() {
                let idx = self.string_const(&units)?;
                self.code().push_const(idx);
                self.names_push(tok.start, &["template.part"]);
                parts = self.template_part(parts, start);
            }
            if matches!(
                tok.kind,
                TokenKind::NoSubstTemplate | TokenKind::TemplateTail
            ) {
                self.next()?;
                break;
            }
            self.next()?;
            let part = self.tok.start;
            self.parse_expr(false)?;
            self.syn_part(Role::Subst, part, self.prev_end);
            self.mark(tok.end - 2);
            self.code().to_string();
            self.names_push(part, &["template.part"]);
            parts = self.template_part(parts, start);
            if !self.is(TokenKind::RBrace) {
                return syntax(self.tok.start, "expected '}' after template expression");
            }
            self.tok = self.lx.rescan_template_continuation(self.tok)?;
        }
        self.names_pop(parts as usize);
        match parts {
            0 => {
                let idx = self.string_const(&[])?;
                self.code().push_const(idx);
            }
            1 => {}
            n => self.code().concat(n as u16),
        }
        self.syn_close();
        Ok(Expr::Value)
    }

    /// The template of a tagged template, after the tag and `this`: pushes the
    /// template object and the substitutions, returns the argument count. The object is
    /// described by constants (cooked strings, `undefined` for an invalid escape, and
    /// raw strings) and created once per site by `get_template_object` (D1362).
    fn parse_tagged_template(&mut self) -> PResult<u16> {
        let site = self.add_const(crate::value::Value::UNDEFINED);
        self.code().get_template_object(site);
        // The template object is named by the template, a substitution by its start.
        self.names_push(self.tok.start, &["call.arg"]);
        let mut cooked: alloc::vec::Vec<crate::value::Value> = alloc::vec::Vec::new();
        let mut raw: alloc::vec::Vec<crate::value::Value> = alloc::vec::Vec::new();
        let mut units = alloc::vec::Vec::new();
        let mut argc: u32 = 1;
        loop {
            let tok = self.tok;
            self.syn_part(Role::Quasi, tok.start, tok.end);
            units.clear();
            let c = match self.lx.cook_template(&tok, &mut units) {
                Ok(_) => crate::value::Value::string(self.template_string(&units)?),
                Err(_) => crate::value::Value::UNDEFINED,
            };
            cooked.push_out(c);
            units.clear();
            self.lx.template_raw(&tok, &mut units);
            raw.push_out(crate::value::Value::string(self.template_string(&units)?));
            if matches!(
                tok.kind,
                TokenKind::NoSubstTemplate | TokenKind::TemplateTail
            ) {
                self.next()?;
                break;
            }
            self.next()?;
            let arg = self.tok.start;
            self.parse_expr(false)?;
            self.syn_part(Role::Subst, arg, self.prev_end);
            self.names_push(arg, &["call.arg"]);
            argc += 1;
            if argc > u16::MAX as u32 {
                return syntax(self.tok.start, "Too many call arguments");
            }
            if !self.is(TokenKind::RBrace) {
                return syntax(self.tok.start, "expected '}' after template expression");
            }
            self.tok = self.lx.rescan_template_continuation(self.tok)?;
        }
        self.names_pop(argc as usize);
        // CONSTS[site] = start of `n, cooked x n, raw x n`.
        let start = self.f.consts.len() as u32;
        let n = cooked.len();
        self.f.consts.push_out(crate::value::Value::int(n as i32));
        self.f.consts.extend(cooked);
        self.f.consts.extend(raw);
        self.f.consts[site as usize] = crate::value::Value::int(start as i32);
        Ok(argc as u16)
    }

    /// A string for a template object.
    fn template_string(&mut self, units: &[u16]) -> PResult<crate::value::Str> {
        self.alloc_units(units)
    }

    /// Counts a pushed template part, joining early when the count would not fit (the
    /// joined part is named by the template's `start`).
    fn template_part(&mut self, parts: u32, start: u32) -> u32 {
        let parts = parts + 1;
        if parts == u16::MAX as u32 {
            self.code().concat(u16::MAX);
            self.names_replace(parts as usize, start, &["template.part"]);
            return 1;
        }
        parts
    }

    /// `[a, , ...b]`: up to `STACK_ELEMS` leading elements go on the stack for one
    /// `array_from`; holes, spreads and the rest are appended one by one.
    fn parse_array(&mut self) -> PResult<Expr> {
        const STACK_ELEMS: u16 = 256;
        // An element's slot is named by its start, the array under construction's by
        // the `[`.
        let open = self.tok.start;
        self.syn_open(NodeKind::Array, open);
        self.next()?;
        let mut on_stack: u16 = 0;
        let mut appending = false;
        loop {
            match self.tok.kind {
                TokenKind::RBracket => break,
                TokenKind::Comma => {
                    self.syn_part(Role::Hole, self.tok.start, self.tok.start);
                    self.start_appending(&mut appending, on_stack, open);
                    self.code().array_hole();
                    self.next()?;
                    continue;
                }
                TokenKind::Ellipsis => {
                    let spread = self.tok.start;
                    self.start_appending(&mut appending, on_stack, open);
                    self.next()?;
                    self.parse_assign(false)?;
                    self.syn_part(Role::Spread, spread, self.prev_end);
                    self.code().array_spread();
                }
                _ => {
                    let elem = self.tok.start;
                    self.parse_assign(false)?;
                    self.syn_part(Role::Element, elem, self.prev_end);
                    if appending {
                        self.code().array_push();
                    } else {
                        self.names_push(elem, &["array.element"]);
                        on_stack += 1;
                        if on_stack == STACK_ELEMS {
                            self.start_appending(&mut appending, on_stack, open);
                        }
                    }
                }
            }
            if !self.eat(TokenKind::Comma)? {
                break;
            }
        }
        self.expect(TokenKind::RBracket, "]")?;
        self.syn_close();
        if !appending {
            self.code().array_from(on_stack);
            self.names_pop(on_stack as usize);
        } else {
            self.names_pop(1);
        }
        Ok(Expr::ArrayLit)
    }

    fn start_appending(&mut self, appending: &mut bool, on_stack: u16, open: u32) {
        if !*appending {
            self.code().array_from(on_stack);
            self.names_replace(on_stack as usize, open, &["array.literal"]);
            *appending = true;
        }
    }

    /// `{ a: 1, b, [k]: v, m() {}, get x() {}, set x(v) {}, __proto__: p, ...o }`.
    fn parse_object(&mut self) -> PResult<Expr> {
        let open = self.tok.start;
        self.syn_open(NodeKind::Object, open);
        self.next()?;
        self.code().object();
        self.names_push(open, &["object.literal"]);
        let mut seen_proto = false;
        while !self.is(TokenKind::RBrace) {
            let tok = self.tok;
            if tok.kind == TokenKind::Ellipsis {
                self.next()?;
                self.parse_assign(false)?;
                self.syn_part(Role::Spread, tok.start, self.prev_end);
                self.code().copy_data_props(0);
            } else {
                self.parse_property(tok, &mut seen_proto)?;
            }
            if !self.eat(TokenKind::Comma)? {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "}")?;
        self.syn_close();
        self.names_pop(1);
        Ok(Expr::ObjectLit)
    }

    /// A property name: `Some(atom)` for a static one, `None` for a computed one (the
    /// key is then on the stack).
    pub(super) fn parse_property_key(&mut self) -> PResult<Option<Atom>> {
        let tok = self.tok;
        match tok.kind {
            TokenKind::LBracket => {
                self.next()?;
                self.parse_assign(false)?;
                self.expect(TokenKind::RBracket, "]")?;
                #[cfg(feature = "dump")]
                if let Some(r) = self.f.smap.as_mut() {
                    r.computed_keys.push((tok.start, self.prev_end));
                }
                Ok(None)
            }
            TokenKind::String => {
                let a = self.cooked_string_atom()?;
                self.next()?;
                Ok(Some(a))
            }
            TokenKind::Number => {
                let lit = self.lx.number_literal(&tok);
                let n = crate::num::literal_digits_value(lit.base.radix(), lit.digits);
                let a = self.atom_of_number(n)?;
                self.next()?;
                Ok(Some(a))
            }
            TokenKind::BigInt => {
                let s = self.bigint_literal()?.to_string_radix(10);
                let a = self.heap.atom_str(s.as_str())?;
                self.next()?;
                Ok(Some(a))
            }
            k if k.is_identifier_name() => {
                let a = self.ident_atom()?;
                self.next()?;
                Ok(Some(a))
            }
            _ => syntax(tok.start, "invalid property name"),
        }
    }

    fn parse_property(&mut self, tok: crate::lexer::Token, seen_proto: &mut bool) -> PResult<()> {
        // `get`/`set`/`async` are prefixes only when a property name follows (for
        // `async`, on the same line).
        let prefix = if matches!(tok.kw, Kw::Get | Kw::Set | Kw::Async)
            && tok.kind == TokenKind::Ident
            && !tok.has_escape()
        {
            let next = self.peek();
            let starts = (next.kind.is_identifier_name()
                || matches!(
                    next.kind,
                    TokenKind::String
                        | TokenKind::Number
                        | TokenKind::BigInt
                        | TokenKind::LBracket
                        | TokenKind::PrivateName
                        | TokenKind::Star
                ))
                && !(tok.kw == Kw::Async && next.newline_before());
            if starts {
                self.next()?;
                Some(tok.kw)
            } else {
                None
            }
        } else {
            None
        };
        let is_async = prefix == Some(Kw::Async);
        let prefix = prefix.filter(|&p| p != Kw::Async);
        let is_gen = if prefix.is_none() {
            self.eat(TokenKind::Star)?
        } else {
            if self.is(TokenKind::Star) {
                return syntax(self.tok.start, "invalid property name");
            }
            false
        };
        if self.is(TokenKind::PrivateName) {
            return syntax(self.tok.start, "invalid property name");
        }
        let key_tok = self.tok;
        let key = self.parse_property_key()?;
        let key_end = self.prev_end;
        if let Some(p) = prefix {
            // An accessor.
            let (kind, prefix_id, word) = if p == Kw::Get {
                (FuncKind::Getter, 1, "get ")
            } else {
                (FuncKind::Setter, 2, "set ")
            };
            let name = match key {
                Some(a) => {
                    let mut n = alloc::string::String::from(word);
                    n.push_str(&self.heap.atom_name(a));
                    self.heap.atom_str(&n)?
                }
                None => Atom::NULL,
            };
            let child = self.parse_method_function(kind, name, tok.start, (false, false))?;
            self.syn_part(Role::Method, tok.start, self.prev_end);
            self.code().fclosure(child);
            match (key, kind) {
                (Some(a), FuncKind::Getter) => self.code().define_getter(a),
                (Some(a), _) => self.code().define_setter(a),
                (None, FuncKind::Getter) => {
                    self.code().set_function_name(prefix_id);
                    self.code().define_getter_elem()
                }
                (None, _) => {
                    self.code().set_function_name(prefix_id);
                    self.code().define_setter_elem()
                }
            }
            return Ok(());
        }
        let method_start = self.is(TokenKind::LParen) || (self.ts && self.is(TokenKind::Lt));
        if (is_async || is_gen) && !method_start {
            return syntax(self.tok.start, "invalid property name");
        }
        match self.tok.kind {
            _ if method_start => {
                let child = self.parse_method_function(
                    FuncKind::Method,
                    key.unwrap_or(Atom::NULL),
                    if is_async || is_gen {
                        tok.start
                    } else {
                        key_tok.start
                    },
                    (is_async, is_gen),
                )?;
                self.syn_part(Role::Method, tok.start, self.prev_end);
                self.code().fclosure(child);
                match key {
                    Some(a) => self.code().define_method(a),
                    None => {
                        self.code().set_function_name(0);
                        self.code().define_method_elem()
                    }
                }
            }
            TokenKind::Colon => {
                let key_role = if key.is_some() {
                    Role::Key
                } else {
                    Role::ComputedKey
                };
                self.syn_part(key_role, key_tok.start, key_end);
                self.next()?;
                let value = self.tok.start;
                // A literal `__proto__: v` sets the prototype (not for computed or
                // shorthand keys).
                let is_proto = key.is_some()
                    && matches!(key_tok.kind, TokenKind::Ident | TokenKind::String)
                    && self.lx_text_is(key_tok, "__proto__");
                if is_proto {
                    self.parse_assign(false)?;
                    self.syn_part(Role::Value, value, self.prev_end);
                    if *seen_proto {
                        // (Duplicates are fine in a destructuring pattern, which is
                        // parsed by compiler/pattern.rs instead.)
                        return syntax(self.tok.start, "duplicate __proto__ property name");
                    }
                    *seen_proto = true;
                    self.code().set_proto();
                    return Ok(());
                }
                let value_start = self.tok.start;
                match key {
                    Some(a) => {
                        self.parse_named(a, |p| p.parse_assign(false))?;
                    }
                    None => {
                        self.code().to_propkey();
                        self.names_push(key_tok.start, &["object.key"]);
                        self.parse_assign(false)?;
                        self.names_pop(1);
                    }
                }
                self.syn_part(Role::Value, value, self.prev_end);
                match key {
                    Some(a) => self.code().define_field(a, 0),
                    None => {
                        // NamedEvaluation with a computed key happens at run time.
                        if self.last_anon_fn == Some((value_start, self.prev_end)) {
                            self.code().set_function_name(0);
                        }
                        self.code().define_elem()
                    }
                }
            }
            TokenKind::Comma | TokenKind::RBrace if key_tok.kind == TokenKind::Ident => {
                if key_tok.kw == Kw::Yield {
                    return syntax(key_tok.start, "unexpected 'yield' keyword");
                }
                if key_tok.is_escaped_reserved() || key_tok.kw.is_strict_reserved() {
                    return syntax(key_tok.start, "invalid property name");
                }
                if key_tok.kw == Kw::Await && self.await_reserved() {
                    return syntax(self.tok.start, "expecting ':'");
                }
                let a = key.unwrap_or(Atom::NULL);
                self.syn_part(Role::Shorthand, key_tok.start, key_tok.end);
                if key_tok.kw == Kw::Arguments {
                    self.note_arguments(key_tok.start)?;
                }
                let r = self.name_ref(a, key_tok.start);
                self.mark(key_tok.start);
                self.code().name_get(r);
                self.code().define_field(a, 0);
            }
            _ => return syntax(self.tok.start, "expecting '}'"),
        }
        Ok(())
    }

    /// The source text of a token (identifiers cooked).
    fn lx_text_is(&mut self, tok: crate::lexer::Token, s: &str) -> bool {
        if tok.kind == TokenKind::String {
            let mut units = alloc::vec::Vec::new();
            self.lx.cook_string(&tok, &mut units);
            return units.iter().copied().eq(s.encode_utf16());
        }
        self.lx.ident_value(&tok, &mut self.buf8) == s.as_bytes()
    }
}
