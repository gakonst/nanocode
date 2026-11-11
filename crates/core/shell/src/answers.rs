use psh_classify::{Classification, Via, classify_ast};
use psh_syntax::{
    AndOr, AssignValue, Ast, BinaryTest, Brace, CmdId, Command, CommandKind, CondExpr, CondKind,
    DefaultKind, ListId, Param, ParamName, ParamOp, ParamPrefix, Part, Pipeline, RedirFd,
    RedirTarget, SimpleCommand, Subscript, UnaryTest, Word, WordPart, backtick_body, literal_text,
    parse,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub struct Answers {
    pub commands: BTreeSet<String>,
    pub variables: BTreeSet<String>,
}

const REFUSED_BUILTINS: [&str; 20] = [
    "alias", "bg", "dirs", "disown", "fg", "hash", "help", "history", "jobs", "kill", "popd",
    "printenv", "pushd", "time", "times", "type", "ulimit", "umask", "unalias", "which",
];

const REFUSED_PARAMETERS: [&str; 6] = [
    "BASHPID",
    "EPOCHREALTIME",
    "EPOCHSECONDS",
    "RANDOM",
    "SECONDS",
    "SRANDOM",
];

const UNSET_PARAMETERS: [&str; 20] = [
    "BASH_ARGC",
    "BASH_ARGV",
    "BASH_COMPAT",
    "BASH_ENV",
    "BASH_LINENO",
    "BASH_REMATCH",
    "BASH_SOURCE",
    "BASH_XTRACEFD",
    "CDPATH",
    "ENV",
    "FUNCNAME",
    "GLOBIGNORE",
    "LINENO",
    "MAPFILE",
    "OLDPWD",
    "OPTARG",
    "PIPESTATUS",
    "POSIXLY_CORRECT",
    "REPLY",
    "TIMEFORMAT",
];

const SHELL_BUILTINS: [&str; 30] = [
    ":",
    "[",
    "break",
    "cd",
    "command",
    "continue",
    "declare",
    "env",
    "exit",
    "export",
    "false",
    "getopts",
    "let",
    "local",
    "mapfile",
    "printf",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "set",
    "shift",
    "shopt",
    "test",
    "timeout",
    "true",
    "typeset",
    "unset",
    "wait",
];

const UNPARSED: &str = "syntax error";

const UNSEEN_CODE: [&str; 6] = ["eval", "source", ".", "trap", "exec", "builtin"];

const SEEN_REASONS: [&str; 2] = ["calls a function defined in the script", "unknown program"];

impl Answers {
    pub(crate) fn runs(&self, name: &str) -> bool {
        self.commands.contains(name) || SHELL_BUILTINS.contains(&name)
    }
}

pub(crate) fn admit(source: &[u8], answers: &Answers) -> Result<(), String> {
    let text = String::from_utf8_lossy(source);
    let ast = parse(&text).map_err(|_| UNPARSED.to_owned())?;
    let found = classify_ast(&ast);
    commands(&found, answers)?;
    Walk::new(answers).list(&ast, ast.root())?;
    for (at, nested) in found.sources.iter().enumerate() {
        let quoted = |command: &psh_classify::CommandInfo| {
            command.source == Some(at) && command.via == Via::Backtick
        };
        if found.commands.iter().any(quoted) {
            continue;
        }
        let inner = parse(&nested.text).map_err(|_| UNPARSED.to_owned())?;
        Walk::new(answers).list(&inner, inner.root())?;
    }
    Ok(())
}

fn commands(found: &Classification, answers: &Answers) -> Result<(), String> {
    for (at, command) in found.commands.iter().enumerate() {
        let Some(first) = command.words.first() else {
            continue;
        };
        let Some(name) = command.name.as_deref() else {
            return Err(first.text.clone());
        };
        if first
            .value
            .as_deref()
            .is_some_and(|value| value.contains('/'))
        {
            return Err(first.text.clone());
        }
        if command.reason == Some("calls a function defined in the script") {
            continue;
        }
        let values: Vec<Option<&str>> = command.words[1..]
            .iter()
            .map(|word| word.value.as_deref())
            .collect();
        if refused_builtin(name, &values) || UNSEEN_CODE.contains(&name) {
            return Err(name.to_owned());
        }
        if command
            .reason
            .is_some_and(|reason| !SEEN_REASONS.contains(&reason))
        {
            return Err(name.to_owned());
        }
        if matches!(name, "bash" | "sh") {
            let seen = |inner: &psh_classify::CommandInfo| {
                inner.parent == Some(at) && inner.via == Via::ShellString
            };
            if found.commands.iter().any(seen) {
                continue;
            }
            return Err(name.to_owned());
        }
        if SHELL_BUILTINS.contains(&name) {
            continue;
        }
        if !answers.commands.contains(name) {
            return Err(name.to_owned());
        }
    }
    Ok(())
}

fn refused_builtin(name: &str, args: &[Option<&str>]) -> bool {
    if REFUSED_BUILTINS.contains(&name) {
        return true;
    }
    let dynamic = args.iter().any(Option::is_none);
    let literal = || args.iter().flatten();
    match name {
        "command" => {
            dynamic
                || args
                    .first()
                    .is_some_and(|arg| arg.is_some_and(|arg| arg.starts_with('-')))
        }
        "env" => {
            dynamic || {
                let mut rest = literal().peekable();
                loop {
                    match rest.peek() {
                        None => break true,
                        Some(&&"-u") => {
                            rest.next();
                            rest.next();
                        }
                        Some(arg) if arg.starts_with('-') || assignment(arg) => {
                            rest.next();
                        }
                        Some(_) => break false,
                    }
                }
            }
        }
        "set" => dynamic || matches!(args.last(), None | Some(Some("-o" | "+o"))),
        "shopt" => {
            dynamic
                || !literal().any(|arg| {
                    arg.strip_prefix('-')
                        .is_some_and(|flags| flags.contains(['q', 's', 'u']))
                })
        }
        "declare" | "typeset" | "local" | "export" | "readonly" => {
            let functions = literal().any(|arg| {
                arg.strip_prefix('-')
                    .is_some_and(|flags| flags.contains(['f', 'F']))
            });
            let printing = literal().any(|arg| {
                arg.strip_prefix('-')
                    .is_some_and(|flags| flags.contains('p'))
            });
            let targets = args
                .iter()
                .any(|arg| arg.is_none_or(|arg| !arg.starts_with(['-', '+'])));
            !functions && (printing || !targets)
        }
        _ => false,
    }
}

fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| is_name(name))
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct Walk {
    known: BTreeSet<String>,
    maps: BTreeSet<String>,
    keyed: bool,
}

impl Walk {
    fn new(answers: &Answers) -> Self {
        Self {
            known: answers.variables.clone(),
            maps: BTreeSet::new(),
            keyed: false,
        }
    }

    fn apart(&self) -> Self {
        Self {
            known: self.known.clone(),
            maps: self.maps.clone(),
            keyed: false,
        }
    }

    fn subscript(&mut self, ast: &Ast, name: &str, index: &Word) -> Result<(), String> {
        if self.maps.contains(name) {
            self.word(ast, index)
        } else {
            self.arith(ast, index)
        }
    }

    fn reads(&self, name: &str) -> Result<(), String> {
        if REFUSED_PARAMETERS.contains(&name) {
            return Err(format!("${name}"));
        }
        if self.known.contains(name) || UNSET_PARAMETERS.contains(&name) {
            return Ok(());
        }
        Err(format!("${name}"))
    }

    fn list(&mut self, ast: &Ast, id: ListId) -> Result<(), String> {
        for item in &ast.list(id).items {
            self.and_or(ast, item)?;
        }
        Ok(())
    }

    fn and_or(&mut self, ast: &Ast, item: &AndOr) -> Result<(), String> {
        if item.background {
            return Err("background job".to_owned());
        }
        self.pipeline(ast, &item.first)?;
        let mut chain = self.apart();
        for (_, pipeline) in &item.rest {
            chain.pipeline(ast, pipeline)?;
        }
        Ok(())
    }

    fn pipeline(&mut self, ast: &Ast, pipeline: &Pipeline) -> Result<(), String> {
        if pipeline.time.is_some() {
            return Err("time".to_owned());
        }
        if let [stage] = pipeline.stages.as_slice() {
            return self.command(ast, stage.cmd);
        }
        for stage in &pipeline.stages {
            self.apart().command(ast, stage.cmd)?;
        }
        Ok(())
    }

    fn command(&mut self, ast: &Ast, id: CmdId) -> Result<(), String> {
        let command: &Command = ast.cmd(id);
        match &command.kind {
            CommandKind::Simple(simple) => self.simple(ast, simple)?,
            CommandKind::Brace(list) => self.list(ast, *list)?,
            CommandKind::Subshell(list) => self.apart().list(ast, *list)?,
            CommandKind::If(found) => {
                let mut every: Option<BTreeSet<String>> = None;
                let mut meet = |known: BTreeSet<String>| {
                    every = Some(match every.take() {
                        Some(so_far) => so_far.intersection(&known).cloned().collect(),
                        None => known,
                    });
                };
                let mut branches = found.branches.iter();
                if let Some(first) = branches.next() {
                    self.list(ast, first.cond)?;
                    let mut body = self.apart();
                    body.list(ast, first.body)?;
                    meet(body.known);
                }
                let mut later = self.apart();
                for branch in branches {
                    later.list(ast, branch.cond)?;
                    let mut body = later.apart();
                    body.list(ast, branch.body)?;
                    meet(body.known);
                }
                if let Some(body) = found.else_body {
                    later.list(ast, body)?;
                    meet(later.known);
                    if let Some(every) = every {
                        self.known = every;
                    }
                }
            }
            CommandKind::While(found) => {
                self.list(ast, found.cond)?;
                self.apart().list(ast, found.body)?;
            }
            CommandKind::For(found) => {
                let name = literal_text(ast, &found.var).filter(|name| is_name(name));
                let name = name.ok_or_else(|| ast.text(found.var.span).to_owned())?;
                for word in found.words.iter().flatten() {
                    self.word(ast, word)?;
                }
                let mut body = self.apart();
                body.known.insert(name);
                body.list(ast, found.body)?;
            }
            CommandKind::Select(_) => return Err("select".to_owned()),
            CommandKind::ArithFor(found) => {
                self.arith(ast, &found.init)?;
                self.arith(ast, &found.cond)?;
                let mut body = self.apart();
                body.list(ast, found.body)?;
                body.arith(ast, &found.step)?;
            }
            CommandKind::Case(found) => {
                self.word(ast, &found.word)?;
                for item in &found.items {
                    let mut arm = self.apart();
                    for pattern in &item.patterns {
                        arm.word(ast, pattern)?;
                    }
                    if let Some(body) = item.body {
                        arm.list(ast, body)?;
                    }
                }
            }
            CommandKind::Arith(word) => self.arith(ast, word)?,
            CommandKind::Cond(expr) => self.cond(ast, expr)?,
            CommandKind::Function(found) => self.apart().command(ast, found.body)?,
            CommandKind::Coproc(_) => return Err("coproc".to_owned()),
        }
        for redirect in &command.redirects {
            match &redirect.target {
                RedirTarget::Word(word) => self.word(ast, word)?,
                RedirTarget::HereDoc(id) => {
                    let found = ast.heredoc(*id);
                    if !found.quoted {
                        self.word(ast, &found.body)?;
                    }
                }
            }
            if let Some(RedirFd::Var(span)) = redirect.fd {
                self.known.insert(ast.text(span).to_owned());
            }
        }
        Ok(())
    }

    fn simple(&mut self, ast: &Ast, simple: &SimpleCommand) -> Result<(), String> {
        let mut assigned = Vec::new();
        for assign in &simple.assigns {
            let name = ast.text(assign.name);
            if assign.append {
                self.reads(name)?;
            }
            if let Some(index) = &assign.index {
                self.subscript(ast, name, index)?;
            }
            match &assign.value {
                AssignValue::Scalar(word) => self.word(ast, word)?,
                AssignValue::Array(array) => {
                    for element in &array.elems {
                        if let Some(key) = &element.key {
                            self.subscript(ast, name, key)?;
                        }
                        self.word(ast, &element.value)?;
                    }
                }
            }
            assigned.push(name.to_owned());
        }
        let declares = simple
            .words
            .first()
            .and_then(|first| literal_text(ast, first))
            .is_some_and(|name| matches!(name.as_str(), "declare" | "typeset" | "local"));
        let flagged = |word: &Word| {
            let text = ast.text(word.span);
            text.starts_with('-') && text.contains('A')
        };
        self.keyed = declares && simple.words.iter().any(flagged);
        if self.keyed {
            for word in &simple.words[1..] {
                let text = ast.text(word.span);
                let target = text.split(['=', '+']).next().unwrap_or(text);
                if is_name(target) {
                    self.maps.insert(target.to_owned());
                }
            }
        }
        for word in &simple.words {
            self.word(ast, word)?;
        }
        self.keyed = false;
        let Some(first) = simple.words.first() else {
            self.known.extend(assigned);
            return Ok(());
        };
        let Some(name) = literal_text(ast, first) else {
            return Ok(());
        };
        let args = &simple.words[1..];
        match name.as_str() {
            "cd" if args.is_empty() => self.reads("HOME")?,
            "read" => {
                let mut named = false;
                let mut words = args.iter();
                while let Some(word) = words.next() {
                    let text = literal_text(ast, word).ok_or_else(|| "read".to_owned())?;
                    if let Some(flags) = text.strip_prefix('-') {
                        if flags.ends_with(['a', 'd', 'i', 'n', 'N', 'p', 't', 'u']) {
                            let value = words.next().ok_or_else(|| "read".to_owned())?;
                            if flags.ends_with('a') {
                                let array = literal_text(ast, value);
                                self.known.insert(array.ok_or_else(|| "read".to_owned())?);
                                named = true;
                            }
                        }
                        continue;
                    }
                    self.known.insert(text);
                    named = true;
                }
                if !named {
                    self.known.insert("REPLY".to_owned());
                }
            }
            "mapfile" | "readarray" => {
                let last = args.last().and_then(|word| literal_text(ast, word));
                match last.filter(|name| is_name(name)) {
                    Some(array) => self.known.insert(array),
                    None => self.known.insert("MAPFILE".to_owned()),
                };
            }
            "getopts" => {
                let target = args.get(1).and_then(|word| literal_text(ast, word));
                self.known
                    .insert(target.ok_or_else(|| "getopts".to_owned())?);
                self.known.insert("OPTARG".to_owned());
                self.known.insert("OPTIND".to_owned());
            }
            "printf" => {
                if args
                    .first()
                    .and_then(|word| literal_text(ast, word))
                    .as_deref()
                    == Some("-v")
                {
                    let target = args.get(1).and_then(|word| literal_text(ast, word));
                    self.known
                        .insert(target.ok_or_else(|| "printf".to_owned())?);
                }
            }
            "unset" | "export" | "readonly" | "declare" | "typeset" | "local" => {
                for word in args {
                    let text = ast.text(word.span);
                    let target = text.split(['=', '+']).next().unwrap_or(text);
                    if is_name(target) {
                        self.known.insert(target.to_owned());
                    }
                }
            }
            "let" => {
                for word in args {
                    self.arith(ast, word)?;
                }
            }
            "test" | "[" => {
                let mut words = args.iter();
                while let Some(word) = words.next() {
                    if literal_text(ast, word).as_deref() == Some("-v") {
                        let target = words.next().and_then(|word| literal_text(ast, word));
                        self.reads(&target.ok_or_else(|| "test".to_owned())?)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn cond(&mut self, ast: &Ast, expr: &CondExpr) -> Result<(), String> {
        match &expr.kind {
            CondKind::Unary { op, arg, .. } => {
                if matches!(
                    op,
                    UnaryTest::ShellVariableIsSetAndAssigned
                        | UnaryTest::ShellVariableIsSetAndNameRef
                ) {
                    let target = literal_text(ast, arg).ok_or_else(|| "[[".to_owned())?;
                    let name = target.split('[').next().unwrap_or(&target);
                    return self.reads(name);
                }
                self.word(ast, arg)
            }
            CondKind::Binary { op, lhs, rhs, .. } => {
                if matches!(
                    op,
                    BinaryTest::ArithmeticEqualTo
                        | BinaryTest::ArithmeticNotEqualTo
                        | BinaryTest::ArithmeticLessThan
                        | BinaryTest::ArithmeticLessThanOrEqualTo
                        | BinaryTest::ArithmeticGreaterThan
                        | BinaryTest::ArithmeticGreaterThanOrEqualTo
                ) {
                    self.arith(ast, lhs)?;
                    return self.arith(ast, rhs);
                }
                self.word(ast, lhs)?;
                self.word(ast, rhs)
            }
            CondKind::Word(word) => self.word(ast, word),
            CondKind::Not(inner) | CondKind::Group(inner) => self.cond(ast, inner),
            CondKind::And(left, right) | CondKind::Or(left, right) => {
                self.cond(ast, left)?;
                self.cond(ast, right)
            }
        }
    }

    fn arith(&mut self, ast: &Ast, word: &Word) -> Result<(), String> {
        self.word(ast, word)?;
        let mut assigned = Vec::new();
        for part in &word.parts {
            if !matches!(part.kind, Part::Lit) {
                continue;
            }
            let text = ast.text(part.span).as_bytes();
            let mut at = 0;
            while at < text.len() {
                let start = at;
                if text[at].is_ascii_digit() {
                    while at < text.len()
                        && (text[at].is_ascii_alphanumeric()
                            || matches!(text[at], b'#' | b'_' | b'@'))
                    {
                        at += 1;
                    }
                    continue;
                }
                if !(text[at].is_ascii_alphabetic() || text[at] == b'_') {
                    at += 1;
                    continue;
                }
                while at < text.len() && (text[at].is_ascii_alphanumeric() || text[at] == b'_') {
                    at += 1;
                }
                let name = String::from_utf8_lossy(&text[start..at]).into_owned();
                let rest = text[at..].trim_ascii_start();
                if rest.first() == Some(&b'=') && rest.get(1) != Some(&b'=') {
                    assigned.push(name);
                } else {
                    self.reads(&name)?;
                }
            }
        }
        self.known.extend(assigned);
        Ok(())
    }

    fn word(&mut self, ast: &Ast, word: &Word) -> Result<(), String> {
        self.parts(ast, &word.parts, false)
    }

    fn parts(&mut self, ast: &Ast, parts: &[WordPart], quoted: bool) -> Result<(), String> {
        for (at, part) in parts.iter().enumerate() {
            match &part.kind {
                Part::Lit => {
                    let text = ast.text(part.span);
                    if at == 0 && !quoted && text.starts_with('~') {
                        let user = &text[1..text.find('/').unwrap_or(text.len())];
                        match user {
                            "" => self.reads("HOME")?,
                            "+" | "-" => {}
                            _ => return Err(format!("~{user}")),
                        }
                    }
                }
                Part::Escaped | Part::Single | Part::AnsiC => {}
                Part::Double(inner) | Part::Locale(inner) => self.parts(ast, inner, true)?,
                Part::Param(param) => self.param(ast, param)?,
                Part::BadSubst => return Err(ast.text(part.span).to_owned()),
                Part::CmdSubst(list) | Part::ProcSubst { list, .. } => {
                    self.apart().list(ast, *list)?;
                }
                Part::FunSub { list, .. } => self.list(ast, *list)?,
                Part::Backtick => {
                    let body = backtick_body(ast.text(part.span), quoted);
                    let inner = parse(&body).map_err(|_| UNPARSED.to_owned())?;
                    self.apart().list(&inner, inner.root())?;
                }
                Part::Arith(word) => self.arith(ast, word)?,
                Part::Brace(brace) => {
                    if let Brace::Alts(words) = &**brace {
                        for word in words {
                            self.parts(ast, &word.parts, quoted)?;
                        }
                    }
                }
                Part::ExtGlob { alts, .. } => {
                    for word in alts {
                        self.word(ast, word)?;
                    }
                }
                Part::Array(array) => {
                    for element in &array.elems {
                        match &element.key {
                            Some(key) if self.keyed => self.word(ast, key)?,
                            Some(key) => self.arith(ast, key)?,
                            None => {}
                        }
                        self.word(ast, &element.value)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn param(&mut self, ast: &Ast, param: &Param) -> Result<(), String> {
        let text = ast.text(param.name_span);
        if param.prefix == ParamPrefix::Indirect || matches!(param.op, ParamOp::Names { .. }) {
            return Err(format!("${{!{text}}}"));
        }
        match param.name {
            ParamName::Var => self.reads(text)?,
            ParamName::Positional => {}
            ParamName::Special(b'$') => return Err("$$".to_owned()),
            ParamName::Special(_) => {}
        }
        if let Some(Subscript::Expr(index)) = &param.index {
            self.subscript(ast, text, index)?;
        }
        match &param.op {
            ParamOp::Default { kind, word, .. } => {
                self.word(ast, word)?;
                if *kind == DefaultKind::Assign && param.name == ParamName::Var {
                    self.known.insert(text.to_owned());
                }
            }
            ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                self.word(ast, pattern)?;
            }
            ParamOp::Replace {
                pattern,
                replacement,
                ..
            } => {
                self.word(ast, pattern)?;
                if let Some(replacement) = replacement {
                    self.word(ast, replacement)?;
                }
            }
            ParamOp::Case { pattern, .. } => {
                if let Some(pattern) = pattern {
                    self.word(ast, pattern)?;
                }
            }
            ParamOp::Substring { offset, length } => {
                self.arith(ast, offset)?;
                if let Some(length) = length {
                    self.arith(ast, length)?;
                }
            }
            ParamOp::None | ParamOp::Transform(_) | ParamOp::Names { .. } => {}
        }
        Ok(())
    }
}
