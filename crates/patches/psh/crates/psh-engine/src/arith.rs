//! Arithmetic over the shell's variables: psh-syntax's evaluator (`eval_arith`) with
//! the vproc as its environment, arrays included.

use psh_syntax::{ArithEnv, ArithIndex, EvalError, eval_arith};

use crate::state::Vp;

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

    fn set(
        &mut self,
        name: &[u8],
        index: Option<ArithIndex<'_>>,
        value: i64,
    ) -> Result<(), String> {
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
