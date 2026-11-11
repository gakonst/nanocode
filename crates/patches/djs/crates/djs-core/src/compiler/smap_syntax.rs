//! The code around a function's suspension sites (D2850): what the plain-dump exporter
//! needs to write "the rest of the function from a site" as source text (a continuation,
//! D2830, D2831) without parsing the function again.
//!
//! The compiler is single-pass and builds no syntax tree. While recording suspension
//! maps it keeps every syntax node that *encloses a site* of the function (statements
//! and expressions of the function's own code, not of nested functions), with the spans
//! of its parts, and for each site the path of those nodes from the function's body down
//! to the site. Everything else stays text: the exporter copies spans of the function's
//! source verbatim (`FunctionMap::start` + offset).
//!
//! Conventions:
//!
//! - Every offset is from the function's start (`FunctionMap::start`), like `Site::pos`
//!   and the offsets in slot names (`call.callee@41`); spans are half-open `[start,
//!   end)`. Offsets are into the compiled source, TypeScript annotations included.
//! - A node's `parts` are its direct parts in source order, each with its role. A part
//!   that is absent from the source (no `else`, an empty `for` test) is not listed.
//!   Parts never overlap; the text between them is punctuation and keywords (`if (`,
//!   `) `, `;`, `=>`), which the exporter writes itself.
//! - A part's span is the span of the construct in it, parentheses around it included:
//!   in `f((await x))` the argument part is `(await x)`.
//! - Where a slot name refers to a construct, the offset is the node's `start` or a
//!   part's `start` (see each [`NodeKind`]); so a temporary `c@o` of a site belongs to the
//!   node on the site's path whose start or part start is `o`.
//!
//! Only these constructs are nodes; class bodies are not (a site there is unsupported
//! anyway), and TypeScript expressions (`x as T`, `x!`) are transparent (see
//! `Syntax::erased`). A path that passes through something else has a **gap**: the part that should hold the next node holds more than that node and
//! parentheses. `Site::gap` reports it, and the exporter must refuse such a site rather
//! than drop the construct.

#![cfg_attr(not(feature = "dump"), allow(dead_code))]

use alloc::string::String;
use alloc::vec::Vec;

use super::func::VarId;

#[cfg(feature = "dump")]
pub(in crate::compiler) use recording::{SynRec, build_syntax};

/// The syntax recorded for one function (`FunctionMap::syntax`).
#[derive(Clone, Debug, Default)]
pub struct Syntax {
    /// The nodes that enclose a site, in no particular order: sites refer to them by
    /// index (`Step::node`).
    pub nodes: Vec<Node>,
    /// References in the function's own code, and in the arrow functions nested in it
    /// (which share them), to `arguments`, `new.target` and `super`: a continuation is
    /// a function of its own, so its `arguments` is not the original's (the exporter
    /// renames these to a binding of the saved value), and it has no `new.target` or
    /// home object. Source order.
    pub special: Vec<(Special, u32)>,
    /// The TypeScript-only ranges of the function's own code (annotations, `as T`,
    /// `satisfies T`, `x!`, `<T>x`, type arguments, type-only statements), in source
    /// order, D2853. They are transparent: no node stands for a TypeScript expression,
    /// and the gap check counts these ranges as blanks, so `(await x) as T` has a
    /// complete path. Splice from `compiler::strip_types`'s text, which blanks exactly
    /// these ranges and keeps every offset (and nested functions' types too).
    pub erased: Vec<(u32, u32)>,
}

#[cfg(feature = "dump")]
impl Syntax {
    /// The nodes and special references as text, for the pin test and for reading:
    /// one line per node (`index kind @start..end`, its parts as `Role start..end`,
    /// its declarations as `kind name@scope#part`).
    pub fn text(&self) -> String {
        use core::fmt::Write;
        let mut out = String::new();
        for (i, n) in self.nodes.iter().enumerate() {
            let _ = write!(out, "  node {i} {} @{}..{}:", n.kind.name(), n.start, n.end);
            for p in &n.parts {
                let _ = write!(out, " {:?} {}..{}", p.role, p.start, p.end);
            }
            if !n.decls.is_empty() {
                out.push_str(" |");
                for d in &n.decls {
                    let _ = write!(out, " {:?} {}@{}", d.kind, d.name, d.scope);
                    match d.part {
                        Some(p) => {
                            let _ = write!(out, "#{p}");
                        }
                        None => out.push_str("#-"),
                    }
                }
            }
            out.push('\n');
        }
        if !self.erased.is_empty() {
            out.push_str("  erased");
            for (s, e) in &self.erased {
                let _ = write!(out, " {s}..{e}");
            }
            out.push('\n');
        }
        if !self.special.is_empty() {
            out.push_str("  special");
            for (s, at) in &self.special {
                let _ = write!(out, " {s:?}@{at}");
            }
            out.push('\n');
        }
        out
    }
}

impl NodeKind {
    /// A short name: the variant's, with `?` for optional and `[]` for computed.
    pub fn name(self) -> String {
        let mut s = String::from(match self {
            NodeKind::Body => "Body",
            NodeKind::Block => "Block",
            NodeKind::Expr => "Expr",
            NodeKind::Decl(_) => "Decl",
            NodeKind::If => "If",
            NodeKind::Label => "Label",
            NodeKind::While => "While",
            NodeKind::DoWhile => "DoWhile",
            NodeKind::For => "For",
            NodeKind::ForIn => "ForIn",
            NodeKind::ForOf => "ForOf",
            NodeKind::ForAwait => "ForAwait",
            NodeKind::Try => "Try",
            NodeKind::Switch => "Switch",
            NodeKind::Return => "Return",
            NodeKind::Throw => "Throw",
            NodeKind::Await => "Await",
            NodeKind::Yield => "Yield",
            NodeKind::YieldStar => "YieldStar",
            NodeKind::Call { .. } => "Call",
            NodeKind::New => "New",
            NodeKind::ImportCall => "ImportCall",
            NodeKind::Member { .. } => "Member",
            NodeKind::Binary => "Binary",
            NodeKind::Logical => "Logical",
            NodeKind::Assign => "Assign",
            NodeKind::Unary => "Unary",
            NodeKind::Update => "Update",
            NodeKind::Conditional => "Conditional",
            NodeKind::Sequence => "Sequence",
            NodeKind::Template => "Template",
            NodeKind::TaggedTemplate => "TaggedTemplate",
            NodeKind::Array => "Array",
            NodeKind::Object => "Object",
            NodeKind::ArrayPattern => "ArrayPattern",
            NodeKind::ObjectPattern => "ObjectPattern",
            NodeKind::Default => "Default",
        });
        match self {
            NodeKind::Decl(k) => {
                let _ = core::fmt::Write::write_fmt(&mut s, format_args!("({k:?})"));
            }
            NodeKind::Call { optional: true } => s.push('?'),
            NodeKind::Member { computed, optional } => {
                if optional {
                    s.push('?');
                }
                if computed {
                    s.push_str("[]");
                }
            }
            _ => {}
        }
        s
    }
}

/// What a [`Syntax::special`] reference is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    /// The identifier `arguments`, resolving to this function's arguments object
    /// (offset of the identifier).
    Arguments,
    /// `new.target` (offset of `new`).
    NewTarget,
    /// `super` in `super.x`, `super[x]` or `super(...)` (offset of `super`).
    Super,
}

/// One step of a site's path (`Site::path`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Step {
    /// Index into `Syntax::nodes`.
    pub node: u32,
    /// The index (into the node's `parts`) of the part that holds the next step's node;
    /// `None` for the last step (the site's own construct, see `Site::path`).
    pub part: Option<u16>,
}

/// A syntax node that encloses at least one site.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub start: u32,
    pub end: u32,
    pub parts: Vec<Part>,
    /// The bindings of the scopes this node opens, in declaration order. Where each
    /// kind of binding is listed:
    ///
    /// - `Body`: the parameters, the `var`s (wherever they are in the function), the
    ///   body-level declarations, a named function expression's own name;
    /// - `Block` (including the blocks of `try`, `catch` and `finally`) and `Switch`
    ///   (all case clauses share one scope): their lexical declarations;
    /// - `For`: the `let`/`const` of its init (`var`s are the `Body`'s);
    /// - `ForIn`/`ForOf`/`ForAwait`: the `let`/`const`/`using` names of the head, so
    ///   the head's kind is their `kind`; empty decls mean the head is an assignment
    ///   target (or a `var`, whose names are the `Body`'s);
    /// - `Try`: the `catch` parameter's names (`CatchParam` for a name, `Let` for the
    ///   names of a pattern), with `part` the `CatchParam` part; the catch block's own
    ///   declarations are its `Block`'s.
    ///
    /// Empty for nodes that open no scope.
    pub decls: Vec<Decl>,
}

/// A part of a node: its role and its span.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Part {
    pub role: Role,
    pub start: u32,
    pub end: u32,
}

/// A binding a node's scope declares.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    /// The offset that names the declaring scope in suspension maps (`Slot::Binding`'s
    /// `scope`, the `S` of a shadowed binding's key `name@S`).
    pub scope: u32,
    /// An offset inside the code that declares it: in JavaScript inside the
    /// declarator's `Target` (the name's last character, or within the pattern), so a
    /// declaration statement's names can be told apart by declarator; inside the
    /// parameter list for a parameter, the `catch` parameter for one, the function
    /// for a function declaration.
    pub at: u32,
    /// The part of the node whose code declares it (a statement of a statement list,
    /// a `for` head, the parameter list, a `catch` parameter); `None` for a named
    /// function expression's own name. A binding declared by a statement before the
    /// site's statement has run its declaration (its value is in the frame, a TDZ value
    /// if it is still uninitialized); one declared after it has not.
    pub part: Option<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeclKind {
    Var,
    Let,
    Const,
    /// `using` and `await using` declarations.
    Using,
    AwaitUsing,
    /// A class declaration's name.
    Class,
    /// A function declaration's name (initialized at its scope's entry).
    Function,
    Param,
    CatchParam,
    /// A named function expression's own name.
    FuncName,
}

/// The role of a part in its node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    // ---- statements
    /// A function's parameter list, with its parentheses when it has them.
    Params,
    /// A statement of a statement list (function body, block, case clause).
    Stmt,
    /// A function declaration in a statement list: hoisted, it runs nothing where it
    /// stands.
    FuncDecl,
    /// A loop's or labelled statement's body.
    Body,
    /// The test of `if`, `while`, `do`, `for` and `?:`.
    Test,
    /// The branches of `if` and `?:`.
    Then,
    Else,
    /// A `for (init; …)`'s init; a declarator's initializer (after the `=`).
    Init,
    /// A `for (…; …; update)`'s update.
    Update,
    /// A `for-in`/`for-of`'s left side (`const [k, v]`, `x.y`).
    Head,
    /// A `for-in`/`for-of`'s right side.
    Iterable,
    /// A label's identifier.
    Label,
    /// A `try`'s block, `catch` parameter (without its parentheses), `catch` block and
    /// `finally` block.
    Block,
    CatchParam,
    CatchBody,
    Finally,
    /// A `switch`'s discriminant; a `case`'s test (followed by the clause's statements
    /// as `Stmt` parts); a `default` (the keyword's span, followed by its statements).
    Discriminant,
    Case,
    Default,
    // ---- expressions
    /// The operand of `await`, `yield`, `yield*`, a unary or update operator, `return`
    /// and `throw`; the expression of an expression statement; the concise body of an
    /// arrow function.
    Operand,
    /// An operator token (binary, logical, assignment, unary, update).
    Op,
    /// The operands of a binary, logical or assignment operator. An assignment to a
    /// name or a pattern has its target as `Left`; one to a member has `Object` and
    /// `Property` instead (see `Assign`).
    Left,
    Right,
    /// A declarator's target (a name or a pattern).
    Target,
    /// A call's or `new`'s callee (`super` for `super(...)`), the `(` of its arguments
    /// (an optional call's `?.` is between the callee and the `(`), and each argument
    /// (`...x` is `Spread`). An argument part starts where `call.arg@`/`new.arg@` name it; the
    /// argument array of a call with spread is named by the `(`'s start.
    Callee,
    Open,
    Arg,
    Spread,
    /// A member expression's object (`super` for `super.x`) and its property: the
    /// bare name of `.x` (or `?.x`; the exporter writes the `.`/`?.`), or for a
    /// computed member the whole `[key]`, brackets included. Also the parts of a member
    /// target of `Assign` and `Update`.
    Object,
    Property,
    /// An array literal's element (`...x` is `Spread`), or a hole (an empty span where
    /// the hole is).
    Element,
    Hole,
    /// A template literal's text part as its token: the backtick or `}` before it to the
    /// `${` or backtick after it (`template.part@` names it by its start), and a
    /// substitution's expression.
    Quasi,
    Subst,
    /// A tagged template's tag.
    Tag,
    /// A rest element or property of a pattern, `...` included.
    Rest,
    /// An object literal's property: a plain key (identifier, string, number), a
    /// computed key with its brackets (`object.key@` names it by the `[`), the value
    /// after the `:`, a shorthand property `x`, a method, getter or setter as a whole,
    /// and a spread `...x` (`Spread`).
    Key,
    ComputedKey,
    Value,
    Shorthand,
    Method,
    /// An expression of a comma sequence.
    Expr,
}

/// What a node is, and what parts it has (in this order; `?` optional, `*` repeated).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    // ---- statements
    /// The function itself: its span is the whole function `[0, end - start)`. Parts:
    /// `Params?` then the body's statements (`Stmt`/`FuncDecl`*), or for an arrow with
    /// a concise body `Params? Operand`. The codemode body and Scripts have no
    /// `Params`. Always `path[0]`.
    Body,
    /// `{ … }`: `Stmt`/`FuncDecl`*. Also the blocks of `try`, `catch`, `finally` when a
    /// path enters them.
    Block,
    /// An expression statement: `Operand`.
    Expr,
    /// A `var`/`let`/`const`/`using`/`await using` declaration (a statement, or the
    /// declaration in a `for` init): `(Target Init?)*`.
    Decl(DeclKind),
    /// `if (Test) Then else Else?`.
    If,
    /// `Label: Body`.
    Label,
    /// `while (Test) Body`, `do Body while (Test)`.
    While,
    DoWhile,
    /// `for (Init?; Test?; Update?) Body`.
    For,
    /// `for (Head in Iterable) Body`, `for (Head of Iterable) Body`,
    /// `for await (Head of Iterable) Body`. Their temporaries (`for-of.iterator@…`) are
    /// named by the node's start.
    ForIn,
    ForOf,
    ForAwait,
    /// `try Block catch (CatchParam?) CatchBody? finally Finally?`. Its temporaries
    /// (`completion.kind@…`) are named by the node's start.
    Try,
    /// `switch (Discriminant) { (Case|Default Stmt*)* }`.
    Switch,
    /// `return Operand?`, `throw Operand`.
    Return,
    Throw,
    // ---- expressions
    /// `await Operand`, `yield Operand?`, `yield* Operand`.
    Await,
    Yield,
    YieldStar,
    /// `Callee Open Arg*` (args `Arg` or `Spread`); `call.callee@`/`call.this@` name it
    /// by its start. `optional`: this call is `?.(`.
    Call {
        optional: bool,
    },
    /// `new Callee Open? Arg*`; `new.callee@` names it by its start.
    New,
    /// `import(Arg Arg?)`.
    ImportCall,
    /// `Object Property`; `member.object@` names it by its start. `optional`: this
    /// member is `?.`.
    Member {
        computed: bool,
        optional: bool,
    },
    /// `Left Op Right`: binary operators (`binary.left@` by its start) and
    /// `&&`/`||`/`??`.
    Binary,
    Logical,
    /// An assignment (`assign.*@` by its start): `Left Op Right` for a name or pattern
    /// target (a pattern is not a node: a site in it is a gap), `Object Property Op
    /// Right` for a member target (`Object` is `super` for `super.x = …`; `Property`
    /// as in `Member`).
    Assign,
    /// `Op Operand` (`typeof`, `!`, `-`, `delete`, …).
    Unary,
    /// `++`/`--`: `Op Operand` or `Operand Op` for a name target, `Op Object Property`
    /// or `Object Property Op` for a member target.
    Update,
    /// `Test ? Then : Else`.
    Conditional,
    /// `Expr (, Expr)*`.
    Sequence,
    /// `` `…${Subst}…` ``: `Quasi (Subst Quasi)*`.
    Template,
    /// `Tag Quasi (Subst Quasi)*`; its arguments are named like a call's.
    TaggedTemplate,
    /// `[ Element|Hole|Spread * ]`; `array.literal@` names it by its start (the `[`).
    Array,
    /// `{ (Key Value | ComputedKey Value | Shorthand | Method | Spread)* }`;
    /// `object.literal@` names it by its start (the `{`).
    Object,
    // ---- patterns (D2852)
    /// A destructuring pattern, wherever one stands: a declarator's `Target`, an
    /// assignment's `Left`, a `for-in`/`for-of` `Head`, a parameter in `Params`, a
    /// `CatchParam`, or nested in another pattern. djs evaluates a pattern after its
    /// value, but its nodes nest as in the source. Bindings stay on the enclosing
    /// node's `decls`. `[ (Element|Hole)* Rest? ]` (an `Element` part is the target
    /// with its default, a `Hole` an empty span where the hole is). The iterator
    /// record is `pattern.iterator@`/`pattern.next@` by the node's start; the iterator
    /// is `undefined` once done (there is no separate done flag).
    ArrayPattern,
    /// `{ (Key Value | ComputedKey Value | Shorthand)* Rest? }`: a `Value` or
    /// `Shorthand` part is the target with its default (`a: b = 1`, `a = 1`);
    /// `ComputedKey` includes the brackets, and its converted key is `pattern.key@` by
    /// the `[` while the target and default run. The value being destructured is
    /// `pattern.source@` by the node's start, and with a rest property the keys taken
    /// so far are `pattern.excluded@` (an array).
    ObjectPattern,
    /// A pattern target with a default, `t = d`: `Target Init` (`Init` without the
    /// `=`); the node inside an `Element`, `Value` or `Shorthand` part when a path goes
    /// through the default or through a nested pattern target. In a `Shorthand` the
    /// `Target` is the name. A reference target of an assignment pattern
    /// (`[o.x = 1] = v`) keeps its slot names (`assign.object@…`) at the target's
    /// start, which is the node's.
    Default,
}

// ---- Recording ------------------------------------------------------------------------

#[cfg(feature = "dump")]
mod recording {

    use super::*;
    use crate::compiler::func::{BODY_SCOPE, FuncState, ScopeId};
    use crate::heap::Heap;

    /// A node being parsed.
    struct Open {
        kind: NodeKind,
        start: u32,
        /// Where its parts start in `SynRec::parts`.
        mark: u32,
        /// Its index in `SynRec::nodes`, once a site is known to be in it.
        index: Option<u32>,
        /// The scopes it opens.
        scopes: [Option<ScopeId>; 2],
    }

    /// A node that encloses a site, in source offsets (not yet relative to the function).
    struct Kept {
        node: Node,
        scopes: [Option<ScopeId>; 2],
    }

    /// The parser's record of the syntax around one function's sites (D2850). Only
    /// async functions and generators record (only they have sites), so a function's
    /// recording is on or off for its whole parse and opens and closes always pair.
    #[derive(Default)]
    pub(in crate::compiler) struct SynRec {
        /// The nodes being parsed, outermost first. The function body is implicit: below
        /// the first.
        open: Vec<Open>,
        /// The parts of the open nodes and of the body, each node's from its `mark` on
        /// (nodes nest, so a node's parts are contiguous once its children closed).
        parts: Vec<Part>,
        /// Index 0 is the body once any node is kept.
        nodes: Vec<Option<Kept>>,
        /// Per site (`Recorder::sites`), its path of node indices.
        pub paths: Vec<Vec<u32>>,
        /// Declarations: the binding, an offset inside the code that declares it, its kind.
        decls: Vec<(VarId, u32, DeclKind)>,
        special: Vec<(Special, u32)>,
        /// TypeScript-only ranges (`Syntax::erased`), in source offsets.
        erased: Vec<(u32, u32)>,
        /// The member node that closed last, if the last node to close was one: an
        /// assignment's or update's target (`Parser::syn_target`).
        last: Option<Last>,
    }

    struct Last {
        start: u32,
        end: u32,
        index: Option<u32>,
        parts: [Part; 2],
    }

    impl SynRec {
        /// A site: every open node (and the body) encloses it.
        pub(in crate::compiler) fn site(&mut self) {
            if self.nodes.is_empty() {
                self.nodes.push(None);
            }
            let mut path = Vec::with_capacity(self.open.len() + 1);
            path.push(0);
            for o in &mut self.open {
                let i = *o.index.get_or_insert_with(|| {
                    self.nodes.push(None);
                    (self.nodes.len() - 1) as u32
                });
                path.push(i);
            }
            self.paths.push(path);
        }
    }

    /// The first offset in `from..to` that cannot be between a part's start and the
    /// node inside it (or after the node), if any: blanks, comments, parentheses,
    /// TypeScript-only ranges and what `role` adds around its node can.
    fn filler(
        src: &[u8],
        erased: &[(u32, u32)],
        from: usize,
        to: usize,
        role: Role,
    ) -> Option<usize> {
        let mut i = from;
        while i < to {
            if let Some(&(_, e)) = erased
                .iter()
                .find(|r| r.0 as usize <= i && i < r.1 as usize)
            {
                i = e as usize;
                continue;
            }
            let c = src[i];
            match c {
                b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | b'(' | b')' => i += 1,
                b'[' | b']' if matches!(role, Role::Property | Role::ComputedKey) => i += 1,
                b'.' if matches!(role, Role::Spread | Role::Rest) => i += 1,
                // A declaration head's keyword (`const [a] of …`, `await using x`).
                b'a'..=b'z' if role == Role::Head => i += 1,
                b'/' if src.get(i + 1) == Some(&b'/') => {
                    while i < to && src[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if src.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    while i + 1 < to && !(src[i] == b'*' && src[i + 1] == b'/') {
                        i += 1;
                    }
                    i += 2;
                }
                // Non-ASCII blanks (NBSP, line separators, BOM, …) are rare enough to count
                // as text: the exporter refuses rather than misreads.
                _ => return Some(i),
            }
        }
        None
    }

    /// A site's path and gap (`Site::path`, `Site::gap`).
    pub(in crate::compiler) type SitePath = (Vec<Step>, Option<u32>);

    /// The syntax of function `f` and the paths of its emitted sites (`order`: the
    /// recorder's site indices in the map's order).
    pub(in crate::compiler) fn build_syntax(
        heap: &Heap,
        f: &FuncState,
        rec: &SynRec,
        src: &[u8],
        order: &[usize],
    ) -> (Syntax, Vec<SitePath>) {
        let base = f.src_start;
        let rel = |p: u32| p.saturating_sub(base);
        let mut nodes: Vec<Node> = Vec::with_capacity(rec.nodes.len());
        let mut scopes: Vec<[Option<ScopeId>; 2]> = Vec::with_capacity(rec.nodes.len());
        // Recorded index -> index in `nodes` (a member that became an assignment's
        // target left no node).
        let mut map: Vec<Option<u32>> = alloc::vec![None; rec.nodes.len()];
        for (i, k) in rec.nodes.iter().enumerate() {
            let (mut node, sc) = match k {
                Some(k) => (k.node.clone(), k.scopes),
                None if i == 0 => {
                    // The body: the parts left over.
                    let mut parts = rec.parts.clone();
                    crate::sort::sort_by(&mut parts, |a, b| a.start < b.start);
                    let node = Node {
                        kind: NodeKind::Body,
                        start: base,
                        end: f.src_end,
                        parts,
                        decls: Vec::new(),
                    };
                    (node, [Some(BODY_SCOPE), Some(f.var_scope)])
                }
                // (Unclosed: a parse error ends the compile before maps are built.)
                None => continue,
            };
            node.start = rel(node.start);
            node.end = rel(node.end);
            for p in &mut node.parts {
                p.start = rel(p.start);
                p.end = rel(p.end);
            }
            map[i] = Some(nodes.len() as u32);
            nodes.push(node);
            scopes.push(sc);
        }
        // Declarations: to the innermost node of their scope with a part around them.
        let mut seen: Vec<VarId> = Vec::new();
        for &(v, pos, kind) in &rec.decls {
            if seen.contains(&v) {
                continue;
            }
            seen.push(v);
            let var = &f.vars[v as usize];
            let pos = rel(pos);
            let mut best: Option<(usize, Option<u16>)> = None;
            for (n, node) in nodes.iter().enumerate() {
                let own =
                    scopes[n].contains(&Some(var.scope)) || (n == 0 && kind == DeclKind::FuncName);
                if !own {
                    continue;
                }
                let part = node
                    .parts
                    .iter()
                    .position(|p| p.start <= pos && pos < p.end.max(p.start + 1))
                    .map(|p| p as u16);
                let better = match best {
                    None => true,
                    Some((b, bp)) => {
                        (part.is_some() && bp.is_none())
                            || (part.is_some() == bp.is_some()
                                && node.end - node.start < nodes[b].end - nodes[b].start)
                    }
                };
                if better {
                    best = Some((n, part));
                }
            }
            if let Some((n, part)) = best {
                nodes[n].decls.push(Decl {
                    name: heap.atom_name(var.name),
                    kind,
                    scope: f.scopes[var.scope as usize].pos.saturating_sub(base),
                    at: pos,
                    part,
                });
            }
        }
        // Paths, with the part that holds each next step.
        let paths = order
            .iter()
            .map(|&k| {
                let Some(path) = rec.paths.get(k) else {
                    return (Vec::new(), None);
                };
                let path: Vec<u32> = path
                    .iter()
                    .filter_map(|&n| map.get(n as usize).copied().flatten())
                    .collect();
                let mut steps = Vec::with_capacity(path.len());
                let mut gap = None;
                for (j, &n) in path.iter().enumerate() {
                    let Some(&next) = path.get(j + 1) else {
                        steps.push(Step {
                            node: n,
                            part: None,
                        });
                        break;
                    };
                    let (node, inner) = (&nodes[n as usize], &nodes[next as usize]);
                    let part = node
                        .parts
                        .iter()
                        .position(|p| p.start <= inner.start && inner.end <= p.end);
                    if gap.is_none() {
                        gap = match part {
                            None => Some(inner.start),
                            Some(p) => {
                                let p = node.parts[p];
                                let at = |o: u32| (base + o) as usize;
                                let er = &rec.erased;
                                filler(src, er, at(p.start), at(inner.start), p.role)
                                    .or_else(|| filler(src, er, at(inner.end), at(p.end), p.role))
                                    .map(|i| rel(i as u32))
                            }
                        };
                    }
                    steps.push(Step {
                        node: n,
                        part: part.map(|p| p as u16),
                    });
                }
                (steps, gap)
            })
            .collect();
        let mut special: Vec<(Special, u32)> =
            rec.special.iter().map(|&(s, p)| (s, rel(p))).collect();
        crate::sort::sort_by(&mut special, |a, b| a.1 < b.1);
        let mut erased: Vec<(u32, u32)> =
            rec.erased.iter().map(|&(s, e)| (rel(s), rel(e))).collect();
        crate::sort::sort_by(&mut erased, |a, b| a.0 < b.0);
        (
            Syntax {
                nodes,
                special,
                erased,
            },
            paths,
        )
    }

    // ---- The parser's hooks ------------------------------------------------------------

    impl crate::compiler::parser::Parser<'_, '_> {
        /// The recording of the current function, if it records syntax.
        #[inline]
        fn syn(&mut self) -> Option<&mut SynRec> {
            if !(self.f.is_async || self.f.is_generator) {
                return None;
            }
            self.f.smap.as_mut().map(|r| &mut r.syn)
        }

        /// The current function records syntax.
        #[inline]
        pub(in crate::compiler) fn syn_on(&self) -> bool {
            (self.f.is_async || self.f.is_generator) && self.f.smap.is_some()
        }

        /// The number of sites so far: what [`Parser::syn_open_late`] takes for a left
        /// operand that starts here.
        #[inline]
        pub(in crate::compiler) fn syn_mark(&mut self) -> u32 {
            self.syn().map_or(0, |r| r.paths.len() as u32)
        }

        /// A node starts at `start`.
        #[inline]
        pub(in crate::compiler) fn syn_open(&mut self, kind: NodeKind, start: u32) {
            if let Some(r) = self.syn() {
                let mark = r.parts.len() as u32;
                r.open.push(Open {
                    kind,
                    start,
                    mark,
                    index: None,
                    scopes: [None, None],
                });
            }
        }

        /// A node whose first part, `role` from `start` to `left_end`, was parsed already
        /// (a binary operator's left operand, a call's callee): the sites since `mark`
        /// ([`Parser::syn_mark`] where the operand started) are in it. For `Assign` and
        /// `Update` the part is their target ([`Parser::syn_target`]).
        pub(in crate::compiler) fn syn_open_late(
            &mut self,
            kind: NodeKind,
            start: u32,
            mark: u32,
            role: Role,
            left_end: u32,
        ) {
            let Some(r) = self.syn() else { return };
            let depth = r.open.len() + 1;
            let mut index = None;
            if (mark as usize) < r.paths.len() {
                r.nodes.push(None);
                let i = (r.nodes.len() - 1) as u32;
                for p in &mut r.paths[mark as usize..] {
                    p.insert(depth.min(p.len()), i);
                }
                index = Some(i);
            }
            let pmark = r.parts.len() as u32;
            r.open.push(Open {
                kind,
                start,
                mark: pmark,
                index,
                scopes: [None, None],
            });
            if matches!(kind, NodeKind::Assign | NodeKind::Update) {
                self.syn_target(start, left_end, role);
            } else {
                self.syn_part(role, start, left_end);
            }
        }

        /// The target of an assignment or update, `start..end`: a member becomes the
        /// parts `Object Property` (its node leaves the paths: the target's parts take
        /// its place), anything else the part `role`.
        pub(in crate::compiler) fn syn_target(&mut self, start: u32, end: u32, role: Role) {
            let Some(r) = self.syn() else { return };
            // The member is the last node that closed, inside the target (a target is a
            // name, a member or a pattern; parentheses are no nodes).
            match r.last.take() {
                Some(m) if m.start >= start && m.end <= end => {
                    if let Some(i) = m.index {
                        for p in &mut r.paths {
                            p.retain(|&n| n != i);
                        }
                        r.nodes[i as usize] = None;
                    }
                    r.parts.extend(m.parts);
                }
                _ => r.parts.push(Part { role, start, end }),
            }
        }

        /// The innermost open node is a `kind` after all (`for (…` turns out a for-of).
        #[inline]
        pub(in crate::compiler) fn syn_kind(&mut self, kind: NodeKind) {
            if let Some(o) = self.syn().and_then(|r| r.open.last_mut()) {
                o.kind = kind;
            }
        }

        /// The innermost open node opens the current scope.
        #[inline]
        pub(in crate::compiler) fn syn_scope(&mut self) {
            let s = self.scope;
            if let Some(o) = self.syn().and_then(|r| r.open.last_mut()) {
                if o.scopes[0].is_none() {
                    o.scopes[0] = Some(s);
                } else {
                    o.scopes[1] = Some(s);
                }
            }
        }

        /// A part of the innermost open node (of the body when none is open).
        #[inline]
        pub(in crate::compiler) fn syn_part(&mut self, role: Role, start: u32, end: u32) {
            if let Some(r) = self.syn() {
                r.parts.push(Part { role, start, end });
            }
        }

        /// The innermost open node ends where the last token ended.
        #[inline]
        pub(in crate::compiler) fn syn_close(&mut self) {
            let end = self.prev_end;
            let Some(r) = self.syn() else { return };
            let Some(o) = r.open.pop() else {
                debug_assert!(false, "syntax node closed twice");
                return;
            };
            let mark = o.mark as usize;
            r.last = match o.kind {
                NodeKind::Member { .. } if r.parts.len() == mark + 2 => Some(Last {
                    start: o.start,
                    end,
                    index: o.index,
                    parts: [r.parts[mark], r.parts[mark + 1]],
                }),
                _ => None,
            };
            if let Some(i) = o.index {
                let mut parts = r.parts[mark..].to_vec();
                crate::sort::sort_by(&mut parts, |a, b| a.start < b.start);
                r.nodes[i as usize] = Some(Kept {
                    node: Node {
                        kind: o.kind,
                        start: o.start,
                        end,
                        parts,
                        decls: Vec::new(),
                    },
                    scopes: o.scopes,
                });
            }
            r.parts.truncate(mark);
        }

        /// Binding `v` is declared by the code that just ended (its last token is the
        /// declaring code's). Again for the same binding: it is a `kind` after all
        /// (`using` declares a `const`, `var f; function f() {}`).
        #[inline]
        pub(in crate::compiler) fn syn_decl(&mut self, v: VarId, kind: DeclKind) {
            let pos = self.prev_end.saturating_sub(1);
            if let Some(r) = self.syn() {
                match r.decls.iter_mut().find(|d| d.0 == v) {
                    Some(d) => d.2 = kind,
                    None => r.decls.push((v, pos, kind)),
                }
            }
        }

        /// A TypeScript-only range of the current function's code.
        #[inline]
        pub(in crate::compiler) fn syn_erased(&mut self, start: u32, end: u32) {
            if let Some(r) = self.syn() {
                r.erased.push((start, end));
            }
        }

        /// A reference to `arguments`, `new.target` or `super` at `pos`, recorded in the
        /// function that owns it (the innermost non-arrow one).
        pub(in crate::compiler) fn syn_special(&mut self, s: Special, pos: u32) {
            let rec = match self.own_this_function() {
                Ok(None) | Err(()) => self.f.smap.as_mut(),
                Ok(Some(i)) => self.stack[i].f.smap.as_mut(),
            };
            if let Some(r) = rec {
                r.syn.special.push((s, pos));
            }
        }
    }
}

#[cfg(not(feature = "dump"))]
impl super::parser::Parser<'_, '_> {
    #[inline(always)]
    pub(super) fn syn_mark(&mut self) -> u32 {
        0
    }
    #[inline(always)]
    pub(super) fn syn_on(&self) -> bool {
        false
    }
    #[inline(always)]
    pub(super) fn syn_open(&mut self, _kind: NodeKind, _start: u32) {}
    #[inline(always)]
    pub(super) fn syn_open_late(&mut self, _k: NodeKind, _s: u32, _m: u32, _r: Role, _e: u32) {}
    #[inline(always)]
    pub(super) fn syn_kind(&mut self, _kind: NodeKind) {}
    #[inline(always)]
    pub(super) fn syn_scope(&mut self) {}
    #[inline(always)]
    pub(super) fn syn_part(&mut self, _role: Role, _start: u32, _end: u32) {}
    #[inline(always)]
    pub(super) fn syn_close(&mut self) {}
    #[inline(always)]
    pub(super) fn syn_target(&mut self, _start: u32, _end: u32, _role: Role) {}
    #[inline(always)]
    pub(super) fn syn_decl(&mut self, _v: VarId, _kind: DeclKind) {}
    #[inline(always)]
    pub(super) fn syn_special(&mut self, _s: Special, _pos: u32) {}
    #[inline(always)]
    pub(super) fn syn_erased(&mut self, _start: u32, _end: u32) {}
}
