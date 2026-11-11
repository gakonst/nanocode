// Ported from GoAWK's compiler (internal/compiler/compiler.go, opcodes.go),
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! The syntax tree to instructions for a stack machine ([`super::interp`]). Loops are
//! jumps; `for (k in a)` iterates a snapshot of the keys kept on a stack of its own, so
//! nothing in the machine recurses.

use std::collections::HashMap;
use std::rc::Rc;

use psh_regex::Regex;

use super::ast::*;
use super::lexer::{Builtin, Pos};
use super::parser::ParseError;
use super::resolve::{Resolved, Scope, Special, Type};
use super::value::S;

/// A scalar variable.
#[derive(Clone, Copy, Debug)]
pub enum Var {
    Global(u32),
    Local(u32),
    Special(Special),
}

/// An array variable.
#[derive(Clone, Copy, Debug)]
pub enum Arr {
    Global(u32),
    Local(u32),
}

/// What getline assigns.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// `$0` (with NF).
    Record,
    Var(Var),
    /// `$n`: the index is on the stack.
    Field,
    /// `a[k]`: the index is on the stack.
    Array(Arr),
}

/// The separator of `split`.
#[derive(Clone, Copy, Debug)]
pub enum SplitSep {
    /// FS.
    Fs,
    /// A string on the stack (FS rules: " ", one character, else a regex).
    Dynamic,
    /// A regex literal.
    Regex(u32),
}

#[derive(Clone, Copy, Debug)]
pub enum Redir {
    None,
    Write,
    Append,
    Pipe,
}

#[derive(Debug)]
pub enum Op {
    Num(f64),
    Str(S),
    Dup,
    Drop,
    Swap,
    /// a b c -> b c a
    Rote,
    /// index -> $index
    Field,
    FieldInt(u32),
    Load(Var),
    /// index -> a[index] (created if missing)
    ArrayGet(Arr),
    /// index -> (index in a)
    In(Arr),
    /// value -> (stored)
    Store(Var),
    /// value index -> (stored)
    StoreField,
    /// count value index -> count: stores only if count > 0 (sub/gsub on a field).
    StoreFieldSub,
    /// value index -> (stored)
    StoreArray(Arr),
    /// index -> (deleted)
    Delete(Arr),
    DeleteAll(Arr),
    /// $0 ~ the regex
    MatchRecord(u32),
    /// value -> (value ~ the regex), negated for `!~`
    MatchConst(u32, bool),
    /// n values -> joined with SUBSEP
    IndexMulti(u32),
    ConcatMulti(u32),
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// l r -> (l ~ r), r a dynamic regex
    Match,
    NotMatch,
    Not,
    Neg,
    Plus,
    Boolean,
    /// Relative to the next instruction.
    Jump(i32),
    JumpFalse(i32),
    JumpTrue(i32),
    Next,
    Nextfile,
    Exit,
    /// status -> (exits)
    ExitStatus,
    /// Snapshots the keys of the array for the loop that follows.
    ForInStart(Arr),
    /// Assigns the next key to the variable, or ends the loop: pops the snapshot and
    /// jumps.
    ForInNext(Var, i32),
    /// Pops the snapshot (`break` out of a for-in).
    ForInEnd,
    /// args -> result
    Call(Builtin, u32),
    LengthArray(Arr),
    /// s [sep] -> count
    Split(Arr, SplitSep),
    /// s re -> RSTART, filling the array with the groups (gawk's third argument).
    MatchArray(Arr),
    /// fmt args... -> string
    Sprintf(u32),
    /// The scalar arguments are on the stack (missing ones as Nulls before).
    CallUser {
        func: u32,
        arrays: Box<[Arr]>,
    },
    /// value -> (returned)
    Return,
    ReturnNull,
    Nulls(u32),
    /// [dest] args... ->
    Print(u32, Redir),
    Printf(u32, Redir),
    /// [file/cmd] [index] -> status
    Getline(super::ast::GetlineSrc, Target),
    /// The source line of the statements that follow (for messages).
    Line(u32),
}

pub struct Func {
    pub num_scalars: usize,
    pub num_arrays: usize,
    /// Per parameter: whether it is an array.
    pub arrays: Vec<bool>,
    pub body: Vec<Op>,
}

pub struct CAction {
    pub pattern: Vec<Vec<Op>>,
    /// None: `{ print }`.
    pub body: Option<Vec<Op>>,
}

pub struct Program {
    pub begin: Vec<Op>,
    pub actions: Vec<CAction>,
    pub end: Vec<Op>,
    pub has_end: bool,
    pub funcs: Vec<Func>,
    pub regexes: Vec<Rc<Regex>>,
    /// Global scalars and arrays by name (`-v`, `var=value` operands, ENVIRON...).
    pub scalar_names: HashMap<Name, u32>,
    pub array_names: HashMap<Name, u32>,
    pub num_scalars: usize,
    pub num_arrays: usize,
}

/// Which code a frame runs.
#[derive(Clone, Copy, Debug)]
pub enum CodeId {
    Begin,
    End,
    Pattern(usize, usize),
    Action(usize),
    Func(usize),
}

impl Program {
    pub fn code(&self, id: CodeId) -> &[Op] {
        match id {
            CodeId::Begin => &self.begin,
            CodeId::End => &self.end,
            CodeId::Pattern(a, i) => &self.actions[a].pattern[i],
            CodeId::Action(a) => self.actions[a].body.as_deref().unwrap_or(&[]),
            CodeId::Func(f) => &self.funcs[f].body,
        }
    }
}

/// Compiles a regex literal (an error is a syntax error at `pos`).
pub type RegexCompiler<'a> = &'a dyn Fn(&[u8]) -> Result<Regex, String>;

pub fn compile(
    prog: &super::ast::Program,
    res: &Resolved,
    re: RegexCompiler,
) -> Result<Program, ParseError> {
    let mut out = Program {
        begin: Vec::new(),
        actions: Vec::new(),
        end: Vec::new(),
        has_end: !prog.end.is_empty(),
        funcs: Vec::new(),
        regexes: Vec::new(),
        scalar_names: HashMap::new(),
        array_names: HashMap::new(),
        num_scalars: res.num_global_scalars,
        num_arrays: res.num_global_arrays,
    };
    for (name, info) in &res.globals {
        if info.ty == Type::Array {
            out.array_names.insert(name.clone(), info.index as u32);
        } else {
            out.scalar_names.insert(name.clone(), info.index as u32);
        }
    }
    for (i, f) in prog.functions.iter().enumerate() {
        let arrays: Vec<bool> = f
            .params
            .iter()
            .map(|p| res.locals[i][p].ty == Type::Array)
            .collect();
        let num_arrays = arrays.iter().filter(|a| **a).count();
        out.funcs.push(Func {
            num_scalars: arrays.len() - num_arrays,
            num_arrays,
            arrays,
            body: Vec::new(),
        });
    }
    let mut regex_index: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut c = Compiler {
        res,
        func: None,
        code: Vec::new(),
        breaks: Vec::new(),
        continues: Vec::new(),
        regexes: &mut out.regexes,
        regex_index: &mut regex_index,
        re,
        func_arrays: out.funcs.iter().map(|f| f.arrays.clone()).collect(),
    };
    let mut bodies = Vec::new();
    for (i, f) in prog.functions.iter().enumerate() {
        c.func = Some(i);
        c.stmts(&f.body)?;
        bodies.push(std::mem::take(&mut c.code));
    }
    c.func = None;
    for b in &prog.begin {
        c.stmts(b)?;
    }
    let begin = std::mem::take(&mut c.code);
    let mut actions = Vec::new();
    for a in &prog.actions {
        let mut pattern = Vec::new();
        for p in &a.pattern {
            c.expr(p)?;
            pattern.push(std::mem::take(&mut c.code));
        }
        let body = match &a.body {
            None => None,
            Some(b) => {
                c.stmts(b)?;
                Some(std::mem::take(&mut c.code))
            }
        };
        actions.push(CAction { pattern, body });
    }
    for b in &prog.end {
        c.stmts(b)?;
    }
    let end = std::mem::take(&mut c.code);
    drop(c);
    for (f, body) in out.funcs.iter_mut().zip(bodies) {
        f.body = body;
    }
    out.begin = begin;
    out.actions = actions;
    out.end = end;
    Ok(out)
}

struct Compiler<'a> {
    res: &'a Resolved,
    func: Option<usize>,
    code: Vec<Op>,
    /// Per enclosing loop, the jumps to patch to its end and whether it is a for-in
    /// (whose break also drops the key snapshot).
    breaks: Vec<(bool, Vec<usize>)>,
    continues: Vec<Vec<usize>>,
    regexes: &'a mut Vec<Rc<Regex>>,
    regex_index: &'a mut HashMap<Vec<u8>, u32>,
    re: RegexCompiler<'a>,
    func_arrays: Vec<Vec<bool>>,
}

type CResult = Result<(), ParseError>;

impl Compiler<'_> {
    fn scalar(&self, name: &[u8]) -> Var {
        match self.res.lookup(self.func, name).0 {
            Scope::Global(i) => Var::Global(i as u32),
            Scope::Local(i) => Var::Local(i as u32),
            Scope::Special(s) => Var::Special(s),
        }
    }

    fn array(&self, name: &[u8]) -> Arr {
        match self.res.lookup(self.func, name).0 {
            Scope::Local(i) => Arr::Local(i as u32),
            Scope::Global(i) => Arr::Global(i as u32),
            Scope::Special(_) => unreachable!("special variables are scalars"),
        }
    }

    fn is_array(&self, name: &[u8]) -> bool {
        self.res.lookup(self.func, name).1 == Type::Array
    }

    fn regex(&mut self, src: &[u8], pos: Pos) -> Result<u32, ParseError> {
        if let Some(&i) = self.regex_index.get(src) {
            return Ok(i);
        }
        let re = (self.re)(src).map_err(|msg| ParseError { pos, msg })?;
        let i = self.regexes.len() as u32;
        self.regexes.push(Rc::new(re));
        self.regex_index.insert(src.to_vec(), i);
        Ok(i)
    }

    fn add(&mut self, op: Op) {
        self.code.push(op);
    }

    /// A forward jump to patch: its index.
    fn jump_forward(&mut self, op: fn(i32) -> Op) -> usize {
        self.code.push(op(0));
        self.code.len() - 1
    }

    fn patch(&mut self, at: usize) {
        let offset = (self.code.len() - at - 1) as i32;
        self.code[at] = match self.code[at] {
            Op::Jump(_) => Op::Jump(offset),
            Op::JumpFalse(_) => Op::JumpFalse(offset),
            Op::JumpTrue(_) => Op::JumpTrue(offset),
            Op::ForInNext(v, _) => Op::ForInNext(v, offset),
            _ => unreachable!(),
        };
    }

    fn jump_back(&mut self, label: usize, op: fn(i32) -> Op) {
        let offset = label as i32 - (self.code.len() as i32 + 1);
        self.code.push(op(offset));
    }

    fn stmts(&mut self, ss: &[Stmt]) -> CResult {
        for s in ss {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> CResult {
        match s {
            Stmt::Expr(e) => {
                match e {
                    // No value needed: no Dup and Drop.
                    Expr::Assign(l, r) => {
                        self.expr(r)?;
                        self.assign(l)?;
                    }
                    Expr::Incr { expr, decr, .. } => {
                        self.dupe_index_lvalue(expr)?;
                        self.add(Op::Num(1.0));
                        self.add(if *decr { Op::Sub } else { Op::Add });
                        self.assign_rote_index(expr)?;
                    }
                    Expr::AugAssign(l, op, r) => {
                        self.aug_assign(l, *op, r)?;
                        self.add(Op::Drop);
                    }
                    _ => {
                        self.expr(e)?;
                        self.add(Op::Drop);
                    }
                }
            }
            Stmt::Print {
                args,
                redirect,
                dest,
            }
            | Stmt::Printf {
                args,
                redirect,
                dest,
            } => {
                if let Some(d) = dest {
                    self.expr(d)?;
                }
                for a in args {
                    self.expr(a)?;
                }
                let r = match redirect {
                    Redirect::None => Redir::None,
                    Redirect::Write => Redir::Write,
                    Redirect::Append => Redir::Append,
                    Redirect::Pipe => Redir::Pipe,
                };
                let n = args.len() as u32;
                self.add(if matches!(s, Stmt::Print { .. }) {
                    Op::Print(n, r)
                } else {
                    Op::Printf(n, r)
                });
            }
            Stmt::If { cond, body, else_ } => {
                self.expr(cond)?;
                let if_mark = self.jump_forward(Op::JumpFalse);
                self.stmts(body)?;
                if else_.is_empty() {
                    self.patch(if_mark);
                } else {
                    let else_mark = self.jump_forward(Op::Jump);
                    self.patch(if_mark);
                    self.stmts(else_)?;
                    self.patch(else_mark);
                }
            }
            Stmt::For {
                pre,
                cond,
                post,
                body,
            } => {
                if let Some(p) = pre {
                    self.stmt(p)?;
                }
                self.breaks.push((false, Vec::new()));
                self.continues.push(Vec::new());
                // The condition once before the loop and again at its end.
                let mark = match cond {
                    Some(c) => {
                        self.expr(c)?;
                        Some(self.jump_forward(Op::JumpFalse))
                    }
                    None => None,
                };
                let start = self.code.len();
                self.stmts(body)?;
                self.patch_continues();
                if let Some(p) = post {
                    self.stmt(p)?;
                }
                match cond {
                    Some(c) => {
                        self.expr(c)?;
                        self.jump_back(start, Op::JumpTrue);
                    }
                    None => self.jump_back(start, Op::Jump),
                }
                if let Some(m) = mark {
                    self.patch(m);
                }
                self.patch_breaks();
            }
            Stmt::While { cond, body } => {
                self.breaks.push((false, Vec::new()));
                self.continues.push(Vec::new());
                self.expr(cond)?;
                let mark = self.jump_forward(Op::JumpFalse);
                let start = self.code.len();
                self.stmts(body)?;
                self.patch_continues();
                self.expr(cond)?;
                self.jump_back(start, Op::JumpTrue);
                self.patch(mark);
                self.patch_breaks();
            }
            Stmt::DoWhile { body, cond } => {
                self.breaks.push((false, Vec::new()));
                self.continues.push(Vec::new());
                let start = self.code.len();
                self.stmts(body)?;
                self.patch_continues();
                self.expr(cond)?;
                self.jump_back(start, Op::JumpTrue);
                self.patch_breaks();
            }
            Stmt::ForIn {
                var, array, body, ..
            } => {
                let v = self.scalar(var);
                let a = self.array(array);
                self.add(Op::ForInStart(a));
                let top = self.code.len();
                self.code.push(Op::ForInNext(v, 0));
                self.breaks.push((true, Vec::new()));
                self.continues.push(Vec::new());
                self.stmts(body)?;
                self.patch_continues();
                self.jump_back(top, Op::Jump);
                self.patch(top);
                self.patch_breaks();
            }
            Stmt::Break => {
                let for_in = self.breaks.last().expect("the parser checks break").0;
                if for_in {
                    self.code.push(Op::ForInEnd);
                }
                let j = self.jump_forward(Op::Jump);
                self.breaks.last_mut().unwrap().1.push(j);
            }
            Stmt::Continue => {
                let j = self.jump_forward(Op::Jump);
                self.continues.last_mut().unwrap().push(j);
            }
            Stmt::Next => self.add(Op::Next),
            Stmt::Nextfile => self.add(Op::Nextfile),
            Stmt::Exit(e) => match e {
                Some(e) => {
                    self.expr(e)?;
                    self.add(Op::ExitStatus);
                }
                None => self.add(Op::Exit),
            },
            Stmt::Return(e) => match e {
                Some(e) => {
                    // `return a` of an array parameter is not valid awk; a scalar only.
                    self.expr(e)?;
                    self.add(Op::Return);
                }
                None => self.add(Op::ReturnNull),
            },
            Stmt::Delete { array, index, .. } => {
                let a = self.array(array);
                if index.is_empty() {
                    self.add(Op::DeleteAll(a));
                } else {
                    self.index(index)?;
                    self.add(Op::Delete(a));
                }
            }
            Stmt::Block(b) => self.stmts(b)?,
            Stmt::Line(n) => self.add(Op::Line(*n)),
        }
        Ok(())
    }

    fn patch_breaks(&mut self) {
        if let Some((_, list)) = self.breaks.pop() {
            for j in list {
                self.patch(j);
            }
        }
    }

    fn patch_continues(&mut self) {
        let list = self.continues.pop().unwrap();
        for j in list {
            self.patch(j);
        }
    }

    fn assign(&mut self, target: &Expr) -> CResult {
        match target {
            Expr::Var(name, _) => {
                let v = self.scalar(name);
                self.add(Op::Store(v));
            }
            Expr::Field(i) => {
                self.expr(i)?;
                self.add(Op::StoreField);
            }
            Expr::Index(name, _, index) => {
                self.index(index)?;
                let a = self.array(name);
                self.add(Op::StoreArray(a));
            }
            _ => unreachable!("not an lvalue"),
        }
        Ok(())
    }

    /// Assigns to `target` whose index (if any) is below the value on the stack.
    fn assign_rote_index(&mut self, target: &Expr) -> CResult {
        match target {
            Expr::Var(..) => self.assign(target)?,
            Expr::Field(_) => {
                self.add(Op::Swap);
                self.add(Op::StoreField);
            }
            Expr::Index(name, _, _) => {
                self.add(Op::Swap);
                let a = self.array(name);
                self.add(Op::StoreArray(a));
            }
            _ => unreachable!("not an lvalue"),
        }
        Ok(())
    }

    /// Pushes an lvalue's index (if any) and then its value.
    fn dupe_index_lvalue(&mut self, e: &Expr) -> CResult {
        match e {
            Expr::Var(..) => self.expr(e)?,
            Expr::Field(i) => {
                self.expr(i)?;
                self.add(Op::Dup);
                self.add(Op::Field);
            }
            Expr::Index(name, _, index) => {
                self.index(index)?;
                self.add(Op::Dup);
                let a = self.array(name);
                self.add(Op::ArrayGet(a));
            }
            _ => unreachable!("not an lvalue"),
        }
        Ok(())
    }

    fn bin(&mut self, op: BinOp) {
        self.add(match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Mod => Op::Mod,
            BinOp::Pow => Op::Pow,
            BinOp::Concat => Op::Concat,
            BinOp::Eq => Op::Eq,
            BinOp::Ne => Op::Ne,
            BinOp::Lt => Op::Lt,
            BinOp::Le => Op::Le,
            BinOp::Gt => Op::Gt,
            BinOp::Ge => Op::Ge,
            BinOp::Match => Op::Match,
            BinOp::NotMatch => Op::NotMatch,
            BinOp::And | BinOp::Or => unreachable!(),
        });
    }

    /// `l op= r`, leaving the new value on the stack.
    fn aug_assign(&mut self, l: &Expr, op: BinOp, r: &Expr) -> CResult {
        match l {
            // The right side first, as gawk: `x += x++`, `$f() += g()` calls g first.
            Expr::Var(..) => {
                self.expr(r)?;
                self.expr(l)?;
                self.add(Op::Swap);
                self.bin(op);
                self.add(Op::Dup);
                self.assign(l)?;
            }
            _ => {
                // r [index old] -> [index old r] -> [index new] -> [new new index].
                self.expr(r)?;
                self.dupe_index_lvalue(l)?;
                self.add(Op::Rote);
                self.bin(op);
                self.add(Op::Dup);
                self.add(Op::Rote);
                self.store_indexed(l);
            }
        }
        Ok(())
    }

    /// value index -> (stored), for a field or an array element target.
    fn store_indexed(&mut self, l: &Expr) {
        match l {
            Expr::Field(_) => self.add(Op::StoreField),
            Expr::Index(name, _, _) => {
                let a = self.array(name);
                self.add(Op::StoreArray(a));
            }
            _ => unreachable!(),
        }
    }

    fn index(&mut self, index: &[Expr]) -> CResult {
        for e in index {
            match e {
                // An integer constant subscript is its string already.
                Expr::Num(n) if *n == n.trunc() && n.abs() < 1e15 => {
                    self.add(Op::Str(Rc::from(format!("{}", *n as i64).as_bytes())));
                }
                _ => self.expr(e)?,
            }
        }
        if index.len() > 1 {
            self.add(Op::IndexMulti(index.len() as u32));
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> CResult {
        match e {
            Expr::Num(n) => self.add(Op::Num(*n)),
            Expr::Str(s) => self.add(Op::Str(Rc::from(s.as_slice()))),
            // Regex positions are compiled by their users; elsewhere its source.
            Expr::RegexArg(r) => self.add(Op::Str(Rc::from(r.as_slice()))),
            Expr::Regex(r) => {
                let i = self.regex(r, Pos::default())?;
                self.add(Op::MatchRecord(i));
            }
            Expr::Field(i) => match **i {
                Expr::Num(n) if n >= 0.0 && n == n.trunc() && n < 1e9 => {
                    self.add(Op::FieldInt(n as u32))
                }
                _ => {
                    self.expr(i)?;
                    self.add(Op::Field);
                }
            },
            Expr::Var(name, _) => {
                let v = self.scalar(name);
                self.add(Op::Load(v));
            }
            Expr::Index(name, _, index) => {
                self.index(index)?;
                let a = self.array(name);
                self.add(Op::ArrayGet(a));
            }
            Expr::Binary(l, op, r) => match op {
                BinOp::And => {
                    self.expr(l)?;
                    self.add(Op::Dup);
                    let m = self.jump_forward(Op::JumpFalse);
                    self.add(Op::Drop);
                    self.expr(r)?;
                    self.patch(m);
                    self.add(Op::Boolean);
                }
                BinOp::Or => {
                    self.expr(l)?;
                    self.add(Op::Dup);
                    let m = self.jump_forward(Op::JumpTrue);
                    self.add(Op::Drop);
                    self.expr(r)?;
                    self.patch(m);
                    self.add(Op::Boolean);
                }
                BinOp::Concat => {
                    let mut values = Vec::new();
                    flatten_concat(e, &mut values);
                    for v in &values {
                        self.expr(v)?;
                    }
                    if values.len() == 2 {
                        self.add(Op::Concat);
                    } else {
                        self.add(Op::ConcatMulti(values.len() as u32));
                    }
                }
                BinOp::Match | BinOp::NotMatch => {
                    self.expr(l)?;
                    match &**r {
                        Expr::RegexArg(re) | Expr::Regex(re) => {
                            let i = self.regex(re, Pos::default())?;
                            self.add(Op::MatchConst(i, *op == BinOp::NotMatch));
                        }
                        _ => {
                            self.expr(r)?;
                            self.bin(*op);
                        }
                    }
                }
                _ => {
                    // gawk folds constants: a constant division by zero is an error.
                    if matches!(op, BinOp::Div | BinOp::Mod)
                        && matches!(**l, Expr::Num(_))
                        && matches!(**r, Expr::Num(z) if z == 0.0)
                    {
                        return Err(ParseError {
                            pos: Pos::default(),
                            msg: "error: division by zero attempted".into(),
                        });
                    }
                    self.expr(l)?;
                    self.expr(r)?;
                    self.bin(*op);
                }
            },
            Expr::Incr { expr, decr, pre } => {
                let op = if *decr { Op::Sub } else { Op::Add };
                if *pre {
                    self.dupe_index_lvalue(expr)?;
                    self.add(Op::Num(1.0));
                    self.add(op);
                    self.dup_and_store(expr)?;
                } else {
                    // The old value (as a number) is the result.
                    self.dupe_index_lvalue(expr)?;
                    self.add(Op::Plus);
                    match &**expr {
                        Expr::Var(..) => {
                            self.add(Op::Dup);
                            self.add(Op::Num(1.0));
                            self.add(op);
                            self.assign(expr)?;
                        }
                        _ => {
                            // [index old] -> [old index old+1] -> store
                            self.add(Op::Dup);
                            self.add(Op::Rote);
                            // [old old index]
                            self.add(Op::Swap);
                            // [old index old]
                            self.add(Op::Num(1.0));
                            self.add(op);
                            // [old index new] -> need [old new index]
                            self.add(Op::Swap);
                            self.store_indexed(expr);
                        }
                    }
                }
            }
            Expr::Assign(l, r) => {
                match &**l {
                    Expr::Var(..) => {
                        self.expr(r)?;
                        self.add(Op::Dup);
                        self.assign(l)?;
                    }
                    Expr::Field(i) => {
                        // The value first, as goawk.
                        self.expr(r)?;
                        self.add(Op::Dup);
                        self.expr(i)?;
                        self.add(Op::StoreField);
                    }
                    Expr::Index(name, _, index) => {
                        self.expr(r)?;
                        self.add(Op::Dup);
                        self.index(index)?;
                        let a = self.array(name);
                        self.add(Op::StoreArray(a));
                    }
                    _ => unreachable!(),
                }
            }
            Expr::AugAssign(l, op, r) => self.aug_assign(l, *op, r)?,
            Expr::Cond(c, t, f) => {
                self.expr(c)?;
                let if_mark = self.jump_forward(Op::JumpFalse);
                self.expr(t)?;
                let else_mark = self.jump_forward(Op::Jump);
                self.patch(if_mark);
                self.expr(f)?;
                self.patch(else_mark);
            }
            Expr::Unary(op, v) => {
                self.expr(v)?;
                self.add(match op {
                    UnOp::Not => Op::Not,
                    UnOp::Neg => Op::Neg,
                    UnOp::Plus => Op::Plus,
                });
            }
            Expr::In(index, name, _) => {
                self.index(index)?;
                let a = self.array(name);
                self.add(Op::In(a));
            }
            Expr::Group(e) => self.expr(e)?,
            Expr::Multi(..) => {
                return Err(ParseError {
                    pos: Pos::default(),
                    msg: "syntax error".into(),
                });
            }
            Expr::Call(b, args) => self.call(*b, args)?,
            Expr::UserCall(name, _, args) => {
                let f = self.res.funcs[name];
                let arrays_of = self.func_arrays[f].clone();
                let mut arrays = Vec::new();
                let mut scalars = 0;
                for (i, a) in args.iter().enumerate() {
                    if arrays_of[i] {
                        let Expr::Var(n, _) = a else {
                            unreachable!("the resolver checks array arguments")
                        };
                        arrays.push(self.array(n));
                    } else {
                        self.expr(a)?;
                        scalars += 1;
                    }
                }
                let want = arrays_of.iter().filter(|a| !**a).count();
                if scalars < want {
                    self.add(Op::Nulls((want - scalars) as u32));
                }
                self.add(Op::CallUser {
                    func: f as u32,
                    arrays: arrays.into_boxed_slice(),
                });
            }
            Expr::Getline { src, target, from } => {
                if let Some(f) = from {
                    self.expr(f)?;
                }
                let t = match target.as_deref() {
                    None => Target::Record,
                    Some(Expr::Var(name, _)) => Target::Var(self.scalar(name)),
                    Some(Expr::Field(i)) => {
                        self.expr(i)?;
                        Target::Field
                    }
                    Some(Expr::Index(name, _, index)) => {
                        self.index(index)?;
                        Target::Array(self.array(name))
                    }
                    Some(_) => unreachable!(),
                };
                self.add(Op::Getline(*src, t));
            }
        }
        Ok(())
    }

    /// The new value is on the stack (above the index, if any): store it and keep it.
    fn dup_and_store(&mut self, target: &Expr) -> CResult {
        match target {
            Expr::Var(..) => {
                self.add(Op::Dup);
                self.assign(target)?;
            }
            _ => {
                // [index new] -> [new new index] -> store
                self.add(Op::Dup);
                self.add(Op::Rote);
                self.store_indexed(target);
            }
        }
        Ok(())
    }

    fn call(&mut self, b: Builtin, args: &[Expr]) -> CResult {
        use Builtin as B;
        match b {
            B::Split => {
                self.expr(&args[0])?;
                let Expr::Var(name, _) = &args[1] else {
                    unreachable!()
                };
                let a = self.array(name);
                let sep = match args.get(2) {
                    None => SplitSep::Fs,
                    // `//`: every character (gawk).
                    Some(Expr::RegexArg(r)) if r.is_empty() => {
                        self.add(Op::Str(Rc::from(&b""[..])));
                        SplitSep::Dynamic
                    }
                    Some(Expr::RegexArg(r)) => SplitSep::Regex(self.regex(r, Pos::default())?),
                    Some(e) => {
                        self.expr(e)?;
                        SplitSep::Dynamic
                    }
                };
                self.add(Op::Split(a, sep));
            }
            B::Sub | B::Gsub => {
                // [index?] old-value re repl -> count new-value -> store new, keep count.
                let default_target = Expr::Field(Box::new(Expr::Num(0.0)));
                let target = args.get(2).unwrap_or(&default_target);
                match target {
                    Expr::Var(..) => {
                        self.regex_or_str(&args[0])?;
                        self.expr(&args[1])?;
                        self.expr(target)?;
                        self.add(Op::Call(b, 3));
                        // [count new]
                        self.assign(target)?;
                    }
                    _ => {
                        // [index old re repl] -> [index re repl old] -> the call.
                        self.dupe_index_lvalue(target)?;
                        self.regex_or_str(&args[0])?;
                        self.expr(&args[1])?;
                        self.add(Op::Rote);
                        self.add(Op::Call(b, 3));
                        // [index count new] -> [count new index]
                        self.add(Op::Rote);
                        match target {
                            Expr::Field(_) => self.add(Op::StoreFieldSub),
                            Expr::Index(name, _, _) => {
                                let a = self.array(name);
                                self.add(Op::StoreArray(a));
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
            B::Match => {
                self.expr(&args[0])?;
                self.regex_or_str(&args[1])?;
                match args.get(2) {
                    Some(Expr::Var(name, _)) => {
                        let a = self.array(name);
                        self.add(Op::MatchArray(a));
                    }
                    _ => self.add(Op::Call(b, 2)),
                }
            }
            B::Gensub => {
                self.regex_or_str(&args[0])?;
                for a in &args[1..] {
                    self.expr(a)?;
                }
                if args.len() == 3 {
                    self.add(Op::FieldInt(0));
                }
                self.add(Op::Call(b, 4));
            }
            B::Length => match args.first() {
                Some(Expr::Var(name, _)) if self.is_array(name) => {
                    let a = self.array(name);
                    self.add(Op::LengthArray(a));
                }
                Some(e) => {
                    self.expr(e)?;
                    self.add(Op::Call(b, 1));
                }
                None => {
                    self.add(Op::FieldInt(0));
                    self.add(Op::Call(b, 1));
                }
            },
            B::Sprintf => {
                for a in args {
                    self.expr(a)?;
                }
                self.add(Op::Sprintf(args.len() as u32));
            }
            _ => {
                for a in args {
                    self.expr(a)?;
                }
                self.add(Op::Call(b, args.len() as u32));
            }
        }
        Ok(())
    }

    /// A regex argument as a value: a literal pushes a regex value (its source,
    /// recognized by the callee through the regex cache), else the expression.
    fn regex_or_str(&mut self, e: &Expr) -> CResult {
        match e {
            Expr::RegexArg(r) => {
                // Checked now, so a bad literal is a syntax error.
                self.regex(r, Pos::default())?;
                self.add(Op::Str(Rc::from(r.as_slice())));
            }
            _ => self.expr(e)?,
        }
        Ok(())
    }
}

/// The operands of a chain of concatenations, left to right.
fn flatten_concat<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::Binary(l, BinOp::Concat, r) => {
            flatten_concat(l, out);
            out.push(r);
        }
        _ => out.push(e),
    }
}
