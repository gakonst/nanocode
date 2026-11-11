//! From the syntax tree to a program for the matchers: an NFA as instructions, in the
//! style of Thompson and Pike. `Split(a, b)` prefers `a`, which orders the paths the
//! submatch search tries.

use alloc::vec::Vec;

use crate::class::{Set, Unit, lower, upper};
use crate::parse::{Assert, Node, Parsed};
use crate::{Error, ErrorKind, Options};

/// Programs larger than this are refused ("Memory exhausted"), which bounds what
/// repetition counts can blow up to.
const MAX_INSTS: usize = 1 << 20;

#[derive(Clone, Debug)]
pub(crate) enum Inst {
    Lit(Unit),
    /// A letter under `icase`: matches when the lower case of the char is this.
    Fold(Unit),
    Any,
    Set(usize),
    Assert(Assert),
    /// Records the position in capture slot `n`.
    Save(usize),
    Split(usize, usize),
    Jmp(usize),
    Backref(usize),
    /// Records where a loop iteration starts (register `n`).
    ProgressStart(usize),
    /// Fails if the iteration consumed nothing: after the first, loop iterations must
    /// not be empty (GNU repeats `(a*)*` over `aa` once, not twice).
    ProgressCheck(usize),
    Match,
}

#[derive(Clone, Debug)]
pub(crate) struct Prog {
    pub insts: Vec<Inst>,
    pub sets: Vec<Set>,
    pub opts: Options,
    pub slots: usize,
    pub regs: usize,
    pub backrefs: bool,
    /// Every match starts at the start of the text (a leading `^` without `newline`).
    pub anchored: bool,
    /// The ASCII bytes every match starts with (the program's leading literals): a
    /// search skips to where the first one is.
    pub prefix: Vec<u8>,
}

pub(crate) fn compile(parsed: &Parsed, opts: Options) -> Result<Prog, Error> {
    let mut c = C {
        insts: Vec::new(),
        sets: Vec::new(),
        regs: 0,
        opts,
    };
    c.node(&parsed.node)?;
    c.push(Inst::Match)?;
    let anchored = !opts.newline && starts_with_bol(&parsed.node);
    let prefix: Vec<u8> = c
        .insts
        .iter()
        .map_while(|i| match i {
            Inst::Lit(u) if *u < 0x80 => Some(*u as u8),
            _ => None,
        })
        .collect();
    Ok(Prog {
        prefix,
        insts: c.insts,
        sets: c.sets,
        opts,
        slots: 2 * (parsed.groups + 1),
        regs: c.regs,
        backrefs: parsed.backrefs,
        anchored,
    })
}

fn starts_with_bol(n: &Node) -> bool {
    match n {
        Node::Assert(Assert::Bol) => true,
        Node::Cat(items) => items.first().is_some_and(starts_with_bol),
        Node::Group(inner, _) => starts_with_bol(inner),
        Node::Alt(alts) => alts.iter().all(starts_with_bol),
        _ => false,
    }
}

struct C {
    insts: Vec<Inst>,
    sets: Vec<Set>,
    regs: usize,
    opts: Options,
}

impl C {
    fn push(&mut self, i: Inst) -> Result<usize, Error> {
        if self.insts.len() >= MAX_INSTS {
            return Err(Error::new(ErrorKind::Space));
        }
        self.insts.push(i);
        Ok(self.insts.len() - 1)
    }

    fn pc(&self) -> usize {
        self.insts.len()
    }

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.insts[at] {
            Inst::Jmp(t) => *t = target,
            Inst::Split(_, b) => *b = target,
            _ => unreachable!("patching a non-jump"),
        }
    }

    fn node(&mut self, n: &Node) -> Result<(), Error> {
        match n {
            Node::Empty => {}
            Node::Lit(u) => {
                let u = *u;
                let utf8 = self.opts.utf8;
                if self.opts.icase && lower(u, utf8) != upper(u, utf8) {
                    self.push(Inst::Fold(lower(u, utf8)))?;
                } else {
                    self.push(Inst::Lit(u))?;
                }
            }
            Node::Any => {
                self.push(Inst::Any)?;
            }
            Node::Set(s) => {
                self.sets.push((**s).clone());
                self.push(Inst::Set(self.sets.len() - 1))?;
            }
            Node::Assert(a) => {
                self.push(Inst::Assert(*a))?;
            }
            Node::Group(inner, k) => {
                self.push(Inst::Save(2 * k))?;
                self.node(inner)?;
                self.push(Inst::Save(2 * k + 1))?;
            }
            Node::Cat(items) => {
                for i in items {
                    self.node(i)?;
                }
            }
            Node::Alt(alts) => {
                let mut ends = Vec::new();
                for (i, a) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        let split = self.push(Inst::Split(0, 0))?;
                        let body = self.pc();
                        if let Inst::Split(x, _) = &mut self.insts[split] {
                            *x = body;
                        }
                        self.node(a)?;
                        ends.push(self.push(Inst::Jmp(0))?);
                        let next = self.pc();
                        self.patch(split, next);
                    } else {
                        self.node(a)?;
                    }
                }
                let end = self.pc();
                for j in ends {
                    self.patch(j, end);
                }
            }
            Node::Repeat(inner, min, max) => self.repeat(inner, *min, *max)?,
            Node::Backref(n) => {
                self.push(Inst::Backref(*n))?;
            }
        }
        Ok(())
    }

    /// `x?` as code: `split body, exit; body`.
    fn optional(&mut self, inner: &Node) -> Result<usize, Error> {
        let split = self.push(Inst::Split(0, 0))?;
        let body = self.pc();
        if let Inst::Split(x, _) = &mut self.insts[split] {
            *x = body;
        }
        self.node(inner)?;
        Ok(split)
    }

    fn repeat(&mut self, inner: &Node, min: u32, max: Option<u32>) -> Result<(), Error> {
        for _ in 0..min {
            self.node(inner)?;
        }
        match max {
            None => {
                // The first iteration may match the empty string, the others may not.
                let mut first = None;
                if min == 0 {
                    first = Some(self.optional(inner)?);
                }
                let r = self.regs;
                self.regs += 1;
                let top = self.push(Inst::Split(0, 0))?;
                let body = self.pc();
                if let Inst::Split(x, _) = &mut self.insts[top] {
                    *x = body;
                }
                self.push(Inst::ProgressStart(r))?;
                self.node(inner)?;
                self.push(Inst::ProgressCheck(r))?;
                self.push(Inst::Jmp(top))?;
                let exit = self.pc();
                self.patch(top, exit);
                if let Some(f) = first {
                    self.patch(f, exit);
                }
            }
            Some(max) => {
                let mut splits = Vec::new();
                for _ in min..max {
                    splits.push(self.optional(inner)?);
                }
                let exit = self.pc();
                for s in splits {
                    self.patch(s, exit);
                }
            }
        }
        Ok(())
    }
}
