// Portions of this file are adapted from brush-parser's `ast.rs` and `word.rs`
// (https://github.com/reubeno/brush, commit 943553433ae0):
// Copyright (c) 2024 reuben olinsky. Licensed under the MIT License (see NOTICE).

//! The syntax tree.
//!
//! Commands and lists live in arenas inside [`Ast`] and refer to each other by index
//! ([`ListId`], [`CmdId`], [`HereDocId`]); words, redirections and expressions are owned
//! by the command that contains them. The engine keeps `(Arc<Ast>, id)` pairs in its
//! frames and function table; the classifier walks from [`Ast::root`].
//!
//! Every node has a [`Span`] into [`Ast::source`]. Text is never copied out of the source:
//! literal word parts are spans, and their meaning is fixed by the part kind (see
//! [`Part`]).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Index;

use crate::source::Source;
use crate::span::Span;

/// Index of a [`List`] in its [`Ast`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ListId(pub(crate) u32);

/// Index of a [`Command`] in its [`Ast`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CmdId(pub(crate) u32);

/// Index of a [`HereDoc`] in its [`Ast`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HereDocId(pub(crate) u32);

impl ListId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl CmdId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl HereDocId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A parsed script (or one chunk of it, see [`crate::Parser`]).
#[derive(Clone, Debug)]
pub struct Ast {
    pub(crate) source: Source,
    pub(crate) lists: Vec<List>,
    pub(crate) cmds: Vec<Command>,
    pub(crate) heredocs: Vec<HereDoc>,
    pub(crate) root: ListId,
    /// Length of the script; the source continues with alias texts after it.
    pub(crate) script_len: u32,
    pub(crate) alias_uses: Vec<crate::alias::AliasUse>,
    /// Where the next chunk starts.
    pub(crate) chunk_end: usize,
}

impl Ast {
    /// The whole source as text: its UTF-8 view (see [`Source`]), which is the source
    /// itself unless it is not valid UTF-8. Spans are offsets into
    /// [`Ast::source_bytes`]; slice with [`Ast::text`], not with this.
    pub fn source(&self) -> &str {
        self.source.view()
    }

    /// The original bytes all spans point into.
    pub fn source_bytes(&self) -> &[u8] {
        self.source.bytes()
    }

    /// The source with its UTF-8 view, shared by all chunks of one script.
    pub fn source_info(&self) -> &Source {
        &self.source
    }

    /// The length of the script text. A chunk that expanded aliases holds their texts
    /// after it: spans at or past this offset are text that came from an alias.
    pub fn script_len(&self) -> u32 {
        self.script_len
    }

    /// The alias expansions of this chunk, in the order they were read.
    pub fn alias_uses(&self) -> &[crate::alias::AliasUse] {
        &self.alias_uses
    }

    /// `span` lifted out of alias text onto the words the aliases replaced: for line
    /// numbers and messages. Spans in the script are returned as they are.
    pub fn original_span(&self, span: Span) -> Span {
        if self.alias_uses.is_empty() {
            return span;
        }
        crate::parser::original_span(&self.alias_uses, span)
    }

    /// Whether the source is valid UTF-8 (then text and bytes agree everywhere).
    pub fn is_utf8(&self) -> bool {
        self.source.is_utf8()
    }

    /// The original bytes of a span.
    pub fn bytes(&self, span: Span) -> &[u8] {
        self.source.slice(span)
    }

    /// Turns text from [`Ast::text`], `print` or `dump` back into the source's bytes
    /// (see [`Source::restore`]).
    pub fn restore_bytes<'t>(&self, text: &'t str) -> alloc::borrow::Cow<'t, [u8]> {
        self.source.restore(text)
    }

    /// The top-level list: the script's commands in order.
    pub fn root(&self) -> ListId {
        self.root
    }

    pub fn list(&self, id: ListId) -> &List {
        &self.lists[id.index()]
    }

    pub fn cmd(&self, id: CmdId) -> &Command {
        &self.cmds[id.index()]
    }

    pub fn heredoc(&self, id: HereDocId) -> &HereDoc {
        &self.heredocs[id.index()]
    }

    /// The text of a span (its UTF-8 view; see [`Source`]).
    pub fn text(&self, span: Span) -> &str {
        self.source.text(span)
    }

    /// Number of commands in the arena (all of them, including nested ones).
    pub fn cmd_count(&self) -> usize {
        self.cmds.len()
    }

    /// Every command in the arena, nested ones included, in the order they were parsed
    /// (children before their parents).
    pub fn cmd_ids(&self) -> impl Iterator<Item = CmdId> + '_ {
        (0..self.cmds.len() as u32).map(CmdId)
    }

    /// Number of lists in the arena.
    pub fn list_count(&self) -> usize {
        self.lists.len()
    }
}

impl Index<ListId> for Ast {
    type Output = List;
    fn index(&self, id: ListId) -> &List {
        self.list(id)
    }
}

impl Index<CmdId> for Ast {
    type Output = Command;
    fn index(&self, id: CmdId) -> &Command {
        self.cmd(id)
    }
}

impl Index<HereDocId> for Ast {
    type Output = HereDoc;
    fn index(&self, id: HereDocId) -> &HereDoc {
        self.heredoc(id)
    }
}

/// A sequence of and-or lists separated by `;`, `&` or newlines: a script, the body of
/// a compound command, or the inside of `$(…)`.
#[derive(Clone, Debug)]
pub struct List {
    pub span: Span,
    pub items: Vec<AndOr>,
}

/// Pipelines joined by `&&` and `||`, optionally run in the background with `&`.
#[derive(Clone, Debug)]
pub struct AndOr {
    pub span: Span,
    pub first: Pipeline,
    pub rest: Vec<(AndOrOp, Pipeline)>,
    /// Terminated by `&`.
    pub background: bool,
    /// Ended by a newline rather than `;`, `&` or the end of the list. Bash's printer
    /// keeps this distinction inside `$(…)`.
    pub newline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndOrOp {
    /// `&&`: run the next pipeline if this one succeeded.
    And,
    /// `||`: run the next pipeline if this one failed.
    Or,
}

/// Commands connected by `|` or `|&`, optionally timed or negated.
#[derive(Clone, Debug)]
pub struct Pipeline {
    pub span: Span,
    /// `time` or `time -p` in front of the pipeline.
    pub time: Option<Time>,
    /// The number of `!` in front of the pipeline (see [`Pipeline::negated`]).
    pub bangs: u32,
    /// The commands. Empty only for a bare `time` or `!`.
    pub stages: Vec<Stage>,
}

impl Pipeline {
    /// The exit status is inverted: an odd number of `!`.
    pub fn negated(&self) -> bool {
        self.bangs % 2 == 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    pub span: Span,
    /// `time -p`: POSIX output format.
    pub posix: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stage {
    pub cmd: CmdId,
    /// Followed by `|&`: this command's stderr goes into the pipe too.
    pub pipe_stderr: bool,
}

/// A command with its redirections.
#[derive(Clone, Debug)]
pub struct Command {
    /// Covers the command and its redirections (and the here-doc operators, not their
    /// bodies, which come after the line).
    pub span: Span,
    pub kind: CommandKind,
    /// In source order. For a simple command these are all its redirections, wherever
    /// they appeared among the words.
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug)]
pub enum CommandKind {
    Simple(SimpleCommand),
    /// `{ list; }`
    Brace(ListId),
    /// `( list )`
    Subshell(ListId),
    If(IfCommand),
    /// `while` or `until`.
    While(WhileCommand),
    /// `for name [in words]; do …; done`.
    For(ForCommand),
    /// `select name [in words]; do …; done`; same shape as `for`.
    Select(ForCommand),
    /// `for (( init; cond; step )); do …; done`.
    ArithFor(ArithForCommand),
    Case(CaseCommand),
    /// `(( expr ))`: the expression text (see [`Part::Arith`]).
    Arith(Word),
    /// `[[ expr ]]`.
    Cond(CondExpr),
    /// `name() body` or `function name body`.
    Function(FunctionDef),
    /// `coproc [NAME] command`.
    Coproc(Coproc),
}

/// Assignments, words and (in [`Command::redirects`]) redirections.
#[derive(Clone, Debug)]
pub struct SimpleCommand {
    /// Assignments before the command name (`A=1 B=2 cmd`), or the whole command.
    pub assigns: Vec<Assign>,
    /// The command name and its arguments. Empty for an assignment- or redirection-only
    /// command.
    pub words: Vec<Word>,
}

#[derive(Clone, Debug)]
pub struct IfCommand {
    /// `if` and every `elif`, in order.
    pub branches: Vec<IfBranch>,
    pub else_body: Option<ListId>,
}

#[derive(Clone, Debug)]
pub struct IfBranch {
    pub span: Span,
    pub cond: ListId,
    pub body: ListId,
}

#[derive(Clone, Debug)]
pub struct WhileCommand {
    /// `until` rather than `while`.
    pub until: bool,
    pub cond: ListId,
    pub body: ListId,
}

#[derive(Clone, Debug)]
pub struct ForCommand {
    /// The loop variable as written (not validated as a name; the engine reports bad
    /// identifiers as bash does).
    pub var: Word,
    /// `in words`; `None` iterates over `"$@"`.
    pub words: Option<Vec<Word>>,
    pub body: ListId,
}

#[derive(Clone, Debug)]
pub struct ArithForCommand {
    /// Each clause's text; an empty word for an omitted clause.
    pub init: Word,
    pub cond: Word,
    pub step: Word,
    pub body: ListId,
}

#[derive(Clone, Debug)]
pub struct CaseCommand {
    pub word: Word,
    pub items: Vec<CaseItem>,
}

#[derive(Clone, Debug)]
pub struct CaseItem {
    pub span: Span,
    pub patterns: Vec<Word>,
    /// `None` for an empty body (`a) ;;`).
    pub body: Option<ListId>,
    pub terminator: CaseTerminator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseTerminator {
    /// `;;`, or none before `esac`: stop.
    Break,
    /// `;&`: run the next item's body without testing it.
    FallThrough,
    /// `;;&`: test the next items too.
    Continue,
}

#[derive(Clone, Debug)]
pub struct FunctionDef {
    /// The name as written. Not expanded; the engine checks it.
    pub name: Word,
    /// Defined with the `function` keyword.
    pub keyword: bool,
    /// The body: always a compound command. Redirections written after the body belong
    /// to it and apply each time the function runs.
    pub body: CmdId,
}

#[derive(Clone, Debug)]
pub struct Coproc {
    /// `coproc NAME compound`; `None` means the default name `COPROC`.
    pub name: Option<Word>,
    pub body: CmdId,
}

/// `name=value`, `name+=value`, `name[sub]=value`, `name=(…)`.
#[derive(Clone, Debug)]
pub struct Assign {
    pub span: Span,
    pub name: Span,
    /// The subscript text of `name[sub]=`, without the brackets.
    pub index: Option<Word>,
    /// `+=`.
    pub append: bool,
    pub value: AssignValue,
}

#[derive(Clone, Debug)]
pub enum AssignValue {
    /// May be an empty word (`a=`).
    Scalar(Word),
    Array(ArrayLit),
}

/// `( elem … )` of a compound assignment.
#[derive(Clone, Debug)]
pub struct ArrayLit {
    pub span: Span,
    pub elems: Vec<ArrayElem>,
}

#[derive(Clone, Debug)]
pub struct ArrayElem {
    pub span: Span,
    /// `[key]=value`: the key text without the brackets.
    pub key: Option<Word>,
    /// `[key]+=value`.
    pub append: bool,
    pub value: Word,
}

/// A redirection: `[fd]op target`.
#[derive(Clone, Debug)]
pub struct Redirect {
    pub span: Span,
    pub fd: Option<RedirFd>,
    pub op: RedirOp,
    pub target: RedirTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirFd {
    /// `2>`
    Number(u32),
    /// `{name}>`: the span of `name`.
    Var(Span),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirOp {
    /// `<`
    Read,
    /// `>`
    Write,
    /// `>>`
    Append,
    /// `>|`
    Clobber,
    /// `<>`
    ReadWrite,
    /// `<&`
    DupRead,
    /// `>&` (also `>& file`, which the engine treats like `&>`)
    DupWrite,
    /// `&>`
    WriteBoth,
    /// `&>>`
    AppendBoth,
    /// `<<<`
    HereString,
    /// `<<`
    HereDoc,
    /// `<<-`
    HereDocStrip,
}

impl RedirOp {
    pub fn as_str(self) -> &'static str {
        match self {
            RedirOp::Read => "<",
            RedirOp::Write => ">",
            RedirOp::Append => ">>",
            RedirOp::Clobber => ">|",
            RedirOp::ReadWrite => "<>",
            RedirOp::DupRead => "<&",
            RedirOp::DupWrite => ">&",
            RedirOp::WriteBoth => "&>",
            RedirOp::AppendBoth => "&>>",
            RedirOp::HereString => "<<<",
            RedirOp::HereDoc => "<<",
            RedirOp::HereDocStrip => "<<-",
        }
    }
}

#[derive(Clone, Debug)]
pub enum RedirTarget {
    Word(Word),
    HereDoc(HereDocId),
}

/// A here-document: its delimiter as written and its body.
#[derive(Clone, Debug)]
pub struct HereDoc {
    /// The delimiter word as written after `<<` (with its quotes).
    pub delimiter: Word,
    /// Any part of the delimiter was quoted: the body is literal (one [`Part::Lit`] per
    /// line). Otherwise the body has expansions, like a double-quoted string in which `"`
    /// is an ordinary character.
    pub quoted: bool,
    /// `<<-`: leading tabs were stripped from the body lines and the delimiter line; the
    /// stripped tabs are not covered by any part.
    pub strip_tabs: bool,
    /// The body: `span` covers the body lines, not the delimiter line.
    pub body: Word,
    /// False when the input ended before the delimiter line (bash warns and uses what it
    /// read).
    pub terminated: bool,
    /// The body of an unquoted here-doc did not parse (an unclosed `$(`, say). Bash only
    /// reports this when the here-doc is expanded; the body is then one literal part.
    pub error: Option<crate::ParseError>,
}

/// A `[[ … ]]` expression.
#[derive(Clone, Debug)]
pub struct CondExpr {
    pub span: Span,
    pub kind: CondKind,
}

#[derive(Clone, Debug)]
pub enum CondKind {
    /// `-f word`
    Unary {
        op: UnaryTest,
        op_span: Span,
        arg: Word,
    },
    /// `word op word`. The right side of `==`, `!=` (and `=`) is a pattern; of `=~` a
    /// regular expression.
    Binary {
        op: BinaryTest,
        op_span: Span,
        lhs: Word,
        rhs: Word,
    },
    /// A lone word: true if non-empty.
    Word(Word),
    Not(Box<CondExpr>),
    And(Box<CondExpr>, Box<CondExpr>),
    Or(Box<CondExpr>, Box<CondExpr>),
    Group(Box<CondExpr>),
}

/// Unary tests of `[[ ]]` (and `test`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryTest {
    /// `-a`, `-e`
    FileExists,
    /// `-b`
    FileExistsAndIsBlockSpecialFile,
    /// `-c`
    FileExistsAndIsCharSpecialFile,
    /// `-d`
    FileExistsAndIsDir,
    /// `-f`
    FileExistsAndIsRegularFile,
    /// `-g`
    FileExistsAndIsSetgid,
    /// `-h`, `-L`
    FileExistsAndIsSymlink,
    /// `-k`
    FileExistsAndHasStickyBit,
    /// `-p`
    FileExistsAndIsFifo,
    /// `-r`
    FileExistsAndIsReadable,
    /// `-s`
    FileExistsAndIsNotZeroLength,
    /// `-t`
    FdIsOpenTerminal,
    /// `-u`
    FileExistsAndIsSetuid,
    /// `-w`
    FileExistsAndIsWritable,
    /// `-x`
    FileExistsAndIsExecutable,
    /// `-G`
    FileExistsAndOwnedByEffectiveGroupId,
    /// `-N`
    FileExistsAndModifiedSinceLastRead,
    /// `-O`
    FileExistsAndOwnedByEffectiveUserId,
    /// `-S`
    FileExistsAndIsSocket,
    /// `-o`
    ShellOptionEnabled,
    /// `-v`
    ShellVariableIsSetAndAssigned,
    /// `-R`
    ShellVariableIsSetAndNameRef,
    /// `-z`
    StringHasZeroLength,
    /// `-n`
    StringHasNonZeroLength,
}

impl UnaryTest {
    pub fn from_op(op: &str) -> Option<UnaryTest> {
        use UnaryTest::*;
        Some(match op {
            "-a" | "-e" => FileExists,
            "-b" => FileExistsAndIsBlockSpecialFile,
            "-c" => FileExistsAndIsCharSpecialFile,
            "-d" => FileExistsAndIsDir,
            "-f" => FileExistsAndIsRegularFile,
            "-g" => FileExistsAndIsSetgid,
            "-h" | "-L" => FileExistsAndIsSymlink,
            "-k" => FileExistsAndHasStickyBit,
            "-p" => FileExistsAndIsFifo,
            "-r" => FileExistsAndIsReadable,
            "-s" => FileExistsAndIsNotZeroLength,
            "-t" => FdIsOpenTerminal,
            "-u" => FileExistsAndIsSetuid,
            "-w" => FileExistsAndIsWritable,
            "-x" => FileExistsAndIsExecutable,
            "-G" => FileExistsAndOwnedByEffectiveGroupId,
            "-N" => FileExistsAndModifiedSinceLastRead,
            "-O" => FileExistsAndOwnedByEffectiveUserId,
            "-S" => FileExistsAndIsSocket,
            "-o" => ShellOptionEnabled,
            "-v" => ShellVariableIsSetAndAssigned,
            "-R" => ShellVariableIsSetAndNameRef,
            "-z" => StringHasZeroLength,
            "-n" => StringHasNonZeroLength,
            _ => return None,
        })
    }
}

/// Binary tests of `[[ ]]` (and `test`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryTest {
    /// `-ef`
    FilesReferToSameDeviceAndInodeNumbers,
    /// `-nt`
    LeftFileIsNewerOrExistsWhenRightDoesNot,
    /// `-ot`
    LeftFileIsOlderOrDoesNotExistWhenRightDoes,
    /// `==`, `=`
    StringExactlyMatchesPattern,
    /// `!=`
    StringDoesNotExactlyMatchPattern,
    /// `=~`
    StringMatchesRegex,
    /// `<`
    LeftSortsBeforeRight,
    /// `>`
    LeftSortsAfterRight,
    /// `-eq`
    ArithmeticEqualTo,
    /// `-ne`
    ArithmeticNotEqualTo,
    /// `-lt`
    ArithmeticLessThan,
    /// `-le`
    ArithmeticLessThanOrEqualTo,
    /// `-gt`
    ArithmeticGreaterThan,
    /// `-ge`
    ArithmeticGreaterThanOrEqualTo,
}

impl BinaryTest {
    pub fn from_op(op: &str) -> Option<BinaryTest> {
        use BinaryTest::*;
        Some(match op {
            "-ef" => FilesReferToSameDeviceAndInodeNumbers,
            "-nt" => LeftFileIsNewerOrExistsWhenRightDoesNot,
            "-ot" => LeftFileIsOlderOrDoesNotExistWhenRightDoes,
            "==" | "=" => StringExactlyMatchesPattern,
            "!=" => StringDoesNotExactlyMatchPattern,
            "=~" => StringMatchesRegex,
            "<" => LeftSortsBeforeRight,
            ">" => LeftSortsAfterRight,
            "-eq" => ArithmeticEqualTo,
            "-ne" => ArithmeticNotEqualTo,
            "-lt" => ArithmeticLessThan,
            "-le" => ArithmeticLessThanOrEqualTo,
            "-gt" => ArithmeticGreaterThan,
            "-ge" => ArithmeticGreaterThanOrEqualTo,
            _ => return None,
        })
    }
}

/// A word: a sequence of parts with no unquoted blanks between them.
#[derive(Clone, Debug)]
pub struct Word {
    pub span: Span,
    pub parts: Vec<WordPart>,
}

#[derive(Clone, Debug)]
pub struct WordPart {
    pub span: Span,
    pub kind: Part,
}

/// The parts of a word. The text of the literal kinds is fixed by the span:
#[derive(Clone, Debug)]
pub enum Part {
    /// Verbatim text: the span's text, as is. Unquoted literal text never contains a
    /// backslash (those become [`Part::Escaped`]), but may contain glob characters and a
    /// leading `~` (tilde expansion is left to the engine, which knows the context).
    Lit,
    /// `\c`: the char after the backslash, taken literally.
    Escaped,
    /// `'…'`: the text between the quotes, literally.
    Single,
    /// `$'…'`: the text between the quotes, with C escapes still to decode.
    AnsiC,
    /// `"…"`
    Double(Vec<WordPart>),
    /// `$"…"` (translated string; psh treats it like `"…"`).
    Locale(Vec<WordPart>),
    /// `$name`, `${…}`
    Param(Box<Param>),
    /// A `${…}` that bash accepts at parse time but rejects when expanded ("bad
    /// substitution").
    BadSubst,
    /// `$( list )`
    CmdSubst(ListId),
    /// `${ list; }` (`reply` false) or `${| list; }` (`reply` true): bash 5.3's command
    /// substitution that runs in the current shell; with `|`, the value is `$REPLY`.
    FunSub { list: ListId, reply: bool },
    /// `` `…` ``: bash parses the body only when it runs, so the AST keeps the text; see
    /// [`crate::backtick_body`].
    Backtick,
    /// `$(( expr ))` or `$[ expr ]`: the expression text, as parts with expansions.
    /// Arithmetic is parsed after expansion, so it is not an expression tree here.
    Arith(Box<Word>),
    /// `<( list )` or `>( list )`.
    ProcSubst { output: bool, list: ListId },
    /// `{a,b}` or `{1..9}` (only in words that undergo brace expansion).
    Brace(Box<Brace>),
    /// `?(…)`, `*(…)`, `+(…)`, `@(…)`, `!(…)`: the operator char and the alternatives.
    ExtGlob { op: u8, alts: Vec<Word> },
    /// `( … )` of `declare name=( … )` and the other declaration builtins.
    Array(ArrayLit),
}

#[derive(Clone, Debug)]
pub enum Brace {
    /// `{a,b,c}`: two or more alternatives.
    Alts(Vec<Word>),
    /// `{x..y[..step]}`: the endpoint texts (integers or single chars) and the step.
    Seq {
        start: Span,
        end: Span,
        step: Option<Span>,
    },
}

/// A parameter expansion.
#[derive(Clone, Debug)]
pub struct Param {
    /// `${…}` rather than `$name`.
    pub braced: bool,
    /// `${#…}` or `${!…}`.
    pub prefix: ParamPrefix,
    pub name: ParamName,
    pub name_span: Span,
    /// `[…]` after the name.
    pub index: Option<Subscript>,
    pub op: ParamOp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamPrefix {
    None,
    /// `${#name}`: length.
    Length,
    /// `${!name}`: indirection; with `[@]`/`[*]`, the keys of an array.
    Indirect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamName {
    /// A variable name.
    Var,
    /// `$1`, `${10}`: the digits of the span.
    Positional,
    /// `$@ $* $# $? $- $$ $! $0`: the char.
    Special(u8),
}

#[derive(Clone, Debug)]
pub enum Subscript {
    /// `[@]`
    At,
    /// `[*]`
    Star,
    /// `[expr]`: the text between the brackets.
    Expr(Word),
}

#[derive(Clone, Debug)]
pub enum ParamOp {
    None,
    /// `-`, `=`, `?`, `+`, optionally with `:`.
    Default {
        kind: DefaultKind,
        colon: bool,
        word: Word,
    },
    /// `#pattern`, `##pattern`.
    RemovePrefix {
        longest: bool,
        pattern: Word,
    },
    /// `%pattern`, `%%pattern`.
    RemoveSuffix {
        longest: bool,
        pattern: Word,
    },
    /// `/pat/rep`, `//pat/rep`, `/#pat/rep`, `/%pat/rep`.
    Replace {
        kind: ReplaceKind,
        pattern: Word,
        replacement: Option<Word>,
    },
    /// `^`, `^^`, `,`, `,,`, `~`, `~~`, with an optional pattern.
    Case {
        kind: CaseKind,
        pattern: Option<Word>,
    },
    /// `:offset[:length]`: arithmetic texts.
    Substring {
        offset: Word,
        length: Option<Word>,
    },
    /// `@op`: the operator char (`Q`, `E`, `P`, `A`, `K`, `a`, `k`, `u`, `U`, `L`).
    Transform(u8),
    /// `${!prefix*}` or `${!prefix@}`: names of variables with that prefix.
    Names {
        star: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultKind {
    /// `-`: use a default value.
    Use,
    /// `=`: assign a default value.
    Assign,
    /// `?`: fail with a message.
    Error,
    /// `+`: use an alternative value.
    Alternative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceKind {
    /// `/`
    First,
    /// `//`
    All,
    /// `/#`
    Prefix,
    /// `/%`
    Suffix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseKind {
    /// `^`
    UpperFirst,
    /// `^^`
    UpperAll,
    /// `,`
    LowerFirst,
    /// `,,`
    LowerAll,
    /// `~`
    ToggleFirst,
    /// `~~`
    ToggleAll,
}
