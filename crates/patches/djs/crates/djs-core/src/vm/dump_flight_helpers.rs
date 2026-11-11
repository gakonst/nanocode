//! Iterator helpers in plain dumps (D2877): made again by the public methods over the
//! saved state. The underlying iterator record is `$djs.I(it, next)`, a callback
//! that a counter already passed is `$djs.M(f, k)` (it calls `f` with the counter
//! shifted by `k`), take's and drop's remaining counts are passed as limits, and helpers
//! that stopped inside a flatMap's inner iterator or a concat's item are made one step
//! before and stepped once while the dump loads (`$djs.FM`, `$djs.CC`: the step yields
//! a placeholder; no user code runs). A completed helper is `$djs.HD()`. Re-dumps read
//! these wrappers back, so the original's state is written again, not the wrappers.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;

impl Dumper<'_> {
    pub(super) fn plain_helper_expr(&mut self, n: usize, o: Obj, before: u32) -> String {
        let h = self.h;
        let slot = |i: u32| h.object_slot(o, i);
        let kind = slot(0).as_int().unwrap_or(-1);
        let state = slot(1).as_int().unwrap_or(-1);
        let e = if state == 3 && kind != 0 {
            String::from("$djs.HD()")
        } else {
            match self.plain_helper_text(kind, o) {
                Some(e) => e,
                None => {
                    let p = self.nodes[n].path.clone();
                    self.refuse(
                        p,
                        "plain dumps: an iterator helper of this kind in flight (0203)",
                    );
                    String::from("undefined")
                }
            }
        };
        let name = self.fresh("i");
        self.line(cat!(name, " = ", e, ";"), before);
        self.nodes[n].loc = name.clone();
        self.nodes[n].state = State::Done;
        name
    }

    fn plain_helper_text(&mut self, kind: i32, o: Obj) -> Option<String> {
        let h = self.h;
        let slot = |i: u32| h.object_slot(o, i);
        let rec = |d: &mut Self, it: Value, next: Value| -> Option<String> {
            let (it, next) = unwrap_record(h, it, next);
            let ie = d.expr(it, None, None);
            let ne = d.expr(next, None, None);
            (!ie.is_empty() && !ne.is_empty()).then(|| cat!(ie, ", ", ne))
        };
        let callback = |d: &mut Self, f: Value, k: Value| -> Option<String> {
            let (f, k) = unwrap_callback(h, f, k)?;
            let fe = d.expr(f, None, None);
            (!fe.is_empty()).then(|| cat!("$djs.M(", fe, ", ", k, ")"))
        };
        Some(match kind {
            0 => cat!("Iterator.from($djs.I(", rec(self, slot(2), slot(3))?, "))"),
            1 | 2 => cat!(
                "Iterator.prototype.",
                if kind == 1 { "map" } else { "filter" },
                ".call($djs.I(",
                rec(self, slot(2), slot(3))?,
                "), ",
                callback(self, slot(4), slot(5))?,
                ")"
            ),
            3 | 4 => {
                let limit = slot(4).as_number()?;
                let limit = if limit.is_infinite() {
                    String::from("Infinity")
                } else {
                    cat!(limit as i64)
                };
                cat!(
                    "Iterator.prototype.",
                    if kind == 3 { "take" } else { "drop" },
                    ".call($djs.I(",
                    rec(self, slot(2), slot(3))?,
                    "), ",
                    limit,
                    ")"
                )
            }
            5 if slot(6).is_undefined() => cat!(
                "Iterator.prototype.flatMap.call($djs.I(",
                rec(self, slot(2), slot(3))?,
                "), ",
                callback(self, slot(4), slot(5))?,
                ")"
            ),
            5 => {
                let (f, k) = unwrap_callback(h, slot(4), slot(5))?;
                let fe = self.expr(f, None, None);
                let outer = rec(self, slot(2), slot(3))?;
                let inner = rec(self, slot(6), slot(7))?;
                cat!("$djs.FM(", outer, ", ", fe, ", ", k, ", ", inner, ")")
            }
            6 => {
                // The iterables not yet opened, from the index of the next one.
                let pairs = super::origin::elements(h, slot(4))?;
                let from = slot(5).as_int().unwrap_or(0).max(0) as usize;
                let mut items: Vec<String> = Vec::new();
                for p in pairs.iter().skip(from) {
                    let pair = super::origin::elements(h, *p)?;
                    let (m, it) = unwrap_concat_item(h, *pair.first()?, *pair.get(1)?);
                    let me = self.expr(m, None, None);
                    let ie = self.expr(it, None, None);
                    items.push_out(cat!("$djs.CI(", me, ", ", ie, ")"));
                }
                if slot(2).is_undefined() {
                    cat!("Iterator.concat(", join(&items, ", "), ")")
                } else {
                    let cur = rec(self, slot(2), slot(3))?;
                    cat!("$djs.CC(", cur, ", [", join(&items, ", "), "])")
                }
            }
            _ => return None,
        })
    }
}

/// An iterator record a plain dump wrapped (`$djs.I`/`$djs.L`: `next` closes over `it`
/// and `next`), as the original's.
fn unwrap_record(h: &Heap, it: Value, next: Value) -> (Value, Value) {
    let wrapped = it.as_object().is_some_and(|o| {
        super::origin::own(h, o, "next").is_some_and(|n| {
            n.as_object()
                .is_some_and(|f| h.object_class(f) == ObjectClass::Function)
        }) && h
            .own_keys(o)
            .into_iter()
            .any(|k| h.atom_symbol(k).is_none() && atom_text(h, k) == "return")
    });
    if wrapped && let Some(r) = super::origin::unwrap_loop(h, it) {
        return r;
    }
    (it, next)
}

/// A helper's callback and counter, through a plain dump's `$djs.M(f, k)` (its arrow
/// closes over `$djs_f` and `$djs_k`): the user's function and the counter it sees next.
fn unwrap_callback(h: &Heap, f: Value, k: Value) -> Option<(Value, i64)> {
    let k = k.as_number().unwrap_or(0.0) as i64;
    if let Some(fo) = f.as_object()
        && h.object_class(fo) == ObjectClass::Function
        && let Some(of) = captured_value(h, fo, "$djs_f")
        && let Some(ok) = captured_value(h, fo, "$djs_k")
    {
        return Some((of, k + ok.as_number()? as i64));
    }
    Some((f, k))
}

/// A concat item's `[method, iterable]` through a plain dump's `$djs.CI(m, o)` (its
/// method closes over `$djs_m` and `$djs_o`).
fn unwrap_concat_item(h: &Heap, m: Value, it: Value) -> (Value, Value) {
    if let Some(fo) = m.as_object()
        && h.object_class(fo) == ObjectClass::Function
        && let Some(om) = captured_value(h, fo, "$djs_m")
        && let Some(oi) = captured_value(h, fo, "$djs_o")
    {
        return (om, oi);
    }
    (m, it)
}
