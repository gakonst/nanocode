use super::lex::{Kind, StrPart, Token};
use super::path::{Opt, Part, Path};

pub const NESTING_DEPTH_MAX: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Math {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Default)]
pub enum Term<'s> {
    #[default]
    Id,
    Recurse,
    Num(&'s str),
    Str(Option<&'s str>, Vec<StrPart<'s, Term<'s>>>),
    Arr(Option<Box<Term<'s>>>),
    Obj(Vec<(Term<'s>, Option<Term<'s>>)>),
    Neg(Box<Term<'s>>),
    BinOp(Box<Term<'s>>, BinaryOp<'s>, Box<Term<'s>>),
    Label(&'s str, Box<Term<'s>>),
    Break(&'s str),
    Fold(&'s str, Box<Term<'s>>, Pattern<'s>, Vec<Term<'s>>),
    TryCatch(Box<Term<'s>>, Option<Box<Term<'s>>>),
    IfThenElse(Vec<(Term<'s>, Term<'s>)>, Option<Box<Term<'s>>>),
    Def(Vec<Def<'s>>, Box<Term<'s>>),
    Call(&'s str, Vec<Term<'s>>),
    Var(&'s str),
    Path(Box<Term<'s>>, Path<Term<'s>>),
}

#[derive(Debug)]
pub enum Pattern<'s> {
    Var(&'s str),
    Arr(Vec<Pattern<'s>>),
    Obj(Vec<(Term<'s>, Pattern<'s>)>),
}

#[derive(Debug)]
pub enum BinaryOp<'s> {
    Pipe(Option<Pattern<'s>>),
    Comma,
    Alt,
    Or,
    And,
    Math(Math),
    Cmp(Cmp),
    Assign,
    Update,
    UpdateMath(Math),
    UpdateAlt,
}

#[derive(Debug)]
pub struct Def<'s> {
    pub name: &'s str,
    pub args: Vec<&'s str>,
    pub body: Term<'s>,
}

#[derive(Debug, Default)]
pub struct Module<'s, B> {
    pub deps: Vec<Dep<'s>>,
    pub body: B,
}

#[derive(Debug)]
pub struct Dep<'s> {
    pub path: &'s str,
    pub name: Option<&'s str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Unexpected,
    Depth,
}

type Found<'s, 't> = (Problem, Option<&'t Token<'s>>);
type Result<'s, 't, T> = core::result::Result<T, Found<'s, 't>>;

impl<'s> Term<'s> {
    pub fn from_str(text: &'s str) -> Self {
        Term::Str(None, Vec::from([StrPart::Str(text)]))
    }
}

impl<'s> Pattern<'s> {
    pub fn vars(&self, out: &mut Vec<&'s str>) {
        let mut pending = Vec::from([self]);
        while let Some(pattern) = pending.pop() {
            match pattern {
                Pattern::Var(name) => out.push(*name),
                Pattern::Arr(items) => pending.extend(items.iter().rev()),
                Pattern::Obj(entries) => pending.extend(entries.iter().rev().map(|(_, p)| p)),
            }
        }
    }
}

impl BinaryOp<'_> {
    fn precedence(&self) -> usize {
        match self {
            BinaryOp::Pipe(None) => 0,
            BinaryOp::Comma => 1,
            BinaryOp::Pipe(Some(_)) => 2,
            BinaryOp::Assign | BinaryOp::Update | BinaryOp::UpdateMath(_) | BinaryOp::UpdateAlt => {
                3
            }
            BinaryOp::Alt => 4,
            BinaryOp::Or => 5,
            BinaryOp::And => 6,
            BinaryOp::Cmp(Cmp::Eq | Cmp::Ne) => 7,
            BinaryOp::Cmp(_) => 8,
            BinaryOp::Math(Math::Add | Math::Sub) => 9,
            BinaryOp::Math(Math::Mul | Math::Div) => 10,
            BinaryOp::Math(Math::Rem) => 11,
        }
    }

    fn right_associative(&self) -> bool {
        matches!(
            self,
            BinaryOp::Pipe(_)
                | BinaryOp::Assign
                | BinaryOp::Update
                | BinaryOp::UpdateMath(_)
                | BinaryOp::UpdateAlt
        )
    }

    fn binds_before(&self, next: &BinaryOp<'_>) -> bool {
        let (this, that) = (self.precedence(), next.precedence());
        this > that || (this == that && !next.right_associative())
    }
}

fn reduce_top<'s>(operands: &mut Vec<Term<'s>>, operators: &mut Vec<BinaryOp<'s>>) {
    let op = operators.pop().expect("an operator to reduce");
    let rhs = operands.pop().expect("a right operand");
    let lhs = operands.pop().expect("a left operand");
    operands.push(Term::BinOp(Box::new(lhs), op, Box::new(rhs)));
}

fn climb_plain<'s>(head: Term<'s>, tail: Vec<(BinaryOp<'s>, Term<'s>)>) -> Term<'s> {
    let mut operands = Vec::from([head]);
    let mut operators: Vec<BinaryOp<'s>> = Vec::new();
    for (op, term) in tail {
        while operators.last().is_some_and(|top| top.binds_before(&op)) {
            reduce_top(&mut operands, &mut operators);
        }
        operators.push(op);
        operands.push(term);
    }
    while !operators.is_empty() {
        reduce_top(&mut operands, &mut operators);
    }
    let term = operands.pop().expect("one term left");
    assert!(operands.is_empty());
    term
}

type Segment<'s> = (
    Option<BinaryOp<'s>>,
    Term<'s>,
    Vec<(BinaryOp<'s>, Term<'s>)>,
);

fn climb<'s>(head: Term<'s>, tail: Vec<(BinaryOp<'s>, Term<'s>)>) -> Term<'s> {
    let mut segments: Vec<Segment<'s>> = Vec::from([(None, head, Vec::new())]);
    for (op, term) in tail {
        if matches!(op, BinaryOp::Pipe(Some(_))) {
            segments.push((Some(op), term, Vec::new()));
        } else {
            segments.last_mut().expect("a segment").2.push((op, term));
        }
    }
    loop {
        let (binding, head, tail) = segments.pop().expect("a segment");
        let rhs = climb_plain(head, tail);
        let Some(binding) = binding else {
            assert!(segments.is_empty());
            return rhs;
        };
        segments
            .last_mut()
            .expect("the segment before a binding")
            .2
            .push((binding, rhs));
    }
}

pub struct Parser<'s, 't> {
    tokens: &'t [Token<'s>],
    position: usize,
    errors: Vec<Found<'s, 't>>,
    level: usize,
    deepest: usize,
}

impl<'s, 't> Parser<'s, 't> {
    pub fn new(tokens: &'t [Token<'s>]) -> Self {
        Parser {
            tokens,
            position: 0,
            errors: Vec::new(),
            level: 0,
            deepest: 0,
        }
    }

    pub fn parse<T: Default>(
        mut self,
        f: impl FnOnce(&mut Self) -> Result<'s, 't, T>,
    ) -> core::result::Result<T, Vec<Found<'s, 't>>> {
        let value = self.finish("", f);
        if let Some(deep) = self.errors.iter().find(|(p, _)| *p == Problem::Depth) {
            return Err(Vec::from([*deep]));
        }
        if self.errors.is_empty() {
            Ok(value)
        } else {
            Err(self.errors)
        }
    }

    fn next(&mut self) -> Option<&'t Token<'s>> {
        let token = self.tokens.get(self.position)?;
        self.position += 1;
        Some(token)
    }

    fn rest(&self) -> &'t [Token<'s>] {
        &self.tokens[self.position..]
    }

    fn verify_last(&self, last: &'static str) -> Result<'s, 't, ()> {
        match (self.rest(), last) {
            ([], "") => Ok(()),
            ([token], last) if token.text == last => Ok(()),
            ([], _) => Err((Problem::Unexpected, None)),
            ([next, ..], _) => Err((Problem::Unexpected, Some(next))),
        }
    }

    fn with_tokens<T>(&mut self, tokens: &'t [Token<'s>], f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = (
            core::mem::replace(&mut self.tokens, tokens),
            core::mem::replace(&mut self.position, 0),
        );
        let value = f(self);
        (self.tokens, self.position) = saved;
        value
    }

    fn finish<T: Default>(
        &mut self,
        last: &'static str,
        f: impl FnOnce(&mut Self) -> Result<'s, 't, T>,
    ) -> T {
        let result = f(self).and_then(|value| {
            self.verify_last(last)?;
            Ok(value)
        });
        result.unwrap_or_else(|e| {
            self.errors.push(e);
            T::default()
        })
    }

    fn with<T: Default>(
        &mut self,
        tokens: &'t [Token<'s>],
        last: &'static str,
        f: impl FnOnce(&mut Self) -> Result<'s, 't, T>,
    ) -> T {
        self.with_tokens(tokens, |p| p.finish(last, f))
    }

    fn maybe<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let saved = self.position;
        let value = f(self);
        if value.is_none() {
            self.position = saved;
        }
        value
    }

    fn try_maybe<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<'s, 't, Option<T>>,
    ) -> Result<'s, 't, Option<T>> {
        let saved = self.position;
        let value = f(self)?;
        if value.is_none() {
            self.position = saved;
        }
        Ok(value)
    }

    fn many1<T>(
        &mut self,
        f: impl Fn(&mut Self) -> Result<'s, 't, T>,
        separator: &str,
        trailing: bool,
        last: &str,
    ) -> Result<'s, 't, Vec<T>> {
        let mut items = Vec::from([f(self)?]);
        loop {
            match self.next() {
                Some(token) if token.text == last => break,
                Some(token) if token.text == separator && trailing && self.next_is(last) => break,
                Some(token) if token.text == separator => items.push(f(self)?),
                next => return Err((Problem::Unexpected, next)),
            }
        }
        Ok(items)
    }

    fn next_is(&mut self, text: &str) -> bool {
        self.maybe(|p| p.next().filter(|token| token.text == text))
            .is_some()
    }

    fn object_items<T>(
        &mut self,
        f: impl Fn(&mut Self) -> Result<'s, 't, T>,
    ) -> Result<'s, 't, Vec<T>> {
        self.many1(f, ",", true, "}")
    }

    fn args<T>(&mut self, f: fn(&mut Self) -> Result<'s, 't, T>) -> Vec<T> {
        self.maybe(|p| match p.next() {
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('(') => {
                Some(p.with(tokens, "", |p| p.many1(f, ";", false, ")")))
            }
            _ => None,
        })
        .unwrap_or_default()
    }

    fn op(&mut self, with_comma: bool) -> Result<'s, 't, Option<BinaryOp<'s>>> {
        self.try_maybe(|p| {
            let Some(token) = p.next() else {
                return Ok(None);
            };
            Ok(Some(match token.text {
                "|" => BinaryOp::Pipe(None),
                "as" => {
                    let pattern = p.pattern()?;
                    p.just("|")?;
                    BinaryOp::Pipe(Some(pattern))
                }
                "," if with_comma => BinaryOp::Comma,
                "+" => BinaryOp::Math(Math::Add),
                "-" => BinaryOp::Math(Math::Sub),
                "*" => BinaryOp::Math(Math::Mul),
                "/" => BinaryOp::Math(Math::Div),
                "%" => BinaryOp::Math(Math::Rem),
                "=" => BinaryOp::Assign,
                "|=" => BinaryOp::Update,
                "+=" => BinaryOp::UpdateMath(Math::Add),
                "-=" => BinaryOp::UpdateMath(Math::Sub),
                "*=" => BinaryOp::UpdateMath(Math::Mul),
                "/=" => BinaryOp::UpdateMath(Math::Div),
                "%=" => BinaryOp::UpdateMath(Math::Rem),
                "<" => BinaryOp::Cmp(Cmp::Lt),
                ">" => BinaryOp::Cmp(Cmp::Gt),
                "<=" => BinaryOp::Cmp(Cmp::Le),
                ">=" => BinaryOp::Cmp(Cmp::Ge),
                "==" => BinaryOp::Cmp(Cmp::Eq),
                "!=" => BinaryOp::Cmp(Cmp::Ne),
                "//" => BinaryOp::Alt,
                "//=" => BinaryOp::UpdateAlt,
                "or" => BinaryOp::Or,
                "and" => BinaryOp::And,
                _ => return Ok(None),
            }))
        })
    }

    fn char0(&mut self, c: char) -> Option<&'s str> {
        self.maybe(|p| match p.next() {
            Some(token) if token.text.len() == c.len_utf8() && token.text.starts_with(c) => {
                Some(token.text)
            }
            _ => None,
        })
    }

    fn dot(&mut self) -> Option<&'s str> {
        self.maybe(|p| match p.next() {
            Some(token) if token.text != ".." => token.text.strip_prefix('.'),
            _ => None,
        })
    }

    fn just(&mut self, text: &'static str) -> Result<'s, 't, &'s str> {
        match self.next() {
            Some(token) if token.text == text => Ok(token.text),
            next => Err((Problem::Unexpected, next)),
        }
    }

    fn var(&mut self) -> Result<'s, 't, &'s str> {
        match self.next() {
            Some(Token {
                text,
                kind: Kind::Var,
            }) => Ok(text),
            next => Err((Problem::Unexpected, next)),
        }
    }

    fn pattern(&mut self) -> Result<'s, 't, Pattern<'s>> {
        match self.next() {
            Some(Token {
                text,
                kind: Kind::Var,
            }) => Ok(Pattern::Var(text)),
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('[') => {
                Ok(Pattern::Arr(self.with(tokens, "", |p| {
                    p.many1(Self::pattern, ",", false, "]")
                })))
            }
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('{') => Ok(Pattern::Obj(
                self.with(tokens, "", |p| p.object_items(Self::pattern_entry)),
            )),
            next => Err((Problem::Unexpected, next)),
        }
    }

    fn previous(&self) -> Option<&'t Token<'s>> {
        self.position
            .checked_sub(1)
            .and_then(|at| self.tokens.get(at))
    }

    fn term_with_comma(&mut self, with_comma: bool) -> Result<'s, 't, Term<'s>> {
        let outer = core::mem::replace(&mut self.deepest, self.level);
        let term = self.chain(with_comma);
        self.deepest = self.deepest.max(outer);
        term
    }

    fn chain(&mut self, with_comma: bool) -> Result<'s, 't, Term<'s>> {
        let head = self.atom()?;
        let mut tail = Vec::new();
        while let Some(op) = self.op(with_comma)? {
            tail.push((op, self.atom()?));
            self.deepest += 1;
            if self.deepest > NESTING_DEPTH_MAX {
                return Err((Problem::Depth, self.previous()));
            }
        }
        Ok(climb(head, tail))
    }

    fn atom(&mut self) -> Result<'s, 't, Term<'s>> {
        if self.level >= NESTING_DEPTH_MAX {
            return Err((Problem::Depth, self.rest().first()));
        }
        self.level += 1;
        self.deepest = self.deepest.max(self.level);
        let term = self.atom_body();
        self.level -= 1;
        term
    }

    fn atom_body(&mut self) -> Result<'s, 't, Term<'s>> {
        let Some(token) = self.next() else {
            return Err((Problem::Unexpected, None));
        };
        let term = match (token.text, &token.kind) {
            ("-", _) => Term::Neg(Box::new(self.atom()?)),
            ("def", _) => {
                let head = self.def_tail()?;
                let mut defs = Vec::from([head]);
                defs.extend(self.defs()?);
                let body = self.term()?;
                Term::Def(defs, Box::new(body))
            }
            ("if", _) => self.if_then_else()?,
            ("try", _) => {
                let body = self.atom()?;
                let catch = self.try_maybe(|p| match p.next() {
                    Some(token) if token.text == "catch" => Ok(Some(p.atom()?)),
                    _ => Ok(None),
                })?;
                Term::TryCatch(Box::new(body), catch.map(Box::new))
            }
            ("label", _) => {
                let name = self.var()?;
                self.just("|")?;
                let body = self.term()?;
                Term::Label(name, Box::new(body))
            }
            ("break", _) => Term::Break(self.var()?),
            (fold @ ("reduce" | "foreach"), _) => {
                let source = self.atom()?;
                self.just("as")?;
                let pattern = self.pattern()?;
                let args = self.args(Self::term);
                Term::Fold(fold, Box::new(source), pattern, args)
            }
            (name, Kind::Var) => Term::Var(name),
            (name, Kind::Fmt) => {
                let parts = self.maybe(|p| match p.next() {
                    Some(Token {
                        kind: Kind::Str(parts),
                        ..
                    }) => Some(p.str_parts(parts)),
                    _ => None,
                });
                match parts {
                    None => Term::Call(name, self.args(Self::term)),
                    Some(parts) => Term::Str(Some(name), parts),
                }
            }
            (name, Kind::Word) => Term::Call(name, self.args(Self::term)),
            ("..", _) => Term::Recurse,
            (text, Kind::Sym) if text.starts_with('.') => self.dot_path(text)?,
            (text, Kind::Num) => Term::Num(text),
            (text, Kind::Block(tokens)) => self.block(text, tokens),
            (_, Kind::Str(parts)) => Term::Str(None, self.str_parts(parts)),
            _ => return Err((Problem::Unexpected, Some(token))),
        };
        let term = match self.opt() {
            Opt::Optional => Term::TryCatch(Box::new(term), None),
            Opt::Essential => term,
        };
        let path = self.path()?;
        Ok(if path.0.is_empty() {
            term
        } else {
            Term::Path(Box::new(term), path)
        })
    }

    fn if_then_else(&mut self) -> Result<'s, 't, Term<'s>> {
        let if_then = |p: &mut Self| -> Result<'s, 't, (Term<'s>, Term<'s>)> {
            let condition = p.term()?;
            p.just("then")?;
            Ok((condition, p.term()?))
        };
        let mut branches = Vec::from([if_then(self)?]);
        let otherwise = loop {
            match self.next() {
                Some(token) if token.text == "elif" => branches.push(if_then(self)?),
                Some(token) if token.text == "else" => {
                    let otherwise = self.term()?;
                    self.just("end")?;
                    break Some(otherwise);
                }
                Some(token) if token.text == "end" => break None,
                next => return Err((Problem::Unexpected, next)),
            }
        };
        Ok(Term::IfThenElse(branches, otherwise.map(Box::new)))
    }

    fn dot_path(&mut self, text: &'s str) -> Result<'s, 't, Term<'s>> {
        let key = if text.len() > 1 {
            Some(Term::from_str(&text[1..]))
        } else {
            self.maybe(|p| p.str_key().ok())
        };
        let Some(key) = key else {
            return Ok(Term::Id);
        };
        let mut parts = Vec::from([(Part::Index(key), self.opt())]);
        parts.extend(self.path()?.0);
        Ok(Term::Path(Box::new(Term::Id), Path(parts)))
    }

    fn block(&mut self, text: &'s str, tokens: &'t [Token<'s>]) -> Term<'s> {
        let only_close = |close: &str| matches!(tokens, [token] if token.text == close);
        match &text[..1] {
            "[" if only_close("]") => Term::Arr(None),
            "{" if only_close("}") => Term::Obj(Vec::new()),
            "[" => Term::Arr(Some(Box::new(self.with(tokens, "]", Self::term)))),
            "{" => self.with(tokens, "", |p| {
                p.object_items(Self::object_entry).map(Term::Obj)
            }),
            "(" => self.with(tokens, ")", Self::term),
            other => unreachable!("a block opens with a bracket, not {other:?}"),
        }
    }

    pub fn term(&mut self) -> Result<'s, 't, Term<'s>> {
        self.term_with_comma(true)
    }

    fn pattern_entry(&mut self) -> Result<'s, 't, (Term<'s>, Pattern<'s>)> {
        let saved = self.position;
        let key = match self.next() {
            Some(Token {
                text,
                kind: Kind::Var,
            }) => return Ok((Term::from_str(&text[1..]), Pattern::Var(text))),
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('(') => self.with(tokens, ")", Self::term),
            Some(Token {
                text,
                kind: Kind::Word,
            }) if !text.contains("::") => Term::from_str(text),
            _ => {
                self.position = saved;
                self.str_key()?
            }
        };
        self.just(":")?;
        Ok((key, self.pattern()?))
    }

    fn object_entry(&mut self) -> Result<'s, 't, (Term<'s>, Option<Term<'s>>)> {
        let saved = self.position;
        let key = match self.next() {
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('(') => {
                let key = self.with(tokens, ")", Self::term);
                self.just(":")?;
                return Ok((key, Some(self.term_with_comma(false)?)));
            }
            Some(Token {
                text,
                kind: Kind::Var,
            }) => Term::Var(text),
            Some(Token {
                text,
                kind: Kind::Word,
            }) if !text.contains("::") => Term::from_str(text),
            _ => {
                self.position = saved;
                self.str_key()?
            }
        };
        let value = match self.char0(':') {
            Some(_) => Some(self.term_with_comma(false)?),
            None => None,
        };
        Ok((key, value))
    }

    fn str_parts(&mut self, parts: &'t [StrPart<'s, Token<'s>>]) -> Vec<StrPart<'s, Term<'s>>> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            out.push(match part {
                StrPart::Str(text) => StrPart::Str(text),
                StrPart::Char(c) => StrPart::Char(*c),
                StrPart::Term(Token {
                    text,
                    kind: Kind::Block(tokens),
                }) if text.starts_with('(') => StrPart::Term(self.with(tokens, ")", Self::term)),
                StrPart::Term(other) => {
                    unreachable!("an interpolation is a parenthesized block, not {other:?}")
                }
            });
        }
        out
    }

    fn path(&mut self) -> Result<'s, 't, Path<Term<'s>>> {
        let mut parts = Vec::new();
        while let Some(part) = self.path_part_opt() {
            parts.push(part);
        }
        while let Some(key) = self.dot() {
            parts.push(if key.is_empty() {
                match self.path_part_opt() {
                    Some(part) => part,
                    None => (Part::Index(self.str_key()?), self.opt()),
                }
            } else {
                (Part::Index(Term::from_str(key)), self.opt())
            });
            while let Some(part) = self.path_part_opt() {
                parts.push(part);
            }
        }
        Ok(Path(parts))
    }

    fn path_part(&mut self) -> Result<'s, 't, Part<Term<'s>>> {
        let done = |p: &Self| matches!(p.rest(), [token] if token.text == "]");
        Ok(if done(self) {
            Part::Range(None, None)
        } else if self.char0(':').is_some() {
            Part::Range(None, Some(self.term()?))
        } else {
            let term = self.term()?;
            if self.char0(':').is_some() {
                if done(self) {
                    Part::Range(Some(term), None)
                } else {
                    Part::Range(Some(term), Some(self.term()?))
                }
            } else {
                Part::Index(term)
            }
        })
    }

    fn path_part_opt(&mut self) -> Option<(Part<Term<'s>>, Opt)> {
        let part = self.maybe(|p| match p.next() {
            Some(Token {
                text,
                kind: Kind::Block(tokens),
            }) if text.starts_with('[') => Some(p.with(tokens, "]", Self::path_part)),
            _ => None,
        })?;
        Some((part, self.opt()))
    }

    fn str_key(&mut self) -> Result<'s, 't, Term<'s>> {
        match self.next() {
            Some(Token {
                text,
                kind: Kind::Fmt,
            }) => match self.next() {
                Some(Token {
                    kind: Kind::Str(parts),
                    ..
                }) => Ok(Term::Str(Some(text), self.str_parts(parts))),
                next => Err((Problem::Unexpected, next)),
            },
            Some(Token {
                kind: Kind::Str(parts),
                ..
            }) => Ok(Term::Str(None, self.str_parts(parts))),
            next => Err((Problem::Unexpected, next)),
        }
    }

    fn opt(&mut self) -> Opt {
        let mut opt = Opt::Essential;
        while self.char0('?').is_some() {
            opt = Opt::Optional;
        }
        opt
    }

    pub fn defs(&mut self) -> Result<'s, 't, Vec<Def<'s>>> {
        let mut defs = Vec::new();
        while self.maybe(|p| p.just("def").ok()).is_some() {
            defs.push(self.def_tail()?);
        }
        Ok(defs)
    }

    fn def_tail(&mut self) -> Result<'s, 't, Def<'s>> {
        let name = match self.next() {
            Some(Token {
                text,
                kind: Kind::Word | Kind::Fmt,
            }) if !text.contains("::") => *text,
            next => return Err((Problem::Unexpected, next)),
        };
        let args = self.args(|p| match p.next() {
            Some(Token {
                text,
                kind: Kind::Word | Kind::Var,
            }) if !text.contains("::") => Ok(*text),
            next => Err((Problem::Unexpected, next)),
        });
        self.just(":")?;
        let body = self.term()?;
        self.just(";")?;
        Ok(Def { name, args, body })
    }

    fn bare_str(&mut self) -> Result<'s, 't, &'s str> {
        match self.next() {
            Some(
                token @ Token {
                    kind: Kind::Str(parts),
                    ..
                },
            ) => match parts[..] {
                [StrPart::Str(text)] => Ok(text),
                _ => Err((Problem::Unexpected, Some(token))),
            },
            next => Err((Problem::Unexpected, next)),
        }
    }

    fn dep(&mut self, import: bool) -> Result<'s, 't, Dep<'s>> {
        let path = self.bare_str()?;
        let name = if import {
            self.just("as")?;
            match self.next() {
                Some(Token {
                    text,
                    kind: Kind::Word | Kind::Var,
                }) => Some(*text),
                next => return Err((Problem::Unexpected, next)),
            }
        } else {
            None
        };
        if !matches!(self.rest(), [token, ..] if token.text == ";") {
            self.term()?;
        }
        self.just(";")?;
        Ok(Dep { path, name })
    }

    pub fn module<B>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<'s, 't, B>,
    ) -> Result<'s, 't, Module<'s, B>> {
        let meta = self.maybe(|p| match p.next() {
            Some(token) if token.text == "module" => Some(p.term().and_then(|_| p.just(";"))),
            _ => None,
        });
        if let Some(meta) = meta {
            meta?;
        }
        let mut deps = Vec::new();
        loop {
            let dep = self.maybe(|p| match p.next() {
                Some(token) if token.text == "include" => Some(p.dep(false)),
                Some(token) if token.text == "import" => Some(p.dep(true)),
                _ => None,
            });
            match dep {
                Some(dep) => deps.push(dep?),
                None => break,
            }
        }
        let body = f(self)?;
        Ok(Module { deps, body })
    }
}

pub fn found_text<'s>(found: Option<&Token<'s>>, code: &'s str) -> &'s str {
    found.map_or(&code[code.len()..], |token| token.text)
}
