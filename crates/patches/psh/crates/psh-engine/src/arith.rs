//! Arithmetic over the shell's variables: psh-syntax's evaluator (`eval_arith`) with
//! the vproc as its environment, arrays included.

use std::io::Write;
use std::rc::Rc;

use psh_syntax::{
    ArithEnv, ArithExpr, ArithIndex, ArithValue, EvalError, Part, Word, eval_arith,
    eval_arith_expr, parse_arith, plain_decimal,
};

use crate::state::{Code, Vp};
use crate::vars::Value;

struct Env<'a> {
    vp: &'a mut Vp,
}

impl ArithEnv for Env<'_> {
    type Error = String;

    fn is_assoc(&mut self, name: &[u8]) -> bool {
        let name = String::from_utf8_lossy(name);
        self.vp.var_ref(&name).is_some_and(|v| v.is_assoc())
    }

    fn get(
        &mut self,
        name: &[u8],
        index: Option<ArithIndex<'_>>,
    ) -> Result<Option<Vec<u8>>, String> {
        let name = String::from_utf8_lossy(name).into_owned();
        match index {
            None => match self.vp.target(&name)? {
                (n, Some(sub)) => self.vp.get_elem(&n, &sub),
                (n, None) => {
                    if n == "BASHPID" && self.vp.bashpid.is_none() {
                        return Err(crate::expand::pid_error(self.vp, b"BASHPID"));
                    }
                    self.vp.refresh_dynamic(&n);
                    Ok(self.vp.vars.value(&n).map(<[u8]>::to_vec))
                }
            },
            Some(ArithIndex::Key(k)) => self.vp.get_elem(&name, k),
            // Out of range below the start: 0, without an error (bash).
            Some(ArithIndex::Num(i)) => Ok(self
                .vp
                .get_elem(&name, i.to_string().as_bytes())
                .unwrap_or(None)),
        }
    }

    /// A variable that is neither a reference nor computed is read where it is: a loop
    /// counter costs no copy of its name or its digits.
    fn load(&mut self, name: &[u8], index: Option<ArithIndex<'_>>) -> Result<ArithValue, String> {
        if index.is_none()
            && let Ok(n) = std::str::from_utf8(name)
            && n != "BASHPID"
            && let Some(var) = self.vp.vars.get(n)
            && !var.nameref
            && var.dynamic.is_none()
        {
            return Ok(match var.get() {
                None => ArithValue::Unset,
                Some(v) => match plain_decimal(v) {
                    Some(n) => ArithValue::Num(n),
                    None => ArithValue::Text(v.to_vec()),
                },
            });
        }
        Ok(match self.get(name, index)? {
            None => ArithValue::Unset,
            Some(v) => match plain_decimal(&v) {
                Some(n) => ArithValue::Num(n),
                None => ArithValue::Text(v),
            },
        })
    }

    fn set(
        &mut self,
        name: &[u8],
        index: Option<ArithIndex<'_>>,
        value: i64,
    ) -> Result<(), String> {
        // A plain scalar takes the digits in the buffer it has: what `assign` does for
        // it, attribute by attribute, without a new name or value.
        if index.is_none()
            && let Ok(n) = std::str::from_utf8(name)
        {
            let allexport = self.vp.opts.get(crate::options::SetOpt::Allexport);
            if let Some(var) = self.vp.vars.get_mut(n)
                && !var.nameref
                && !var.readonly
                && var.dynamic.is_none()
                && let Value::Str(s) = &mut var.value
            {
                s.clear();
                let _ = write!(s, "{value}");
                var.declared_only = false;
                var.exported |= allexport;
                return Ok(());
            }
        }
        let name = String::from_utf8_lossy(name).into_owned();
        let v = value.to_string().into_bytes();
        match index {
            None => self.vp.assign(&name, v),
            Some(ArithIndex::Key(k)) => self.vp.assign_elem(&name, k, v, false),
            Some(ArithIndex::Num(i)) => {
                self.vp
                    .assign_elem(&name, i.to_string().as_bytes(), v, false)
            }
        }
    }
}

/// Evaluates `text`; the error is bash's message without the `line N:` prefix.
pub(crate) fn eval(vp: &mut Vp, text: &[u8]) -> Result<i64, String> {
    match eval_arith(text, &mut Env { vp }) {
        Ok(v) => Ok(v),
        Err(EvalError::Env(msg)) => Err(msg),
        Err(e) => Err(e.bash_text().unwrap_or_default()),
    }
}

/// An arithmetic word without expansions: its text, and the expression parsed from it
/// (`None` for blank text, which is 0).
pub(crate) struct Literal {
    pub text: Vec<u8>,
    expr: Option<ArithExpr>,
}

/// The parsed form of an arithmetic word that is literal text only, kept with the
/// script so a loop expands and parses it once. `None` sends the word down the usual
/// path: it has an expansion, a `~` (which may be a tilde prefix), or does not parse
/// (the error is reported there).
pub(crate) fn literal(code: &Code, word: &Word) -> Option<Rc<Literal>> {
    let key = (word.span.start, word.span.end);
    if let Some(known) = code.arith.borrow().get(&key) {
        return Some(known.clone());
    }
    if !word.parts.iter().all(|p| matches!(p.kind, Part::Lit)) {
        return None;
    }
    let mut text = Vec::new();
    for p in &word.parts {
        text.extend_from_slice(code.ast.bytes(p.span));
    }
    if text.contains(&b'~') {
        return None;
    }
    let expr = parse_arith(&text).ok()?;
    let lit = Rc::new(Literal { text, expr });
    code.arith.borrow_mut().insert(key, lit.clone());
    Some(lit)
}

/// Evaluates a [`Literal`], as [`eval`] evaluates its text.
pub(crate) fn eval_literal(vp: &mut Vp, lit: &Literal) -> Result<i64, String> {
    let Some(expr) = &lit.expr else {
        return Ok(0);
    };
    match eval_arith_expr(expr, &lit.text, &mut Env { vp }) {
        Ok(v) => Ok(v),
        Err(EvalError::Env(msg)) => Err(msg),
        Err(e) => Err(e.bash_text().unwrap_or_default()),
    }
}
