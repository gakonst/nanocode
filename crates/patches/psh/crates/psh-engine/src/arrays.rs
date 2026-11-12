//! Array assignment and access on a vproc's variables (D1260): `a[i]=v`, `a=(…)`,
//! `a+=(…)`, element lookup with negative indices, `unset a[i]`.
//!
//! Subscripts of indexed arrays are arithmetic; of associative arrays, plain strings.
//! Errors are bash's messages without the `line N:` prefix.

use std::collections::BTreeMap;

use crate::state::{Vp, apply_case};
use crate::vars::{Assoc, Value};

/// One element of a compound assignment `(…)`, expanded: `[key]=value`, `[key]+=value`
/// or a plain `value`.
#[derive(Clone, Debug)]
pub(crate) struct ArrayItem {
    pub key: Option<Vec<u8>>,
    pub value: Vec<u8>,
    pub append: bool,
}

impl Vp {
    /// Applies `-i` and `-l`/`-u` of `name` to an element value; `old` is the element's
    /// current value for `+=`.
    fn convert(
        &mut self,
        name: &str,
        value: Vec<u8>,
        old: Option<Vec<u8>>,
        append: bool,
    ) -> Result<Vec<u8>, String> {
        let (integer, case) = match self.vars.get(name) {
            Some(v) => (v.integer, v.case),
            None => (false, crate::vars::Case::None),
        };
        if integer {
            let mut n = crate::arith::eval(self, &value)?;
            if append {
                n = n.wrapping_add(crate::arith::eval(self, &old.unwrap_or_default())?);
            }
            return Ok(n.to_string().into_bytes());
        }
        let value = if append {
            let mut o = old.unwrap_or_default();
            o.extend_from_slice(&value);
            o
        } else {
            value
        };
        Ok(apply_case(case, value))
    }

    fn readonly_check(&self, name: &str) -> Result<(), String> {
        if self.vars.get(name).is_some_and(|v| v.readonly) {
            Err(format!("{name}: readonly variable"))
        } else {
            Ok(())
        }
    }

    /// The highest index of an indexed array (`None` when empty or not an array).
    fn max_index(&self, name: &str) -> Option<i64> {
        match self.vars.get(name).map(|v| &v.value) {
            Some(Value::Indexed(m)) => m.keys().next_back().copied(),
            Some(Value::Str(_)) => Some(0),
            _ => None,
        }
    }

    /// Evaluates an indexed subscript; negative counts from the end. `None` when it
    /// points before the start.
    pub(crate) fn resolve_index(&mut self, name: &str, sub: &[u8]) -> Result<Option<i64>, String> {
        let i = crate::arith::eval(self, sub)?;
        if i >= 0 {
            return Ok(Some(i));
        }
        Ok(self.max_index(name).map(|m| m + 1 + i).filter(|&i| i >= 0))
    }

    /// `name[sub]=value` (or `+=`).
    pub(crate) fn assign_elem(
        &mut self,
        name: &str,
        sub: &[u8],
        value: Vec<u8>,
        append: bool,
    ) -> Result<(), String> {
        let name = self.resolve(name).into_owned();
        self.readonly_check(&name)?;
        if self.vars.get(&name).is_some_and(|v| v.is_assoc()) {
            let old = match self.vars.get(&name).map(|v| &v.value) {
                Some(Value::Assoc(a)) => a.get(sub).map(<[u8]>::to_vec),
                _ => None,
            };
            let v = self.convert(&name, value, old, append)?;
            let var = self.vars.entry(&name);
            var.declared_only = false;
            let aliases = var.dynamic == Some(crate::vars::Dynamic::BashAliases);
            if let Value::Assoc(a) = &mut var.value {
                a.insert(sub.to_vec(), v.clone());
            }
            if aliases {
                self.aliases.set(sub, &v);
            }
            return Ok(());
        }
        let Some(i) = self.resolve_index(&name, sub)? else {
            return Err(format!(
                "{name}[{}]: bad array subscript",
                String::from_utf8_lossy(sub)
            ));
        };
        let old = self.elem(&name, i);
        let v = self.convert(&name, value, old, append)?;
        let var = self.vars.entry(&name);
        var.declared_only = false;
        to_indexed(&mut var.value).insert(i, v);
        Ok(())
    }

    fn elem(&self, name: &str, i: i64) -> Option<Vec<u8>> {
        match self.vars.get(name).map(|v| &v.value) {
            Some(Value::Indexed(m)) => m.get(&i).cloned(),
            Some(Value::Str(s)) if i == 0 => Some(s.clone()),
            _ => None,
        }
    }

    /// `name=(…)` or `name+=(…)`. Indexed unless the variable is associative.
    pub(crate) fn assign_array(
        &mut self,
        name: &str,
        items: Vec<ArrayItem>,
        append: bool,
    ) -> Result<(), String> {
        let name = self.resolve(name).into_owned();
        self.readonly_check(&name)?;
        if self.vars.get(&name).is_some_and(|v| v.is_assoc()) {
            let mut a = match (&self.vars.get(&name).map(|v| &v.value), append) {
                (Some(Value::Assoc(a)), true) => a.clone(),
                _ => Assoc::default(),
            };
            // Without keys, the items are key-value pairs (bash 5.1+).
            let mut pending_key: Option<Vec<u8>> = None;
            for item in items {
                let (key, value, app) = match item.key {
                    Some(k) => (k, item.value, item.append),
                    None => match pending_key.take() {
                        Some(k) => (k, item.value, false),
                        None => {
                            pending_key = Some(item.value);
                            continue;
                        }
                    },
                };
                let old = a.get(&key).map(<[u8]>::to_vec);
                let v = self.convert(&name, value, old, app)?;
                a.insert(key, v);
            }
            if let Some(k) = pending_key {
                a.insert(k, Vec::new());
            }
            let var = self.vars.entry(&name);
            var.value = Value::Assoc(a);
            var.declared_only = false;
            return Ok(());
        }
        let mut m: BTreeMap<i64, Vec<u8>> = if append {
            match self.vars.get(&name).map(|v| &v.value) {
                Some(Value::Indexed(m)) => m.clone(),
                Some(Value::Str(s)) => BTreeMap::from([(0, s.clone())]),
                _ => BTreeMap::new(),
            }
        } else {
            BTreeMap::new()
        };
        let mut next = m.keys().next_back().map_or(0, |k| k + 1);
        // Elements are assigned in a scratch array, so that `-i` sees the old values.
        for item in items {
            let i = match &item.key {
                Some(k) => {
                    let i = crate::arith::eval(self, k)?;
                    if i < 0 {
                        match m
                            .keys()
                            .next_back()
                            .map(|mx| mx + 1 + i)
                            .filter(|&i| i >= 0)
                        {
                            Some(i) => i,
                            None => {
                                return Err(format!(
                                    "{name}[{}]: bad array subscript",
                                    String::from_utf8_lossy(k)
                                ));
                            }
                        }
                    } else {
                        i
                    }
                }
                None => next,
            };
            let old = m.get(&i).cloned();
            let v = self.convert(&name, item.value, old, item.append)?;
            m.insert(i, v);
            next = i + 1;
        }
        let var = self.vars.entry(&name);
        var.value = Value::Indexed(m);
        var.declared_only = false;
        Ok(())
    }

    /// `${name[sub]}`: the element, `None` if unset. Errors for subscripts before the
    /// start of an indexed array.
    pub(crate) fn get_elem(&mut self, name: &str, sub: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let name = self.resolve(name).into_owned();
        match self.vars.get(&name).map(|v| &v.value) {
            Some(Value::Assoc(a)) => return Ok(a.get(sub).map(<[u8]>::to_vec)),
            None | Some(Value::Unset) => {
                // Still evaluated, for its side effects and errors.
                crate::arith::eval(self, sub)?;
                return Ok(None);
            }
            _ => {}
        }
        match self.resolve_index(&name, sub)? {
            Some(i) => Ok(self.elem(&name, i)),
            None => Err(format!("{name}: bad array subscript")),
        }
    }

    /// `unset name[sub]`.
    pub(crate) fn unset_elem(&mut self, name: &str, sub: &[u8]) -> Result<(), String> {
        let name = self.resolve(name).into_owned();
        if self.vars.get(&name).is_some_and(|v| v.readonly) {
            return Err(format!("{name}: cannot unset: readonly variable"));
        }
        let assoc = self.vars.get(&name).is_some_and(|v| v.is_assoc());
        if (sub == b"@" || sub == b"*") && !assoc {
            return match self.vars.get_mut(&name).map(|v| &mut v.value) {
                Some(Value::Indexed(m)) => {
                    m.clear();
                    Ok(())
                }
                Some(_) => Err(format!("{name}: not an array variable")),
                None => Ok(()),
            };
        }
        if assoc {
            if let Some(Value::Assoc(a)) = self.vars.get_mut(&name).map(|v| &mut v.value) {
                a.remove(sub);
            }
            return Ok(());
        }
        let Some(i) = self.resolve_index(&name, sub)? else {
            return Err(format!(
                "{name}[{}]: bad array subscript",
                String::from_utf8_lossy(sub)
            ));
        };
        match self.vars.get_mut(&name).map(|v| &mut v.value) {
            Some(Value::Indexed(m)) => {
                m.remove(&i);
            }
            Some(v @ Value::Str(_)) if i == 0 => *v = Value::Unset,
            Some(Value::Unset) if i == 0 => {
                self.vars.remove(&name);
            }
            Some(Value::Str(_) | Value::Unset) => {
                return Err(format!("{name}: not an array variable"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Makes a value an indexed array (a scalar becomes element 0) and returns its map.
pub(crate) fn to_indexed(v: &mut Value) -> &mut BTreeMap<i64, Vec<u8>> {
    if !matches!(v, Value::Indexed(_)) {
        let m = match std::mem::take(v) {
            Value::Str(s) => BTreeMap::from([(0, s)]),
            _ => BTreeMap::new(),
        };
        *v = Value::Indexed(m);
    }
    match v {
        Value::Indexed(m) => m,
        _ => unreachable!(),
    }
}
