//! Origin tracking (D2870, docs/plain-js-dumps.md §2.3): an activation of a plain dump's
//! continuation `K` is read as an activation of the function `K` stands for.
//!
//! A plain dump writes every suspended activation as a continuation `K` with its origin
//! `K.$djs = [f, at, r, h]` (`$djs.k(K, f, at, r, h)`). Once the dump is loaded and runs, the frames are `K`'s.
//! Dumped again as they are, the next continuation would be written from `K`'s text,
//! which holds the previous dump's prefix: chains would grow with every link. So the
//! exporter reads such a frame as one of `f` at the site `K`'s site stands for, through
//! `K`'s layout, which it makes again from `f`'s site `at`, the tape `r` and the holes `h`
//! (D2913, dump_cont_canon.rs): `K`'s site maps to `f`'s through the stretches `K` copied
//! from `f` (`segs`) or `K`'s own constructs (`scaf`: the site itself, loop scaffolds, a
//! finally's rest); `K`'s bindings are `f`'s by name (`K` declares them as `f` does,
//! nested the same way), the temporaries of `f`'s rewritten statement are `K`'s
//! constants or literals (`cmap`), a loop scaffold's iterable is the saved iterator
//! record again, and `K`'s own `$djs_*` locals are dropped, and with them what only they
//! held. The frame is rebuilt in `f`'s layout, so the rest of the exporter sees an
//! activation of `f`, generated from `f`'s text again.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::compiler::{Holds, NamedSlot, Place, Site, SiteKind, Syntax};
use crate::interp::run::suspend;

/// `K`'s layout, made again from its record.
struct Record {
    segs: Vec<u32>,
    scaf: Vec<Scaffold>,
    /// A temporary of `f`'s rewritten statement: `K`'s binding or a literal.
    cmap: Vec<(String, CVal)>,
}

/// A construct `K` made: kind, offset in `f`, offset in `K`, and what else it records:
/// a finally's completion (its kind, the binding of `K` that holds its value), a `for
/// await`'s bindings of its iterator and `next` (`[0, it, next]`), the origin's site name
/// of a site that is not one of `K`'s kind (a close awaited by K's own `await`).
type Scaffold = (String, u32, u32, Option<(i32, String)>, Extra);

#[derive(Default)]
struct Extra {
    next: Option<String>,
    site: Option<String>,
    /// A temporary of the rewritten statement that K keeps in a construct of its own
    /// (`["tmp", fo, ko, "call.arg@424", "array.element"]`): the origin's slot and K's
    /// construct at `ko`.
    tmp: Option<(String, String)>,
}

enum CVal {
    Binding(String),
    Literal(Value),
}

/// The own data property `name` of `o`.
pub(super) fn own(h: &Heap, o: Obj, name: &str) -> Option<Value> {
    h.own_keys(o)
        .into_iter()
        .find(|&k| h.atom_symbol(k).is_none() && atom_text(h, k) == name)
        .and_then(|k| h.get_own_property(o, k))
        .and_then(|p| match p {
            Property::Data { value, .. } => Some(value),
            _ => None,
        })
}

/// The elements of a dense array.
pub(super) fn elements(h: &Heap, v: Value) -> Option<Vec<Value>> {
    let o = v.as_object()?;
    if h.object_class(o) != ObjectClass::Array {
        return None;
    }
    let len = h.array_length(o);
    let el: Ref = h.arena().get(o.offset(), ObjectCell::ELEMENTS);
    let el = el.get();
    Some(
        (0..len)
            .map(|i| el.map_or(Value::UNDEFINED, |el| h.value_array_get(el, i)))
            .collect(),
    )
}

fn number(v: Value) -> Option<u32> {
    let n = v.as_number()?;
    (n >= 0.0 && n <= u32::MAX as f64 && (n as u32) as f64 == n).then_some(n as u32)
}

fn string(h: &Heap, v: Value) -> Option<String> {
    v.as_string().map(|s| text_of(h, s))
}

/// A list of numbers.
fn numbers(h: &Heap, v: Value) -> Option<Vec<u32>> {
    elements(h, v)?.into_iter().map(number).collect()
}

/// An item of a scaffold record's JSON: a number or a string (names without quotes,
/// commas or escapes, D2913).
enum J {
    N(f64),
    S(String),
}

fn items(json: &str) -> Option<Vec<J>> {
    let json = json.strip_prefix(',').unwrap_or(json);
    if json.is_empty() {
        return Some(Vec::new());
    }
    json.split(',')
        .map(
            |x| match x.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
                Some(s) => Some(J::S(String::from(s))),
                None => x.parse().ok().map(J::N),
            },
        )
        .collect()
}

/// The value of a literal a continuation wrote in place of a temporary (D2913).
fn literal_value(t: &str) -> Option<Value> {
    Some(match t {
        "undefined" => Value::UNDEFINED,
        "null" => Value::NULL,
        "true" => Value::TRUE,
        "false" => Value::FALSE,
        _ => {
            let n: f64 = t.parse().ok()?;
            let i = n as i32;
            if f64::from(i) == n && !(n == 0.0 && n.is_sign_negative()) {
                Value::int(i)
            } else {
                Value::number(n)
            }
        }
    })
}

/// The layout of a continuation made again (D2913), as origin tracking reads it.
fn record_of(h: &Heap, k: Obj, l: cont::canon::Layout) -> Option<Record> {
    // A string literal is one of K's constants, or an atom where K uses it as a key
    // (its text has no escapes, `literal`).
    let consts = template::consts(h, h.closure_template(k));
    let string = |t: &str| {
        let t = t.strip_prefix('"')?.strip_suffix('"')?;
        consts
            .iter()
            .copied()
            .find(|v| v.as_string().is_some_and(|s| text_of(h, s) == t))
            .or_else(|| {
                let a = h.find_atom_str(t)?;
                h.atom_str_value(a).map(Value::string)
            })
    };
    let mut scaf = Vec::new();
    for (kind, fo, ko, json) in l.scaf {
        let e = items(&json)?;
        let s = |j: &J| match j {
            J::S(s) => Some(s.clone()),
            J::N(_) => None,
        };
        let mut extra = Extra::default();
        let fin = match (kind, e.first(), e.get(1)) {
            ("tmp", Some(f), Some(k)) => {
                extra.tmp = Some((s(f)?, s(k)?));
                None
            }
            ("site", Some(n), None) => {
                extra.site = Some(s(n)?);
                None
            }
            (_, Some(J::N(k)), Some(b)) => Some((*k as i32, s(b)?)),
            _ => None,
        };
        if let Some(n) = e.get(2) {
            extra.next = Some(s(n)?);
        }
        scaf.push((String::from(kind), fo, ko, fin, extra));
    }
    let mut cmap = Vec::new();
    for (slot, v, lit) in l.cmap {
        let c = if lit {
            CVal::Literal(literal_value(&v).or_else(|| string(&v))?)
        } else {
            CVal::Binding(v)
        };
        cmap.push_out((slot, c));
    }
    Some(Record {
        segs: l.segs,
        scaf,
        cmap,
    })
}

/// Whether K, stopped at `kpos`, is in text it copied whole from the construct at `foff`
/// on, a copy other than the scaffold at K offset `k` (a later iteration of an outer
/// loop runs the construct as the original's text): the scaffold's constants are stale
/// there (0215).
fn in_copy(segs: &[u32], kpos: u32, foff: u32, k: u32) -> bool {
    segs.chunks(3).any(|s| {
        s.len() == 3
            && (s[0]..s[0] + s[2]).contains(&kpos)
            && (s[1]..s[1] + s[2]).contains(&foff)
            && s[0] + (foff - s[1]) != k
    })
}

/// Offset `k` of `K`'s text in `f`'s, through the stretches `K` copied.
fn to_f(segs: &[u32], k: u32) -> Option<u32> {
    segs.chunks(3)
        .find(|s| s.len() == 3 && s[0] <= k && k < s[0] + s[2])
        .map(|s| s[1] + (k - s[0]))
}

/// The slot name's construct and offset (`call.arg@41`).
fn split(name: &str) -> Option<(&str, u32)> {
    let (c, off) = name.rsplit_once('@')?;
    Some((c, off.parse().ok()?))
}

/// The `$dump.L` wrapper of a loop scaffold: the iterator record it continues.
pub(super) fn unwrap_loop(h: &Heap, r: Value) -> Option<(Value, Value)> {
    let next = own(h, r.as_object()?, "next")?.as_object()?;
    if h.object_class(next) != ObjectClass::Function {
        return None;
    }
    Some((
        captured_value(h, next, "it")?,
        captured_value(h, next, "next")?,
    ))
}

impl Dumper<'_> {
    /// Activation `s` of a continuation, as an activation of its origin: the frame in
    /// the origin's layout, its site and syntax. `Ok(None)`: not a continuation.
    pub(super) fn origin_frame(
        &mut self,
        s: &suspend::Suspended,
        ksite: &Site,
    ) -> Result<Option<(suspend::Suspended, Site, Arc<Syntax>)>, String> {
        let h = self.h;
        let Some(k) = s.frame.first().and_then(|v| v.as_object()) else {
            return Ok(None);
        };
        let no = |why: &str| Err(cat!("a continuation of an earlier dump: ", why));
        // `K.$djs`: `[f, at, r, h]`, its origin, the site and what makes K's layout again
        // (D2913, D2915).
        let Some(r) = own(h, k, "$djs").and_then(|r| elements(h, r)) else {
            return Ok(None);
        };
        let Some(f) = r
            .first()
            .and_then(|f| f.as_object())
            .filter(|&f| h.object_class(f) == ObjectClass::Function)
        else {
            return Ok(None);
        };
        let field = |i: usize| r.get(i).copied().unwrap_or(Value::UNDEFINED);
        let at = match field(1) {
            v if v.is_undefined() => Some(String::from("await#1")),
            v => match v.as_number() {
                Some(n) => number(v).map(|_| cat!("await#", n as u32)),
                None => string(h, v),
            },
        };
        // (A tape of yes/no decisions is a number, its bits the decisions.)
        let tape = match field(2) {
            v if v.is_undefined() => Some(Vec::new()),
            v => match v.as_number() {
                Some(n) if (0.0..9007199254740992.0).contains(&n) && (n as u64) as f64 == n => {
                    let mut n = n as u64;
                    let mut v = Vec::new();
                    while n != 0 {
                        v.push_out((n & 1) as u32);
                        n >>= 1;
                    }
                    Some(v)
                }
                Some(_) => None,
                None => numbers(h, v),
            },
        };
        let holes = match field(3) {
            v if v.is_undefined() => Some(Vec::new()),
            v => numbers(h, v),
        };
        let (Some(at), Some(tape), Some(holes)) = (at, tape, holes) else {
            return no("its record is not one");
        };
        let t = h.closure_template(f);
        let info = template::debug_info(h, t);
        let (a, b) = (info.src_start, info.src_end);
        let mut params = usize::from(h.arena().get::<u16>(t, FuncTemplate::PARAMS));
        let fkind = func_flags::kind(u32::from(h.arena().get::<u16>(t, FuncTemplate::FLAGS)));
        let Some(ftext) = self.sources.fn_text(h, f) else {
            return no("its origin's text is gone");
        };
        // f's maps: its template's script compiled again.
        let m = if fkind == func_flags::KIND_ASYNC_BODY {
            // A codemode body (the host's script, D2882): K was written from the async
            // function the dump writes it as, whose map this is (as for its frames).
            let bits = info.script.get().map_or(0, |s| {
                h.arena().get::<u32>(s, crate::heap::cells::Script::OPTIONS)
            });
            let text = cat!("(", ftext, ")");
            let len = text.len() as u32;
            let Some(maps) = self.maps_of(&text, "", bits & !3) else {
                return no("its origin's script cannot be compiled again");
            };
            let Some(m) = maps.iter().find(|m| m.start == 1 && m.end == len - 1) else {
                return no("its origin has no suspension map");
            };
            m.clone()
        } else {
            let Some(maps) = self.script_maps(t) else {
                return no("its origin's script cannot be compiled again");
            };
            let Some(m) = maps.iter().find(|m| m.start == a && m.end == b) else {
                return no("its origin has no suspension map");
            };
            m.clone()
        };
        let syntax = Arc::new(m.syntax.clone());
        // K's layout, made again from f at the site K was written for.
        let Some(site) = m.sites.iter().find(|x| x.name == at) else {
            return no("its origin has no site of its record");
        };
        let peers = match site.kind {
            SiteKind::ForAwaitClose | SiteKind::YieldStarAwait => m
                .sites
                .iter()
                .filter(|x| x.kind == site.kind && x.pos == site.pos)
                .map(|x| x.name.clone())
                .collect(),
            _ => Vec::new(),
        };
        // (The text K was written from: a class's own functions have the class's text
        // with the other members blanked, as the dump writes them.)
        let otext = match self.class_texts.iter().find(|c| c.0 == ftext) {
            Some((_, blanked)) => blanked.clone(),
            None => ftext.clone(),
        };
        let act = cont::canon::canonical(site, syntax.clone(), otext, fkind, s.kind, peers);
        let Some(ktext) = self.sources.fn_text(h, k) else {
            return no("its text is gone");
        };
        let ts = info.script.get().is_some_and(|s| {
            h.arena().get::<u32>(s, crate::heap::cells::Script::OPTIONS)
                & crate::compiler::CompileOptions::TYPESCRIPT_BIT
                != 0
        });
        let rec = match cont::canon::layout_of(&act, &tape, &holes, &ktext, ts) {
            Ok(l) => record_of(h, k, l),
            Err(e) => return Err(cat!("a continuation of an earlier dump: ", e)),
        };
        let Some(rec) = rec else {
            return no("its layout is not one");
        };
        // K's site in f's text: K's own (the site it was written for) or a copied one.
        let site_rec = rec.scaf.iter().find(|r| r.0 == "site" && r.2 == ksite.pos);
        let at_site = site_rec.map(|r| r.1);
        let site_name = site_rec.and_then(|r| r.4.site.clone());
        // (A site at the end of a copied stretch: a `using` scope's disposal after its
        // `}`.)
        let at_end = || {
            rec.segs
                .chunks(3)
                .find(|s| s.len() == 3 && s[0] + s[2] == ksite.pos)
                .map(|s| s[1] + s[2])
        };
        let Some(fpos) = at_site
            .or_else(|| to_f(&rec.segs, ksite.pos))
            .or_else(|| (ksite.kind == SiteKind::AwaitUsing).then(at_end).flatten())
        else {
            return no("its site is not one of its origin's");
        };
        let kind = fkind;
        // Several sites of a kind at one construct (a `for await`'s closes): the one
        // with K's place among K's at its copy.
        let nth = if site_name.is_none() {
            self.nth_site_at(k, ksite)
        } else {
            0
        };
        let fsite = m
            .sites
            .iter()
            .filter(|x| match &site_name {
                Some(n) => x.name == *n && x.pos == fpos,
                None => x.pos == fpos && same_kind(x.kind, ksite.kind),
            })
            .nth(nth)
            .cloned();
        let Some(mut fsite) = fsite else {
            return no("its origin has no site there");
        };
        if kind == func_flags::KIND_ASYNC_BODY {
            params = fsite.args.len();
        }
        if let Some(u) = &fsite.unsupported {
            return Err(cat!(
                "an activation at ",
                fsite.name,
                ", which dumps cannot resume yet: ",
                u
            ));
        }

        // K's slots by name.
        let khp = s.hp as usize;
        let klp = khp + crate::interp::frame::HEADER as usize;
        let kv = |p: Place| {
            let i = match p {
                Place::Arg(i) => 2 + usize::from(i),
                Place::Local(i) => klp + usize::from(i),
                Place::Stack(j) => klp + ksite.locals.len() + usize::from(j),
            };
            s.frame.get(i).copied().unwrap_or(Value::UNDEFINED)
        };
        let knamed = ksite.named_slots();
        let kslot = |name: &str| knamed.iter().find(|x| x.name == name).map(|x| kv(x.place));
        // The bindings of a name in scope, outermost first (`name` or `name@scope`).
        let base = |x: &NamedSlot| -> String {
            match x.name.rsplit_once('@') {
                Some((n, _)) if matches!(x.holds, Holds::Binding) => String::from(n),
                _ => x.name.clone(),
            }
        };
        let scope_of = |x: &NamedSlot| -> u32 {
            x.name
                .rsplit_once('@')
                .and_then(|(_, s)| s.parse().ok())
                .unwrap_or(0)
        };
        let bindings = |named: &[NamedSlot], name: &str| -> Vec<NamedSlot> {
            let mut v: Vec<NamedSlot> = named
                .iter()
                .filter(|x| matches!(x.holds, Holds::Binding) && base(x) == name)
                .cloned()
                .collect();
            crate::sort::sort_by(&mut v, |p, q| scope_of(p) < scope_of(q));
            v
        };
        let fnamed = fsite.named_slots();

        // The frame in f's layout.
        let hp = 2 + params;
        let lp = hp + crate::interp::frame::HEADER as usize;
        let mut frame = alloc::vec![Value::UNDEFINED; lp + fsite.locals.len() + fsite.stack.len()];
        frame[0] = Value::object(f);
        frame[1] = s.frame.get(1).copied().unwrap_or(Value::UNDEFINED);
        for i in 0..crate::interp::frame::HEADER as usize {
            frame[hp + i] = s.frame.get(khp + i).copied().unwrap_or(Value::UNDEFINED);
        }
        frame[hp + crate::interp::frame::ARGC as usize] = Value::int(params as i32);
        let fidx = |p: Place| match p {
            Place::Arg(i) => 2 + usize::from(i),
            Place::Local(i) => lp + usize::from(i),
            Place::Stack(j) => lp + fsite.locals.len() + usize::from(j),
        };
        for x in &fnamed {
            let v = match x.holds {
                Holds::Binding => {
                    let name = base(x);
                    let kname = if name == "arguments" {
                        String::from("$djs_arguments")
                    } else {
                        name.clone()
                    };
                    let fs = bindings(&fnamed, &name);
                    let ks = bindings(&knamed, &kname);
                    let at = fs.iter().position(|y| y.name == x.name).unwrap_or(0);
                    if ks.len() != fs.len() {
                        if fs.len() == 1 && ks.is_empty() {
                            // Declared later in K (in its TDZ) or never reached; or left
                            // out of K as dead (D2911): a parameter, which has no TDZ,
                            // is undefined then.
                            if matches!(x.place, Place::Arg(_)) {
                                Value::UNDEFINED
                            } else {
                                Value::UNINITIALIZED
                            }
                        } else {
                            return Err(cat!(
                                "a continuation of an earlier dump: its bindings `",
                                name,
                                "` differ from its origin's"
                            ));
                        }
                    } else {
                        kv(ks[at].place)
                    }
                }
                // (`arguments` is K's binding `$djs_arguments`.)
                Holds::Hidden => match kslot(&x.name).or_else(|| {
                    (x.name == "arguments")
                        .then(|| {
                            bindings(&knamed, "$djs_arguments")
                                .first()
                                .map(|y| kv(y.place))
                        })
                        .flatten()
                }) {
                    Some(v) => v,
                    // A class's hidden bindings (its field initializers, keys, private
                    // methods): dead once its statement ran, which K's prefix holds.
                    None if super::flight::class_hidden(&x.name) => Value::UNDEFINED,
                    // The function's `this` kept in a local: K is called with it.
                    None if x.name == "this" => frame[1],
                    // Its home object kept in a local, where K uses no `super` (the call
                    // was in flight: K resumes it): the function's own (0215).
                    None if x.name == "<home>" => super::flight::home_object(self.h, f)
                        .map_or(Value::UNDEFINED, Value::object),
                    None => {
                        return Err(cat!(
                            "a continuation of an earlier dump: no `",
                            x.name,
                            "` in its frame"
                        ));
                    }
                },
                Holds::Temp => match self.origin_temp(&rec, &knamed, &kv, at_site, ksite.pos, x) {
                    Some(v) => v,
                    None => {
                        return Err(cat!(
                            "a continuation of an earlier dump: its frame has no value for ",
                            x.name
                        ));
                    }
                },
            };
            frame[fidx(x.place)] = v;
        }
        // K's temporaries the origin's frame does not take: K stopped inside its own
        // rewritten statement at another site, whose state K's constructs hold.
        if at_site.is_none() {
            for x in knamed.iter().filter(|x| matches!(x.holds, Holds::Temp)) {
                let Some((_, off)) = split(&x.name) else {
                    continue;
                };
                let scaffold = rec.scaf.iter().any(|r| r.2 == off);
                if !scaffold && to_f(&rec.segs, off).is_none() {
                    return Err(cat!(
                        "a continuation of an earlier dump: it stopped again inside its rewritten statement, its ",
                        x.name,
                        " (0203)"
                    ));
                }
            }
        }
        // A binding shared with closures is a box in K's frame: f's map says so too where
        // K's closures (f's, made again) share it.
        for (i, slot) in fsite.locals.iter().enumerate() {
            let v = frame[lp + i];
            if let Some(b) = fsite.boxed.get_mut(i) {
                *b = matches!(
                    slot,
                    crate::compiler::Slot::Binding { .. } | crate::compiler::Slot::Hidden(_)
                ) && v.as_internal().is_some();
            }
        }
        let out = suspend::Suspended {
            kind: s.kind,
            state: s.state,
            frame,
            pc: fsite.resume_pc,
            hp: hp as u32,
            promise: s.promise,
            queue: s.queue.clone(),
        };
        Ok(Some((out, fsite, syntax)))
    }

    /// The place of site `ksite` among the sites of its kind at its offset in `k`'s map.
    fn nth_site_at(&mut self, k: Obj, ksite: &Site) -> usize {
        let h = self.h;
        let t = h.closure_template(k);
        let info = template::debug_info(h, t);
        let Some(maps) = self.script_maps(t) else {
            return 0;
        };
        maps.iter()
            .find(|m| m.start == info.src_start && m.end == info.src_end)
            .and_then(|m| {
                m.sites
                    .iter()
                    .filter(|x| x.pos == ksite.pos && x.kind == ksite.kind)
                    .position(|x| x.name == ksite.name)
            })
            .unwrap_or(0)
    }

    /// The value of the origin's temporary `x`; a completion's kind that names a
    /// statement (a `break` or `continue`, D2733) by its offset in `K` names it in `f`.
    fn origin_temp(
        &self,
        rec: &Record,
        knamed: &[NamedSlot],
        kv: &dyn Fn(Place) -> Value,
        at_site: Option<u32>,
        kpos: u32,
        x: &NamedSlot,
    ) -> Option<Value> {
        let mut v = self.origin_temp_k(rec, knamed, kv, at_site, kpos, x)?;
        // A constant of K that closures capture (a pattern's source, read by K's
        // getters) is a box in K's frame; the original's temporary is its value.
        if let Some(b) = v.as_internal()
            && self.h.cell_kind(b) == crate::heap::cells::BoxCell::KIND
        {
            v = self.h.arena().get(b, crate::heap::cells::BoxCell::VALUE);
        }
        let (c, foff) = split(&x.name)?;
        // (Unless K stopped in a copy of the statement: its scaffold's constant is stale
        // there, the frame's completion is K's, 0215.)
        let from_k = !rec.scaf.iter().any(|r| {
            r.0 == "finally"
                && r.1 == foff
                && c == "completion.kind"
                && !in_copy(&rec.segs, kpos, foff, r.2)
        });
        match v.as_int() {
            Some(code) if c == "completion.kind" && code >= 3 && from_k => {
                let ko = ((code - 3) / 2) as u32;
                let fo = rec
                    .scaf
                    .iter()
                    .find(|r| r.2 == ko && r.0 != "site")
                    .map(|r| r.1)
                    .or_else(|| to_f(&rec.segs, ko))?;
                Some(Value::int(3 + 2 * fo as i32 + (code - 3) % 2))
            }
            _ => Some(v),
        }
    }

    fn origin_temp_k(
        &self,
        rec: &Record,
        knamed: &[NamedSlot],
        kv: &dyn Fn(Place) -> Value,
        at_site: Option<u32>,
        kpos: u32,
        x: &NamedSlot,
    ) -> Option<Value> {
        let h = self.h;
        let (c, foff) = split(&x.name)?;
        // (The construct is a copy K stopped in: the scaffold's constants are stale.)
        let in_copy = |k: u32| in_copy(&rec.segs, kpos, foff, k);
        let kslot = |name: &str| knamed.iter().find(|y| y.name == name).map(|y| kv(y.place));
        // A temporary of the rewritten statement evaluated before the site: K's constant.
        let from_cmap = || {
            rec.cmap
                .iter()
                .find(|e| e.0 == x.name)
                .and_then(|(_, cv)| match cv {
                    CVal::Binding(b) => kslot(b),
                    CVal::Literal(v) => Some(*v),
                })
        };
        if at_site.is_some()
            && let Some(v) = from_cmap()
        {
            return Some(v);
        }
        // A scaffold's: the loop's saved iterator record, a finally's completion.
        for r in rec.scaf.iter().filter(|r| r.1 == foff && !in_copy(r.2)) {
            match (r.0.as_str(), c) {
                ("for-of", "for-of.iterator") | ("for-of", "for-of.next") => {
                    // (Only while K is inside its scaffold: a later iteration of an
                    // outer loop runs the loop as copied text.)
                    if let Some(w) = kslot(&cat!("for-of.iterator@", r.2)) {
                        // (Over the saved iterator itself, D2917: K's loop's own `next`.)
                        let (it, next) = unwrap_loop(h, w).unwrap_or_else(|| {
                            (
                                w,
                                kslot(&cat!("for-of.next@", r.2)).unwrap_or(Value::UNDEFINED),
                            )
                        });
                        return Some(if c == "for-of.iterator" { it } else { next });
                    }
                    // K in the rest of the iteration before its loop (D2908): the record's
                    // constants (no `next`: the iterator's own).
                    if let Some((_, ib)) = &r.3
                        && let Some(it) = kslot(ib)
                    {
                        return Some(if c == "for-of.iterator" {
                            it
                        } else {
                            r.4.next
                                .as_ref()
                                .and_then(|nb| kslot(nb))
                                .unwrap_or(Value::UNDEFINED)
                        });
                    }
                }
                ("pattern", "pattern.iterator") | ("pattern", "pattern.next") => {
                    // K's array pattern over the record (`undefined` once done).
                    if let Some(w) = kslot(&cat!("pattern.iterator@", r.2)) {
                        if w.is_undefined() {
                            return Some(w);
                        }
                        let (it, next) = unwrap_loop(h, w)?;
                        return Some(if c == "pattern.iterator" { it } else { next });
                    }
                }
                ("for-in", "for-in.iterator") => {
                    // The dump's key iterator (`$dump.forIn`), in K's loop over it.
                    if let Some(w) = kslot(&cat!("for-of.iterator@", r.2)) {
                        return unwrap_loop(h, w).map(|x| x.0);
                    }
                    // (Before the loop, D2908: K's constant.)
                    if let Some((_, ib)) = &r.3
                        && let Some(it) = kslot(ib)
                    {
                        return Some(it);
                    }
                }
                ("for-await", "for-await.iterator") | ("for-await", "for-await.next") => {
                    // Its record's constants, while K is inside the scaffold.
                    let b = if c == "for-await.iterator" {
                        r.3.as_ref().map(|x| x.1.clone())
                    } else {
                        r.4.next.clone()
                    };
                    if let Some(v) = b.and_then(|b| kslot(&b)) {
                        return Some(v);
                    }
                }
                ("using", "using.record")
                | ("using", "completion.kind")
                | ("using", "completion.value") => {
                    // K's scope that adds the original's resources again (D2876).
                    if let Some(v) = kslot(&cat!(c, "@", r.2)) {
                        return Some(v);
                    }
                }
                ("try", "completion.kind") | ("try", "completion.value") => {
                    // A try statement K wrote around the site: its finally's completion.
                    if let Some(v) = kslot(&cat!(c, "@", r.2)) {
                        return Some(v);
                    }
                }
                // While K is in its finally scaffold (its completion's constant is in
                // scope); a later iteration runs the copied text.
                ("finally", "completion.kind") | ("finally", "completion.value") => {
                    if let Some((k, b)) = r.3.as_ref()
                        && let Some(v) = kslot(b)
                    {
                        return Some(if c == "completion.kind" {
                            Value::int(*k)
                        } else {
                            v
                        });
                    }
                }
                _ => {}
            }
        }
        // A construct of the text K copied: the same slot at K's offset. (First: in a
        // later iteration of a loop K runs the original's text again.)
        let copied = knamed.iter().find_map(|y| {
            let (yc, ko) = split(&y.name)?;
            (yc == c && to_f(&rec.segs, ko) == Some(foff)).then(|| kv(y.place))
        });
        if copied.is_some() {
            return copied;
        }
        // K stopped again inside its rewritten statement: what it evaluated after the
        // site is in K's own constructs, what came before in its constants.
        for r in rec.scaf.iter().filter(|r| r.0 == "tmp") {
            if let Some((f, kc)) = &r.4.tmp
                && *f == x.name
                && let Some(v) = kslot(&cat!(kc, "@", r.2))
            {
                return Some(v);
            }
        }
        from_cmap()
    }
}

/// Whether sites of kinds `a` (the origin's) and `b` (K's) are the same suspension.
fn same_kind(a: SiteKind, b: SiteKind) -> bool {
    a == b
}
