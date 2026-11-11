//! Completions in temporaries (D2733, docs/compiler.md 7.4): how `break`, `continue`,
//! `return` and exceptions leave a finally block, a `using` list's disposal or a for-of
//! loop whose iterator must be closed.
//!
//! Each such block (a `BlockEnv` of kind `Finally` or `Iter`) is code of its own,
//! entered with a jump after its completion is stored in two temporaries:
//! `completion.kind@S` and `completion.value@S` (S the construct's offset: the `finally`
//! keyword, the `for`, the `using` list's first binding). The kind is an int:
//!
//! - 0: normal (the block ends by falling out of it),
//! - 1: throw (the value is the exception),
//! - 2: return (the value is the return value),
//! - 3 + 2·o: `break` to the statement at offset o from the function's start, 4 + 2·o:
//!   `continue` to it (a loop's keyword, a labeled statement's first label).
//!
//! The block's end dispatches on the kind: falls through to the code after it, rethrows,
//! or continues the exit, which may enter the next block out (the kind of a break stays
//! the same all the way). So a suspended frame keeps its completion in named slots
//! (D2720) and no return address: this replaced `gosub`/`ret` (D2627).
//!
//! The kind is written last, after the value, and only abrupt completions with a value
//! write the value: for the suspension maps' liveness, a write of the kind defines the
//! value too (compiler/temps.rs), and the rethrow clears it. (A for-of whose loop no jump
//! leaves never uses its temporaries: they are dead everywhere.)
use crate::PushOut;

use alloc::vec::Vec;

use super::parser::{BlockEnv, BlockKind, Parser};
use crate::bytecode::Label;
use crate::heap::Atom;

/// Kind codes of the completions that are no exit (see the module docs).
const NORMAL: i32 = 0;
const THROW: i32 = 1;
const RETURN: i32 = 2;

/// The completion of a `Finally` or `Iter` block.
pub(super) struct Completion {
    /// Its temporaries.
    kind: Atom,
    value: Atom,
    /// Script and eval code: the try statement's completion value while its finally
    /// block runs (`Atom::NULL`: none).
    pub saved: Atom,
    /// The exits that entered it, in order of first use: [`Exit::encode`]d.
    exits: Vec<u32>,
}

/// Where an abrupt completion goes once the blocks it leaves have run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Exit {
    /// Return the value on top of the stack.
    Return,
    /// Jump to `blocks[block]`'s break target (`cont`: its continue target).
    Jump { block: usize, cont: bool },
}

impl Exit {
    fn encode(self) -> u32 {
        match self {
            Exit::Return => u32::MAX,
            Exit::Jump { block, cont } => (block as u32) << 1 | u32::from(cont),
        }
    }

    fn decode(x: u32) -> Exit {
        match x {
            u32::MAX => Exit::Return,
            x => Exit::Jump {
                block: (x >> 1) as usize,
                cont: x & 1 != 0,
            },
        }
    }
}

impl Completion {
    /// Whether an exit entered the block.
    pub fn has_exits(&self) -> bool {
        !self.exits.is_empty()
    }
}

impl Parser<'_, '_> {
    /// Pushes a break/continue target of kind `kind`, the statement at `pos`. A
    /// `Finally` or `Iter` block gets its completion, its temporaries named by `pos`.
    /// (`labels` by reference, taken: a copy of a `Vec` argument would be in the frame
    /// of the recursive statement parser calling this, D1941.)
    #[inline(never)]
    pub(super) fn push_block(
        &mut self,
        labels: &mut Vec<Atom>,
        kind: BlockKind,
        break_label: Label,
        continue_label: Option<Label>,
        pos: u32,
    ) {
        let completion = matches!(kind, BlockKind::Finally | BlockKind::Iter).then(|| Completion {
            kind: self.temp("completion.kind", pos),
            value: self.temp("completion.value", pos),
            saved: Atom::NULL,
            exits: Vec::new(),
        });
        self.blocks.push_out(BlockEnv {
            labels: core::mem::take(labels),
            kind,
            break_label,
            continue_label,
            used: false,
            pos,
            completion,
        });
    }

    /// Pops the innermost block: whether a jump used its break target. (In place: a
    /// popped `BlockEnv` would take a slot in the frame of the recursive statement
    /// parser calling this, D1941.)
    #[inline(never)]
    pub(super) fn pop_block(&mut self) -> bool {
        let n = self.blocks.len() - 1;
        let used = self.blocks[n].used;
        self.blocks.truncate(n);
        used
    }

    /// Pushes a `Finally` or `Iter` block entered at `entry`, the statement at `pos`.
    #[inline(never)]
    pub(super) fn push_completion_block(&mut self, kind: BlockKind, entry: Label, pos: u32) {
        self.push_block(&mut Vec::new(), kind, entry, None, pos);
    }

    /// The completion of `blocks[b]`.
    fn completion_of(&self, b: usize) -> &Completion {
        self.blocks[b].completion.as_ref().expect("a completion")
    }

    /// Stores completion `code` in `kind` (written last).
    fn set_kind(&mut self, kind: Atom, code: i32) {
        self.emit_number(f64::from(code));
        self.temp_put(kind);
    }

    /// The normal completion of `blocks[b]` (a `Finally` block); its code follows or is
    /// jumped to.
    pub(super) fn complete_normally(&mut self, b: usize) {
        let kind = self.completion_of(b).kind;
        self.set_kind(kind, NORMAL);
    }

    /// An exception handler of a `Finally` block (its completion `c`, its code at
    /// `entry`), with the exception on the stack: enters the block with it.
    pub(super) fn complete_throwing(&mut self, c: &Completion, entry: Label) {
        self.temp_put(c.value);
        self.set_kind(c.kind, THROW);
        self.code().jump(entry);
    }

    /// Leaves the finally blocks and for-of loops of `self.blocks[from..]` for `exit`,
    /// innermost first. For [`Exit::Return`] the value is on top of the stack (and
    /// nothing else: statements keep nothing there, D2732). Enters the innermost block with the completion, whose
    /// end continues the exit ([`Parser::end_completion`]); without one, it is taken
    /// here.
    #[inline(never)]
    pub(super) fn emit_unwind(&mut self, from: usize, exit: Exit) {
        let inner = (from..self.blocks.len())
            .rev()
            .find(|&i| matches!(self.blocks[i].kind, BlockKind::Finally | BlockKind::Iter));
        let Some(b) = inner else {
            self.take_exit(exit);
            return;
        };
        let (kind, value) = (self.completion_of(b).kind, self.completion_of(b).value);
        if exit == Exit::Return {
            // The value goes into the completion before the stack is cut.
            self.temp_put(value);
        }
        let code = self.exit_code(exit);
        self.set_kind(kind, code);
        let entry = self.blocks[b].break_label;
        self.code().jump(entry);
        let c = self.blocks[b].completion.as_mut().expect("a completion");
        let x = exit.encode();
        if !c.exits.contains(&x) {
            c.exits.push_out(x);
        }
    }

    /// The kind code of `exit` (see the module docs).
    fn exit_code(&self, exit: Exit) -> i32 {
        match exit {
            Exit::Return => RETURN,
            Exit::Jump { block, cont } => {
                let off = self.blocks[block].pos.saturating_sub(self.f.src_start);
                3 + 2 * off as i32 + i32::from(cont)
            }
        }
    }

    /// `exit` once no block is left to run: the jump or the return.
    fn take_exit(&mut self, exit: Exit) {
        match exit {
            Exit::Return => self.emit_return_value(),
            Exit::Jump { block, cont } => {
                let b = &self.blocks[block];
                let target = if cont {
                    b.continue_label.expect("a continue target")
                } else {
                    b.break_label
                };
                self.code().jump(target);
            }
        }
    }

    /// The end of the completion block of `c` (its env popped): dispatch on the kind. A
    /// normal completion goes to `normal` (`None`: the block has none); with `throws`
    /// the block may have been entered by an exception, which is rethrown.
    #[inline(never)]
    pub(super) fn end_completion(&mut self, c: &Completion, normal: Option<Label>, throws: bool) {
        if let Some(l) = normal {
            self.temp_get(c.kind);
            self.code().jump_if_false(l);
        }
        let n = c.exits.len();
        for (i, &x) in c.exits.iter().enumerate() {
            let exit = Exit::decode(x);
            // The last exit needs no test when nothing else can be left.
            let l_next = self.new_label();
            if throws || i + 1 < n {
                self.temp_get(c.kind);
                let code = self.exit_code(exit);
                self.emit_number(f64::from(code));
                self.code().strict_eq();
                self.code().jump_if_false(l_next);
            }
            let from = match exit {
                Exit::Return => {
                    self.temp_get(c.value);
                    0
                }
                Exit::Jump { block, .. } => block + 1,
            };
            self.emit_unwind(from, exit);
            self.place(l_next);
        }
        if throws {
            self.temp_get(c.value);
            self.code().undefined();
            self.temp_put(c.value);
            self.code().throw();
        }
    }
}
