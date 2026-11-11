//! The declaration builtins: `declare`/`typeset`, `local`, `export`, `readonly`
//! (D1262). Their `name=value` and `name=(…)` arguments are assignments, expanded by the
//! interpreter without word splitting, so they come in as [`DeclArg`]s rather than
//! plain words.

use crate::arrays::ArrayItem;
use crate::state::{R, Vp, is_name};
use crate::vars::{Case, Value, Var};

use super::out;

/// An argument of a declaration builtin.
#[derive(Clone, Debug)]
pub(crate) enum DeclArg {
    /// An option or a name.
    Word(Vec<u8>),
    /// `name=value`, `name+=value`, `name[sub]=value`.
    Assign {
        name: Vec<u8>,
        index: Option<Vec<u8>>,
        append: bool,
        value: Vec<u8>,
    },
    /// `name=(…)`, `name+=(…)`.
    Array {
        name: Vec<u8>,
        append: bool,
        items: Vec<ArrayItem>,
    },
}

impl DeclArg {
    /// Reads a plain word as a declaration argument (`declare "a=1"`; also the path
    /// for `builtin declare …` and the like).
    pub(crate) fn from_word(w: Vec<u8>) -> DeclArg {
        if w.first().is_some_and(|&b| b == b'-' || b == b'+') {
            return DeclArg::Word(w);
        }
        let Some(eq) = w.iter().position(|&b| b == b'=') else {
            return DeclArg::Word(w);
        };
        let (lhs, append) = match w[..eq].strip_suffix(b"+") {
            Some(l) => (l, true),
            None => (&w[..eq], false),
        };
        let (name, index) = match lhs.iter().position(|&b| b == b'[') {
            Some(i) if lhs.ends_with(b"]") => {
                (lhs[..i].to_vec(), Some(lhs[i + 1..lhs.len() - 1].to_vec()))
            }
            _ => (lhs.to_vec(), None),
        };
        DeclArg::Assign {
            name,
            index,
            append,
            value: w[eq + 1..].to_vec(),
        }
    }

    fn name(&self) -> &[u8] {
        match self {
            DeclArg::Word(w) => w,
            DeclArg::Assign { name, .. } | DeclArg::Array { name, .. } => name,
        }
    }

    /// The argument as written, for messages.
    fn shown(&self) -> String {
        match self {
            DeclArg::Word(w) => String::from_utf8_lossy(w).into_owned(),
            DeclArg::Assign {
                name,
                index,
                append,
                value,
            } => {
                let mut s = String::from_utf8_lossy(name).into_owned();
                if let Some(i) = index {
                    s.push('[');
                    s.push_str(&String::from_utf8_lossy(i));
                    s.push(']');
                    // bash names the element only.
                    return s;
                }
                if *append {
                    s.push('+');
                }
                s.push('=');
                s.push_str(&String::from_utf8_lossy(value));
                s
            }
            DeclArg::Array { name, .. } => String::from_utf8_lossy(name).into_owned(),
        }
    }
}

/// Which builtin runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    Declare,
    Typeset,
    Local,
    Export,
    Readonly,
}

impl Which {
    pub(crate) fn from_name(name: &[u8]) -> Option<Which> {
        Some(match name {
            b"declare" => Which::Declare,
            b"typeset" => Which::Typeset,
            b"local" => Which::Local,
            b"export" => Which::Export,
            b"readonly" => Which::Readonly,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Which::Declare => "declare",
            Which::Typeset => "typeset",
            Which::Local => "local",
            Which::Export => "export",
            Which::Readonly => "readonly",
        }
    }
}

/// Attributes to set (`-x`, `Some(true)`) or clear (`+x`, `Some(false)`). Given both
/// ways, `+` wins (bash).
#[derive(Default)]
struct Opts {
    indexed: Option<bool>,
    assoc: Option<bool>,
    integer: Option<bool>,
    lower: Option<bool>,
    upper: Option<bool>,
    capitalize: Option<bool>,
    nameref: Option<bool>,
    readonly: Option<bool>,
    export: Option<bool>,
    trace: Option<bool>,
    global: bool,
    /// `-I`: a new local starts as a copy of the variable it shadows.
    inherit: bool,
    print: bool,
    funcs: bool,
    func_names: bool,
    /// The attribute letters given with `-`: they filter what `declare -p` lists.
    filter: Vec<u8>,
}

fn set(field: &mut Option<bool>, on: bool) {
    *field = if on && *field != Some(false) {
        Some(true)
    } else {
        Some(false)
    };
}

impl Opts {
    fn on(f: Option<bool>) -> bool {
        f == Some(true)
    }

    /// Whether `v` has any of the attributes in the filter.
    fn matches(&self, v: &Var) -> bool {
        self.filter.iter().any(|c| match c {
            b'a' => matches!(v.value, Value::Indexed(_)),
            b'A' => v.is_assoc(),
            b'i' => v.integer,
            b'l' => v.case == Case::Lower,
            b'u' => v.case == Case::Upper,
            b'c' => v.case == Case::Capitalize,
            b'n' => v.nameref,
            b'r' => v.readonly,
            b't' => v.trace,
            b'x' => v.exported,
            _ => false,
        })
    }
}

/// The entry point for the five builtins with already expanded arguments.
pub(crate) async fn run(vp: &mut Vp, which: Which, args: Vec<DeclArg>) -> R {
    let bname = which.name();
    if which == Which::Local && vp.frames.is_empty() {
        vp.error("local: can only be used in a function").await;
        return Ok(1);
    }
    let mut o = Opts::default();
    match which {
        Which::Export => o.export = Some(true),
        Which::Readonly => o.readonly = Some(true),
        _ => {}
    }
    let mut i = 0;
    while i < args.len() {
        let DeclArg::Word(w) = &args[i] else { break };
        if w.len() < 2 || (w[0] != b'-' && w[0] != b'+') {
            break;
        }
        if w == b"--" {
            i += 1;
            break;
        }
        let on = w[0] == b'-';
        for &c in &w[1..] {
            let export_like = matches!(which, Which::Export | Which::Readonly);
            let ok = match c {
                b'p' => {
                    o.print = true;
                    true
                }
                b'n' if which == Which::Export => {
                    o.export = Some(!on);
                    true
                }
                b'f' if export_like => {
                    o.funcs = true;
                    true
                }
                b'a' | b'A' if export_like => {
                    set(
                        if c == b'a' {
                            &mut o.indexed
                        } else {
                            &mut o.assoc
                        },
                        on,
                    );
                    true
                }
                _ if export_like => false,
                b'a' => {
                    set(&mut o.indexed, on);
                    true
                }
                b'A' => {
                    set(&mut o.assoc, on);
                    true
                }
                b'i' => {
                    set(&mut o.integer, on);
                    true
                }
                b'l' => {
                    set(&mut o.lower, on);
                    true
                }
                b'u' => {
                    set(&mut o.upper, on);
                    true
                }
                b'c' => {
                    set(&mut o.capitalize, on);
                    true
                }
                b'n' => {
                    set(&mut o.nameref, on);
                    true
                }
                b'r' => {
                    set(&mut o.readonly, on);
                    true
                }
                b'x' => {
                    set(&mut o.export, on);
                    true
                }
                b't' => {
                    set(&mut o.trace, on);
                    true
                }
                b'g' | b'f' | b'F' if which == Which::Local => false,
                b'g' => {
                    o.global = true;
                    true
                }
                b'f' => {
                    o.funcs = true;
                    true
                }
                b'F' => {
                    o.funcs = true;
                    o.func_names = true;
                    true
                }
                b'I' => {
                    o.inherit = on;
                    true
                }
                _ => false,
            };
            if !ok {
                vp.error(&format!("{bname}: -{}: invalid option", c as char))
                    .await;
                vp.error_raw(usage(which).as_bytes()).await;
                return Ok(2);
            }
            if on && b"aAilucnrtx".contains(&c) && !o.filter.contains(&c) {
                o.filter.push(c);
            }
        }
        i += 1;
    }
    if Opts::on(o.indexed) && Opts::on(o.assoc) {
        vp.error(&format!("{bname}: -a: invalid option")).await;
        vp.error_raw(usage(which).as_bytes()).await;
        return Ok(2);
    }
    let rest = &args[i..];
    if o.funcs && which != Which::Export && which != Which::Readonly {
        let attrs = o.filter.iter().any(|c| *c != b'p');
        // With attributes to set (`declare -fx f`), nothing is printed.
        if attrs && !o.print && !rest.is_empty() {
            let missing = rest
                .iter()
                .any(|a| !vp.funcs.contains_key(&*String::from_utf8_lossy(a.name())));
            return Ok(i32::from(missing));
        }
        return functions(vp, bname, rest, o.func_names, o.print).await;
    }
    if rest.is_empty() {
        return print_all(vp, which, &o).await;
    }
    if o.print {
        return print_named(vp, bname, rest).await;
    }
    // In a function, declare and typeset (and local) make locals; -g and export /
    // readonly do not.
    let local = !vp.frames.is_empty()
        && !o.global
        && matches!(which, Which::Declare | Which::Typeset | Which::Local);
    let mut status = 0;
    for a in rest {
        let mut a = a.clone();
        // `declare -a name[5]`: a bare subscripted name declares an array.
        if let DeclArg::Word(w) = &a
            && let Some(open) = w.iter().position(|&b| b == b'[')
            && w.ends_with(b"]")
            && w.len() > open + 2
            && is_name(&w[..open])
            && !matches!(which, Which::Export | Which::Readonly)
        {
            a = DeclArg::Word(w[..open].to_vec());
            o.indexed = o.indexed.or(Some(true));
        }
        let raw = a.name();
        if !is_name(raw) {
            vp.error(&format!("{bname}: `{}': not a valid identifier", a.shown()))
                .await;
            status = 1;
            continue;
        }
        let name = String::from_utf8_lossy(raw).into_owned();
        if matches!(which, Which::Export | Which::Readonly)
            && matches!(a, DeclArg::Assign { index: Some(_), .. })
        {
            vp.error(&format!("{bname}: `{}': not a valid identifier", a.shown()))
                .await;
            status = 1;
            continue;
        }
        if o.funcs {
            // `export -f` marks the function for the shells and programs started from
            // here (`export -nf` takes the mark away); `readonly -f` only checks the name.
            if !vp.funcs.contains_key(&name) {
                vp.error(&format!("{bname}: {name}: not a function")).await;
                status = 1;
            } else if which == Which::Export {
                let exported = std::rc::Rc::make_mut(&mut vp.exported_funcs);
                exported.retain(|n| *n != name);
                if o.export != Some(false) {
                    exported.push(name);
                }
            }
            continue;
        }
        match declare_one(vp, &name, &a, &o, local).await {
            Ok(()) => {}
            Err(DeclError::Msg(msg)) => {
                vp.error(&format!("{bname}: {msg}")).await;
                status = 1;
            }
            Err(DeclError::Unwind(u)) => return Err(u),
        }
    }
    Ok(status)
}

fn usage(which: Which) -> String {
    match which {
        Which::Declare => "declare: usage: declare [-aAfFgiIlnrtux] [name[=value] ...] or declare -p [-aAfFilnrtux] [name ...]\n".into(),
        Which::Typeset => "typeset: usage: typeset [-aAfFgiIlnrtux] name[=value] ... or typeset -p [-aAfFilnrtux] [name ...]\n".into(),
        Which::Local => "local: usage: local [option] name[=value] ...\n".into(),
        Which::Export => "export: usage: export [-fn] [name[=value] ...] or export -p [-f]\n".into(),
        Which::Readonly => "readonly: usage: readonly [-aAf] [name[=value] ...] or readonly -p\n".into(),
    }
}

enum DeclError {
    Msg(String),
    Unwind(crate::state::Unwind),
}

impl From<String> for DeclError {
    fn from(m: String) -> DeclError {
        DeclError::Msg(m)
    }
}

/// `(…)` as the text of an assignment to an array (`declare -a a='(1 2)'`): parsed and
/// expanded as a compound assignment, as bash does.
async fn reparse_compound(
    vp: &mut Vp,
    text: &[u8],
) -> Result<Option<Vec<crate::arrays::ArrayItem>>, DeclError> {
    let mut src = b"__psh_compound=".to_vec();
    src.extend_from_slice(text);
    let Ok(ast) = psh_syntax::parse_bytes(&src, psh_syntax::Options::default()) else {
        return Ok(None);
    };
    let Some(id) = ast.cmd_ids().next() else {
        return Ok(None);
    };
    let psh_syntax::CommandKind::Simple(sc) = &ast.cmd(id).kind else {
        return Ok(None);
    };
    let Some(psh_syntax::AssignValue::Array(lit)) = sc.assigns.first().map(|a| &a.value) else {
        return Ok(None);
    };
    let lit = lit.clone();
    let code = std::rc::Rc::new(crate::state::Code {
        lines: std::rc::Rc::new(psh_syntax::LineIndex::from_bytes(ast.source_bytes())),
        ast: std::sync::Arc::new(ast),
        line_base: vp.line.saturating_sub(1),
        arith: Default::default(),
    });
    let (items, _) = crate::expand::expand_array_lit(vp, &code, &lit)
        .await
        .map_err(DeclError::Unwind)?;
    Ok(Some(items))
}

/// Declares one name with the options, and assigns its value if given.
async fn declare_one(
    vp: &mut Vp,
    name: &str,
    a: &DeclArg,
    o: &Opts,
    local: bool,
) -> Result<(), DeclError> {
    if local {
        // A readonly variable cannot be shadowed either.
        if vp.vars.get(name).is_some_and(|v| v.readonly) {
            return Err(format!("{name}: readonly variable").into());
        }
        let fresh = vp
            .frames
            .last()
            .is_some_and(|f| !f.saved.iter().any(|(s, _)| s == name));
        // A temporary binding (`x=t f`) made local is a local from now on: unset no
        // longer reveals what it shadowed.
        if let Some(f) = vp.frames.last_mut() {
            f.temps.retain(|t| t != name);
        }
        if fresh {
            vp.make_local(name);
            // A new local starts without a value and gets only the attributes given
            // now, unless -I copies the shadowed one.
            let start = match vp.vars.get(name) {
                Some(v) if o.inherit && !v.nameref => v.clone(),
                _ => Var::default(),
            };
            vp.vars.insert(name, start);
        }
    }
    let target = if o.nameref.is_some() {
        name.to_string()
    } else {
        vp.resolve(name).into_owned()
    };
    let has_value = !matches!(a, DeclArg::Word(_));
    let existed = vp.vars.get(&target).is_some();
    {
        let var = vp.vars.entry(&target);
        let changes_attrs = o.indexed.is_some()
            || o.assoc.is_some()
            || o.integer.is_some()
            || o.lower.is_some()
            || o.upper.is_some()
            || o.capitalize.is_some()
            || o.nameref.is_some()
            || o.readonly == Some(false);
        if var.readonly && (has_value || changes_attrs) {
            return Err(format!("{target}: readonly variable").into());
        }
        if (o.indexed == Some(false) && matches!(var.value, Value::Indexed(_)))
            || (o.assoc == Some(false) && var.is_assoc())
        {
            return Err(format!("{target}: cannot destroy array variables in this way").into());
        }
        if Opts::on(o.assoc) && matches!(var.value, Value::Indexed(_)) {
            return Err(format!("{target}: cannot convert indexed to associative array").into());
        }
        if Opts::on(o.indexed) && var.is_assoc() {
            return Err(format!("{target}: cannot convert associative to indexed array").into());
        }
        if Opts::on(o.assoc) && !var.is_assoc() {
            var.declared_only = matches!(var.value, Value::Unset);
            var.value = match std::mem::take(&mut var.value) {
                Value::Str(s) => {
                    let mut a = crate::vars::Assoc::default();
                    a.insert(b"0".to_vec(), s);
                    Value::Assoc(a)
                }
                _ => Value::Assoc(Default::default()),
            };
        }
        if Opts::on(o.indexed) && !var.is_array() {
            var.declared_only = matches!(var.value, Value::Unset);
            crate::arrays::to_indexed(&mut var.value);
        }
        if let Some(i) = o.integer {
            var.integer = i;
        }
        if let Some(t) = o.trace {
            var.trace = t;
        }
        for (f, case) in [
            (o.lower, Case::Lower),
            (o.upper, Case::Upper),
            (o.capitalize, Case::Capitalize),
        ] {
            match f {
                Some(true) => var.case = case,
                Some(false) if var.case == case => var.case = Case::None,
                _ => {}
            }
        }
        if let Some(n) = o.nameref {
            // The value it will refer to must be a name (or `name[sub]`).
            let target_name = match a {
                DeclArg::Assign {
                    index: None, value, ..
                } => Some(value.clone()),
                _ => var.get().map(<[u8]>::to_vec),
            };
            // Only an explicit `declare -n x=x` is refused; an existing value is not
            // checked (it warns when read).
            if n && matches!(a, DeclArg::Assign { index: None, .. })
                && target_name.as_deref() == Some(target.as_bytes())
            {
                return Err(
                    format!("{target}: nameref variable self references not allowed").into(),
                );
            }
            if n && let Some(t) = &target_name
                && !t.is_empty()
                && !is_name(t.split(|&b| b == b'[').next().unwrap_or_default())
            {
                return Err(format!(
                    "`{}': invalid variable name for name reference",
                    String::from_utf8_lossy(t)
                )
                .into());
            }
            // An array cannot be a nameref: the attribute is dropped.
            var.nameref = n && !var.is_array();
        }
        if let Some(x) = o.export {
            var.exported = x;
        }
    }
    let _ = existed;
    match a {
        DeclArg::Word(_) => {}
        DeclArg::Assign {
            index: None,
            append,
            value,
            ..
        } => {
            let is_array = vp.vars.get(&target).is_some_and(|v| v.is_array());
            if o.nameref == Some(true) {
                vp.vars.entry(&target).set(value.clone());
            } else if is_array
                && value.first() == Some(&b'(')
                && value.last() == Some(&b')')
                && let Some(items) = reparse_compound(vp, value).await?
            {
                vp.assign_array(&target, items, *append)?;
            } else {
                let v = vp.prepare_value(&target, value.clone(), *append)?;
                vp.vars.entry(&target).set(v);
            }
        }
        DeclArg::Assign {
            index: Some(i),
            append,
            value,
            ..
        } => {
            vp.assign_elem(&target, i, value.clone(), *append)?;
        }
        DeclArg::Array { append, items, .. } => {
            vp.assign_array(&target, items.clone(), *append)?;
        }
    }
    if Opts::on(o.readonly) {
        vp.vars.entry(&target).readonly = true;
    }
    Ok(())
}

/// `declare -f` / `-F`: function definitions or names.
async fn functions(
    vp: &mut Vp,
    bname: &str,
    names: &[DeclArg],
    names_only: bool,
    print_flag: bool,
) -> R {
    let mut buf = Vec::new();
    let mut status = 0;
    let print = |vp: &Vp, name: &str, buf: &mut Vec<u8>| {
        let Some(f) = vp.funcs.get(name) else {
            return false;
        };
        if names_only && print_flag {
            buf.extend_from_slice(b"declare -f ");
            buf.extend_from_slice(name.as_bytes());
        } else if names_only {
            buf.extend_from_slice(name.as_bytes());
        } else {
            let text = psh_syntax::print_function_bash(&f.code.ast, f.def);
            buf.extend_from_slice(&f.code.ast.restore_bytes(&text));
        }
        buf.push(b'\n');
        true
    };
    if names.is_empty() {
        let mut all: Vec<&String> = vp.funcs.keys().collect();
        all.sort();
        for n in all {
            if names_only {
                buf.extend_from_slice(b"declare -f ");
                buf.extend_from_slice(n.as_bytes());
                buf.push(b'\n');
            } else {
                print(vp, n, &mut buf);
            }
        }
    } else {
        for a in names {
            let n = String::from_utf8_lossy(a.name()).into_owned();
            if !print(vp, &n, &mut buf) {
                status = 1;
            }
        }
    }
    let st = out(vp, bname, &buf).await?;
    Ok(if st != 0 { st } else { status })
}

/// `declare -p NAME…`.
async fn print_named(vp: &mut Vp, bname: &str, names: &[DeclArg]) -> R {
    let mut buf = Vec::new();
    let mut status = 0;
    for a in names {
        // Only plain names: `a[0]` or `x=1` are reported as not found, as written.
        let found = match a {
            DeclArg::Word(w) if is_name(w) => {
                let n = String::from_utf8_lossy(w).into_owned();
                vp.refresh_dynamic(&n);
                vp.vars.get(&n).map(|v| declaration(&n, v))
            }
            _ => None,
        };
        match found {
            Some(d) => buf.extend_from_slice(&d),
            None => {
                vp.error(&format!("{bname}: {}: not found", a.shown()))
                    .await;
                status = 1;
            }
        }
    }
    let st = out(vp, bname, &buf).await?;
    Ok(if st != 0 { st } else { status })
}

/// Without names: every variable (with the attributes asked for), as declarations.
async fn print_all(vp: &mut Vp, which: Which, o: &Opts) -> R {
    let mut vars: Vec<(&String, &Var)> = vp
        .vars
        .iter()
        .filter(|(_, v)| match which {
            Which::Export => v.exported,
            Which::Readonly => v.readonly,
            _ => o.filter.is_empty() || o.matches(v),
        })
        .collect();
    vars.sort_by(|a, b| a.0.cmp(b.0));
    let mut buf = Vec::new();
    let plain =
        !o.print && o.filter.is_empty() && !matches!(which, Which::Export | Which::Readonly);
    for (n, v) in vars {
        if plain {
            // `declare` alone prints `name=value` lines, like `set`.
            if let Some(val) = v.get().filter(|_| !v.is_array()) {
                buf.extend_from_slice(n.as_bytes());
                buf.push(b'=');
                buf.extend_from_slice(&set_quote(val));
                buf.push(b'\n');
                continue;
            }
            if !v.is_array() {
                continue;
            }
        }
        buf.extend_from_slice(&declaration(n, v));
    }
    out(vp, which.name(), &buf).await
}

/// A value as `set` and `declare` without options print it: bare when safe, `'…'`, or
/// `$'…'` with control characters.
pub(crate) fn set_quote(v: &[u8]) -> Vec<u8> {
    if v.iter().any(|b| b.is_ascii_control()) {
        return ansi_c_quote(v);
    }
    let safe = !v.is_empty()
        && v.iter()
            .all(|&b| b.is_ascii_alphanumeric() || b"_./:,=+@%^-".contains(&b) || b >= 0x80);
    if safe {
        return v.to_vec();
    }
    let mut out = vec![b'\''];
    for &b in v {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// `declare -FLAGS name=value` as `declare -p` prints it.
pub(crate) fn declaration(name: &str, v: &Var) -> Vec<u8> {
    let mut out = format!("declare -{} {name}", v.flags()).into_bytes();
    match &v.value {
        Value::Unset => {}
        Value::Str(s) => {
            out.push(b'=');
            out.extend_from_slice(&value_quote(s));
        }
        Value::Indexed(m) if m.is_empty() && v.declared_only => {}
        Value::Assoc(a) if a.len() == 0 && v.declared_only => {}
        Value::Indexed(m) => {
            out.extend_from_slice(b"=(");
            for (i, (k, val)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                out.extend_from_slice(format!("[{k}]=").as_bytes());
                out.extend_from_slice(&value_quote(val));
            }
            out.push(b')');
        }
        Value::Assoc(a) => {
            out.extend_from_slice(b"=(");
            for (k, val) in a.iter() {
                out.push(b'[');
                out.extend_from_slice(&key_quote(k));
                out.extend_from_slice(b"]=");
                out.extend_from_slice(&value_quote(val));
                out.push(b' ');
            }
            out.push(b')');
        }
    }
    out.push(b'\n');
    out
}

/// A value as `declare -p` quotes it: `"…"` with `" \ $` and backquote escaped, or
/// `$'…'` when it has control characters.
pub(crate) fn value_quote(v: &[u8]) -> Vec<u8> {
    if v.iter().any(|b| b.is_ascii_control()) {
        return ansi_c_quote(v);
    }
    let mut out = vec![b'"'];
    for &b in v {
        if matches!(b, b'"' | b'\\' | b'$' | b'`') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b'"');
    out
}

/// An associative key: bare when it has no special chars, else like a value.
fn key_quote(k: &[u8]) -> Vec<u8> {
    let plain = !k.is_empty()
        && k.iter().all(|&b| {
            !b.is_ascii_control()
                && !matches!(
                    b,
                    b' ' | b'\t'
                        | b'"'
                        | b'\''
                        | b'\\'
                        | b'$'
                        | b'`'
                        | b'['
                        | b']'
                        | b'*'
                        | b'?'
                        | b'('
                        | b')'
                        | b'<'
                        | b'>'
                        | b'&'
                        | b'|'
                        | b';'
                        | b'{'
                        | b'}'
                        | b'#'
                        | b'~'
                        | b'!'
                        | b'='
                )
        });
    if plain { k.to_vec() } else { value_quote(k) }
}

fn ansi_c_quote(v: &[u8]) -> Vec<u8> {
    let mut out = b"$'".to_vec();
    for &b in v {
        match b {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x1b => out.extend_from_slice(b"\\E"),
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b if b.is_ascii_control() => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(b'\'');
    out
}
