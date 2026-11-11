//! RegExp leaf intrinsics over `crate::regexp` (D1332): creating RegExp objects,
//! reading their internal slots, the escaped source, and building match results. The
//! matcher itself runs in `interp/run/json_regexp.rs`, which owns fuel and suspension.
//!
//! A RegExp object (`ObjectClass::RegExp`) holds its compiled program in a U32Array cell
//! (pointer-free words, so the heap image carries it as is), its source string and its
//! flag bits. `lastIndex` is an ordinary own data property (writable only).
use crate::PushOut;

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::{
    Atom, Heap, HeapError, ObjectClass, PropFlags, Property, PropertyDescriptor, StrView, atoms,
};
use crate::interp::conv::{Fault, type_error};
use crate::interp::{ErrorKind, Intrinsic, realm};
use crate::regexp::{self, Caps, ExecLimits, ExecState, Flags, Outcome, Prog, Program};
use crate::value::{Obj, Str, Value};

pub(crate) const SLOT_PROGRAM: u32 = 0;
pub(crate) const SLOT_SOURCE: u32 = 1;
pub(crate) const SLOT_FLAGS: u32 = 2;

/// RegExp String Iterator slots and flag bits.
pub(crate) const IT_REGEXP: u32 = 0;
pub(crate) const IT_STRING: u32 = 1;
pub(crate) const IT_FLAGS: u32 = 2;
pub(crate) const IT_GLOBAL: i32 = 1;
pub(crate) const IT_UNICODE: i32 = 2;
pub(crate) const IT_DONE: i32 = 4;

/// The flag letters in `flags` order (hasIndices first, as in RegExp.prototype.flags).
pub(crate) const FLAG_CHARS: [(u8, Flags); 8] = [
    (b'd', Flags::HAS_INDICES),
    (b'g', Flags::GLOBAL),
    (b'i', Flags::IGNORE_CASE),
    (b'm', Flags::MULTILINE),
    (b's', Flags::DOT_ALL),
    (b'u', Flags::UNICODE),
    (b'v', Flags::UNICODE_SETS),
    (b'y', Flags::STICKY),
];

fn syntax_error<T>(msg: &str) -> Result<T, Fault> {
    Err(Fault::error(ErrorKind::SyntaxError, String::from(msg)))
}

/// The RegExp object `v`, if it is one.
pub(crate) fn as_regexp(heap: &Heap, v: Value) -> Option<Obj> {
    v.as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::RegExp)
}

/// Compiles `pattern` with `flags` (both as written in the source or given to the
/// constructor), with QuickJS's SyntaxError messages.
pub(crate) fn compile(pattern: &[u16], flags: &[u16]) -> Result<Program, regexp::SyntaxError> {
    let f = Flags::parse(flags)?;
    regexp::compile(pattern, f)
}

/// A U32Array cell holding `prog`'s words.
pub(crate) fn program_cell(heap: &mut Heap, prog: &Program) -> Result<u32, HeapError> {
    let words = prog.words();
    let cell = heap.alloc_u32_array(words.len() as u32)?;
    for (i, &w) in words.iter().enumerate() {
        heap.u32_array_set(cell, i as u32, w);
    }
    Ok(cell)
}

/// RegExpCreate: a new RegExp object with prototype `proto` (%RegExp.prototype% if
/// `None`), `lastIndex` 0. `cell` is an already compiled program of `pattern` and
/// `flags` (a regex literal's, D1502).
pub(crate) fn create(
    heap: &mut Heap,
    proto: Option<Obj>,
    pattern: Str,
    flags: Str,
    cell: Option<u32>,
) -> Result<Obj, Fault> {
    // Flat strings: the cache compares them by their bytes, and the source slot keeps the
    // flat one.
    let (pattern, flags) = match cell {
        Some(_) => (pattern, flags),
        None => (heap.contiguous(pattern)?, heap.contiguous(flags)?),
    };
    let cell = match cell {
        Some(c) => c,
        None => match cached(heap, pattern, flags) {
            Some(c) => c,
            None => {
                let p = units(heap, pattern)?;
                let f = units(heap, flags)?;
                let prog = match compile(&p, &f) {
                    Ok(prog) => prog,
                    Err(e) => return syntax_error(e.message()),
                };
                let c = program_cell(heap, &prog)?;
                cache(heap, pattern, flags, c)?;
                c
            }
        },
    };
    // The flag bits are the program's header word (`compiler::H_FLAGS`).
    let bits = heap.u32_array_get(cell, 0);
    let proto = proto.unwrap_or_else(|| realm::intrinsic_obj(heap, Intrinsic::RegExpProto));
    let o = heap.alloc_object(ObjectClass::RegExp, Some(proto), 3)?;
    heap.set_object_slot(o, SLOT_PROGRAM, Value::internal(cell));
    heap.set_object_slot(o, SLOT_SOURCE, Value::string(pattern));
    heap.set_object_slot(o, SLOT_FLAGS, Value::int(bits as i32));
    let key = atoms::LAST_INDEX;
    let d = PropertyDescriptor::data(Value::int(0), PropFlags::WRITABLE);
    heap.define_own_property(o, key, &d)?;
    Ok(o)
}

/// Entries of the compile cache (D3138).
const CACHE_ENTRIES: u32 = 8;
/// Longer sources and bigger programs (4 KiB) are not cached: the cache keeps at most
/// about 36 KiB alive, typically 3-4 KiB.
const CACHE_MAX_LEN: u32 = 256;
const CACHE_MAX_WORDS: u32 = 1024;

/// The program of `pattern` with `flags` from the compile cache (moved to the front), if
/// it is there.
fn cached(heap: &mut Heap, pattern: Str, flags: Str) -> Option<u32> {
    let t = realm::intrinsic(heap, Intrinsic::RegExpCache).as_internal()?;
    let len = heap.str_len(pattern);
    let hit = (0..CACHE_ENTRIES).find(|&i| {
        heap.value_array_get(t, 3 * i)
            .as_string()
            .is_some_and(|p| heap.str_len(p) == len && heap.str_eq(p, pattern))
            && heap
                .value_array_get(t, 3 * i + 1)
                .as_string()
                .is_some_and(|f| heap.str_eq(f, flags))
    })?;
    let cell = heap.value_array_get(t, 3 * hit + 2);
    let entry = [
        heap.value_array_get(t, 3 * hit),
        heap.value_array_get(t, 3 * hit + 1),
        cell,
    ];
    shift_down(heap, t, hit);
    for (k, v) in entry.into_iter().enumerate() {
        heap.value_array_set(t, k as u32, v);
    }
    cell.as_internal()
}

/// Entries `0..to` one place down (entry `to` is overwritten).
fn shift_down(heap: &mut Heap, t: u32, to: u32) {
    use crate::heap::cells::ValueArray;
    let at = ValueArray::VALUES.at(t, 0);
    heap.arena_mut()
        .copy_within(at, 3 * to * 8, ValueArray::VALUES.at(t, 3));
}

/// Puts a just compiled program at the front of the compile cache.
fn cache(heap: &mut Heap, pattern: Str, flags: Str, cell: u32) -> Result<(), HeapError> {
    if heap.str_len(pattern) > CACHE_MAX_LEN || heap.u32_array_len(cell) > CACHE_MAX_WORDS {
        return Ok(());
    }
    let t = match realm::intrinsic(heap, Intrinsic::RegExpCache).as_internal() {
        Some(t) => t,
        None => {
            let n = 3 * CACHE_ENTRIES;
            let t = heap.alloc_value_array(n)?;
            for k in 0..n {
                heap.value_array_set(t, k, Value::UNDEFINED);
            }
            heap.arena_mut()
                .set(t, crate::heap::cells::ValueArray::LEN, n);
            realm::set_intrinsic(heap, Intrinsic::RegExpCache, Value::internal(t));
            t
        }
    };
    shift_down(heap, t, CACHE_ENTRIES - 1);
    heap.value_array_set(t, 0, Value::string(pattern));
    heap.value_array_set(t, 1, Value::string(flags));
    heap.value_array_set(t, 2, Value::internal(cell));
    Ok(())
}

/// RegExpCreate of the source of the RegExp `like` with `flags`: with its flags, the
/// new RegExp shares its program (programs are immutable), else it compiles.
pub(crate) fn create_like(
    heap: &mut Heap,
    proto: Option<Obj>,
    like: Obj,
    flags: Str,
) -> Result<Obj, Fault> {
    let pattern = source(heap, like);
    let same = Flags::parse(&units(heap, flags)?).is_ok_and(|f| f == self::flags(heap, like));
    let cell = same.then(|| {
        heap.object_slot(like, SLOT_PROGRAM)
            .as_internal()
            .expect("RegExp program")
    });
    create(heap, proto, pattern, flags, cell)
}

/// The code units of `s`, read from its flat form (a rope's units one by one cost more
/// than compiling a short pattern).
fn units(heap: &mut Heap, s: Str) -> Result<Vec<u16>, HeapError> {
    let flat = heap.contiguous(s)?;
    Ok(match heap.str_view(flat).expect("flat string") {
        StrView::Latin1(b) => b.iter().map(|&c| c as u16).collect(),
        _ => heap.str_to_utf16(flat),
    })
}

/// The compiled program of a RegExp object, read in place from its cell (D3135).
pub(crate) fn program(heap: &Heap, o: Obj) -> Prog<'_> {
    let cell = heap
        .object_slot(o, SLOT_PROGRAM)
        .as_internal()
        .expect("RegExp program");
    let n = heap.u32_array_len(cell);
    let bytes = heap
        .arena()
        .slice(crate::heap::cells::U32Array::DATA.at(cell, 0), 4 * n);
    Prog::new(bytes.as_chunks::<4>().0)
}

pub(crate) fn flags(heap: &Heap, o: Obj) -> Flags {
    Flags::from_bits(heap.object_slot(o, SLOT_FLAGS).as_int().unwrap_or(0) as u16)
}

pub(crate) fn source(heap: &Heap, o: Obj) -> Str {
    heap.object_slot(o, SLOT_SOURCE)
        .as_string()
        .expect("RegExp source")
}

/// The flags as a string, in canonical order ([[OriginalFlags]]).
pub(crate) fn flags_string(heap: &mut Heap, o: Obj) -> Result<Str, Fault> {
    let f = flags(heap, o);
    let s: Vec<u8> = FLAG_CHARS
        .iter()
        .filter(|(_, bit)| f.contains(*bit))
        .map(|(c, _)| *c)
        .collect();
    Ok(heap.alloc_latin1(&s)?)
}

/// Whether the RegExp has the flag letter `c`.
pub(crate) fn has_flag(heap: &Heap, o: Obj, c: u16) -> bool {
    let f = flags(heap, o);
    FLAG_CHARS
        .iter()
        .any(|(ch, bit)| *ch as u16 == c && f.contains(*bit))
}

/// EscapeRegExpPattern as QuickJS does it: `/` outside classes and line terminators
/// escaped, `(?:)` for the empty pattern.
pub(crate) fn escaped_source(heap: &mut Heap, o: Obj) -> Result<Str, Fault> {
    let src = source(heap, o);
    // Most sources need no escape: then the source itself, without copying it.
    if let Some(v) = heap.str_view(src)
        && !v.is_empty()
        && (0..v.len()).all(|i| !matches!(v.get(i), 0x0A | 0x0D | 0x2F))
    {
        return Ok(src);
    }
    let units = heap.str_to_utf16(src);
    if units.is_empty() {
        return Ok(heap.alloc_str("(?:)")?);
    }
    let mut out = crate::buf::vec_with_capacity(units.len() + 4);
    let mut in_class = false;
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        i += 1;
        match c {
            0x5C => {
                out.push_out(c);
                if i < units.len() {
                    out.push_out(units[i]);
                    i += 1;
                }
            }
            0x5D => {
                in_class = false;
                out.push_out(c);
            }
            0x5B => {
                out.push_out(c);
                if !in_class {
                    in_class = true;
                    // `[]` and `[^]`-style leading `]` stays literal.
                    if i < units.len() && units[i] == 0x5D {
                        out.push_out(units[i]);
                        i += 1;
                    }
                }
            }
            0x0A => out.extend_from_slice(&[0x5C, b'n' as u16]),
            0x0D => out.extend_from_slice(&[0x5C, b'r' as u16]),
            0x2F if !in_class => out.extend_from_slice(&[0x5C, 0x2F]),
            _ => out.push_out(c),
        }
    }
    if out == units {
        return Ok(src);
    }
    Ok(alloc_units(heap, &out)?)
}

fn alloc_units(heap: &mut Heap, units: &[u16]) -> Result<Str, HeapError> {
    if units.iter().all(|&u| u <= 0xFF) {
        let bytes: Vec<u8> = units.iter().map(|&u| u as u8).collect();
        heap.alloc_latin1(&bytes)
    } else {
        heap.alloc_utf16(units)
    }
}

/// Sets `lastIndex` (an own data property that is never an accessor: it is
/// non-configurable), with Set's TypeError when it is read-only.
pub(crate) fn set_last_index(heap: &mut Heap, o: Obj, v: Value) -> Result<(), Fault> {
    let key = atoms::LAST_INDEX;
    match heap.get_own_property(o, key) {
        Some(Property::Data { flags, .. }) if !flags.writable() => {
            type_error("'lastIndex' is read-only")
        }
        _ => {
            let d = PropertyDescriptor {
                value: Some(v),
                ..Default::default()
            };
            heap.define_own_property(o, key, &d)?;
            Ok(())
        }
    }
}

/// The substring `s[start..end)` as a value.
pub(crate) fn substring_value(
    heap: &mut Heap,
    s: Str,
    start: usize,
    end: usize,
) -> Result<Value, Fault> {
    substring(heap, s, start, end)
}

/// The substring `s[start..end)` of a flat string view.
fn substring(heap: &mut Heap, s: Str, start: usize, end: usize) -> Result<Value, Fault> {
    Ok(Value::string(super::string::substring(
        heap,
        s,
        start as u32,
        end as u32,
    )?))
}

/// Defines `key: v` on a groups object, except that an undefined value never replaces
/// a defined one (duplicate named groups: only one of them participates).
fn define_group(heap: &mut Heap, groups: Obj, name: &[u16], v: Value) -> Result<(), Fault> {
    let key = heap.atom_utf16(name)?;
    if v.is_undefined() && heap.has_own_property(groups, key) {
        return Ok(());
    }
    heap.create_data_property(groups, key, v)?;
    Ok(())
}

/// A groups object of `values` (by group index) under the group names of `r`'s program.
fn groups_object(heap: &mut Heap, r: Obj, values: &[Value]) -> Result<Value, Fault> {
    let g = heap.new_object(None)?;
    for (i, v) in values.iter().enumerate().skip(1) {
        let name: Option<Vec<u16>> = program(heap, r).group_name(i).map(|n| n.collect());
        if let Some(name) = name {
            define_group(heap, g, &name, *v)?;
        }
    }
    Ok(Value::object(g))
}

/// The match result array of RegExpBuiltinExec (22.2.7.2 steps 22-35) of the RegExp `r`:
/// the captures, `index`, `input`, `groups`, and with the `d` flag `indices`.
pub(crate) fn match_result(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    caps: Caps<'_>,
) -> Result<Value, Fault> {
    let (has_names, indices) = {
        let prog = program(heap, r);
        (
            prog.has_group_names(),
            prog.flags().contains(Flags::HAS_INDICES),
        )
    };
    let n = caps.len();
    // A few groups (most patterns) on the stack.
    let mut few = [Value::UNDEFINED; 8];
    let mut many = Vec::new();
    let values = if n <= few.len() {
        &mut few[..n]
    } else {
        many.resize(n, Value::UNDEFINED);
        &mut many[..]
    };
    for (i, v) in values.iter_mut().enumerate() {
        if let Some((a, b)) = caps.get(i) {
            *v = substring(heap, s, a, b)?;
        }
    }
    let values = &*values;
    let array_proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    let a = heap.new_array_from_values(Some(array_proto), values)?;
    let (start, _) = caps.get(0).expect("group 0 matched");
    let index = atoms::INDEX;
    heap.create_data_property(a, index, Value::int(start as i32))?;
    let input = atoms::INPUT;
    heap.create_data_property(a, input, Value::string(s))?;
    let groups = if has_names {
        groups_object(heap, r, values)?
    } else {
        Value::UNDEFINED
    };
    let groups_key = atoms::GROUPS;
    heap.create_data_property(a, groups_key, groups)?;
    if indices {
        let mut pairs = crate::buf::vec_with_capacity(n);
        for i in 0..n {
            pairs.push_out(match caps.get(i) {
                Some((a, b)) => {
                    let pair = [Value::int(a as i32), Value::int(b as i32)];
                    Value::object(heap.new_array_from_values(Some(array_proto), &pair)?)
                }
                None => Value::UNDEFINED,
            });
        }
        let indices = heap.new_array_from_values(Some(array_proto), &pairs)?;
        let igroups = if has_names {
            groups_object(heap, r, &pairs)?
        } else {
            Value::UNDEFINED
        };
        heap.create_data_property(indices, groups_key, igroups)?;
        let key = heap.atom_str("indices")?;
        heap.create_data_property(a, key, Value::object(indices))?;
    }
    Ok(Value::object(a))
}

/// A new RegExp String Iterator.
pub(crate) fn string_iterator(heap: &mut Heap, r: Obj, s: Str, flags: i32) -> Result<Obj, Fault> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::RegExpStringIteratorProto);
    let it = heap.alloc_object(ObjectClass::RegExpStringIterator, Some(proto), 3)?;
    heap.set_object_slot(it, IT_REGEXP, Value::object(r));
    heap.set_object_slot(it, IT_STRING, Value::string(s));
    heap.set_object_slot(it, IT_FLAGS, Value::int(flags));
    Ok(it)
}

/// The RegExp String Iterator `v`, or qjs's TypeError.
pub(crate) fn as_string_iterator(heap: &Heap, v: Value) -> Result<Obj, Fault> {
    match v
        .as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::RegExpStringIterator)
    {
        Some(o) => Ok(o),
        None => type_error("RegExp String Iterator object expected"),
    }
}

/// The input of the matcher: a flat string's code units, read in place.
pub(crate) fn input(view: StrView<'_>) -> regexp::Input<'_> {
    match view {
        StrView::Latin1(b) => regexp::Input::Latin1(b),
        StrView::Utf16(b) => regexp::Input::Utf16Le(b),
    }
}

/// RegExp.escape ( S ) of a string, as QuickJS writes it (lowercase hex).
pub(crate) fn escape(heap: &mut Heap, s: Str) -> Result<Str, Fault> {
    let units = heap.str_to_utf16(s);
    let mut out = String::new();
    let mut i = 0;
    while i < units.len() {
        let i0 = i;
        let mut c = units[i] as u32;
        i += 1;
        if (0xD800..0xDC00).contains(&c) && i < units.len() && (0xDC00..0xE000).contains(&units[i])
        {
            c = 0x10000 + ((c - 0xD800) << 10) + (units[i] as u32 - 0xDC00);
            i += 1;
        }
        let hex2 = c < 256
            && (c < 33 && !(9..=13).contains(&c)
                || c >= 128
                || (c as u8).is_ascii_alphanumeric() && i0 == 0
                || b",-=<>#&!%:;@~'`\"".contains(&(c as u8)));
        if hex2 {
            crate::text::cat_to!(&mut out, "\\x", crate::text::hex(c, 2));
        } else if (9..=13).contains(&c) {
            out.push_out('\\');
            out.push_out(b"tnvfr"[c as usize - 9] as char);
        } else if c < 128 {
            if !(c as u8).is_ascii_alphanumeric() && c != b'_' as u32 {
                out.push_out('\\');
            }
            out.push_out(c as u8 as char);
        } else if (0xD800..0xE000).contains(&c)
            || matches!(
                c,
                0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF
            )
        {
            crate::text::cat_to!(&mut out, "\\u", crate::text::hex(c, 4));
        } else {
            out.push_out(char::from_u32(c).unwrap_or('\u{FFFD}'));
        }
    }
    Ok(heap.alloc_str(&out)?)
}

/// AdvanceStringIndex ( S, index, unicode ).
pub(crate) fn advance(view: StrView<'_>, i: usize, unicode: bool) -> usize {
    if unicode
        && i + 1 < view.len()
        && (0xD800..0xDC00).contains(&view.get(i))
        && (0xDC00..0xE000).contains(&view.get(i + 1))
    {
        i + 2
    } else {
        i + 1
    }
}

/// GetSubstitution for a match of an unmodified RegExp (D1503): `$$`, `$&`, `` $` ``,
/// `$'`, `$n`/`$nn` and `$<name>` (only with named groups, else literal), appended to
/// `out`. `s` is the matched string.
pub(crate) fn substitute(
    out: &mut Units,
    template: &[u16],
    s: StrView<'_>,
    caps: Caps<'_>,
    prog: Prog<'_>,
) {
    let (m0, m1) = caps.get(0).expect("group 0 matched");
    let m = caps.len() - 1;
    let push_range = |out: &mut Units, a: usize, b: usize| out.range(s, a, b);
    let mut i = 0;
    while i < template.len() {
        let c = template[i];
        if c != b'$' as u16 || i + 1 >= template.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let d = template[i + 1];
        match d {
            0x24 => {
                out.push(0x24);
                i += 2;
            }
            0x26 => {
                push_range(out, m0, m1);
                i += 2;
            }
            0x60 => {
                push_range(out, 0, m0);
                i += 2;
            }
            0x27 => {
                push_range(out, m1.min(s.len()), s.len());
                i += 2;
            }
            0x30..=0x39 => {
                let mut index = (d - 0x30) as usize;
                let mut digits = 1;
                if let Some(&e) = template.get(i + 2)
                    && (0x30..=0x39).contains(&e)
                {
                    let two = index * 10 + (e - 0x30) as usize;
                    if (1..=m).contains(&two) {
                        index = two;
                        digits = 2;
                    }
                }
                if (1..=m).contains(&index) {
                    if let Some((a, b)) = caps.get(index) {
                        push_range(out, a, b);
                    }
                    i += 1 + digits;
                } else {
                    out.push(0x24);
                    i += 1;
                }
            }
            0x3C if prog.has_group_names() => {
                match template[i + 2..].iter().position(|&u| u == b'>' as u16) {
                    None => {
                        out.push(0x24);
                        out.push(0x3C);
                        i += 2;
                    }
                    Some(k) => {
                        let name = &template[i + 2..i + 2 + k];
                        // The participating group of that name, if any (duplicate names).
                        let span = (1..caps.len())
                            .filter(|&g| prog.group_name_is(g, name))
                            .find_map(|g| caps.get(g));
                        if let Some((a, b)) = span {
                            push_range(out, a, b);
                        }
                        i += 3 + k;
                    }
                }
            }
            _ => {
                out.push(0x24);
                i += 1;
            }
        }
    }
}

// ---- The protocol's checks and the matcher in one go (the `@fast` entries of
// builtins/fast.rs, which decline where the builtins suspend or throw) ----

const REGEXP_PROTOTYPE_EXEC: u16 = super::fast::intrinsic_id("RegExpPrototypeExec");
const REGEXP_PROTOTYPE_MATCH: u16 = super::fast::intrinsic_id("RegExpPrototypeMatch");
const REGEXP_SPECIES: u16 = super::fast::intrinsic_id("RegExpSpecies");
/// The accessors of %RegExp.prototype% that a pristine RegExp must still have (D1503).
const REGEXP_GETTERS: [(Atom, u16); 9] = [
    (
        atoms::FLAGS,
        super::fast::intrinsic_id("RegExpPrototypeFlags"),
    ),
    (
        atoms::HAS_INDICES,
        super::fast::intrinsic_id("RegExpPrototypeHasIndices"),
    ),
    (
        atoms::GLOBAL,
        super::fast::intrinsic_id("RegExpPrototypeGlobal"),
    ),
    (
        atoms::IGNORE_CASE,
        super::fast::intrinsic_id("RegExpPrototypeIgnoreCase"),
    ),
    (
        atoms::MULTILINE,
        super::fast::intrinsic_id("RegExpPrototypeMultiline"),
    ),
    (
        atoms::DOT_ALL,
        super::fast::intrinsic_id("RegExpPrototypeDotAll"),
    ),
    (
        atoms::UNICODE,
        super::fast::intrinsic_id("RegExpPrototypeUnicode"),
    ),
    (
        atoms::UNICODE_SETS,
        super::fast::intrinsic_id("RegExpPrototypeUnicodeSets"),
    ),
    (
        atoms::STICKY,
        super::fast::intrinsic_id("RegExpPrototypeSticky"),
    ),
];

/// The methods of %RegExp.prototype% a fast path relies on, by their place in `SLOTS`
/// (the getters of `REGEXP_GETTERS` follow them).
pub(crate) const EXEC: usize = 0;
pub(crate) const MATCH: usize = 1;
pub(crate) const REPLACE: usize = 2;
pub(crate) const SEARCH: usize = 3;
pub(crate) const SPLIT: usize = 4;
pub(crate) const MATCH_ALL: usize = 5;
const CONSTRUCTOR: usize = 6;
const GETTERS: usize = 7;

/// The entry slot of %RegExp.prototype% where each of those properties was found last.
/// Only a hint (the slot is read with its key): every VM of a base image has them in the
/// same slots, so a check reads its entries in place of as many table searches.
static SLOTS: [AtomicU32; GETTERS + REGEXP_GETTERS.len()] =
    [const { AtomicU32::new(0) }; GETTERS + REGEXP_GETTERS.len()];

/// %RegExp.prototype%'s own property `key` (whose hint is `SLOTS[which]`).
fn proto_property(heap: &Heap, which: usize, key: Atom) -> Option<Property> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::RegExpProto);
    let Some(slots) = heap.plain_entry_slots(proto) else {
        return heap.get_own_property(proto, key);
    };
    let hint = SLOTS[which].load(Ordering::Relaxed);
    if hint < slots
        && let Some((k, p)) = heap.plain_entry(proto, hint)
        && k == key
    {
        return Some(p);
    }
    let at = heap.own_entry_index(proto, key)?;
    SLOTS[which].store(at, Ordering::Relaxed);
    heap.plain_entry(proto, at).map(|(_, p)| p)
}

/// The own data property `key` of `o`.
fn own_data(heap: &Heap, o: Obj, key: Atom) -> Option<Value> {
    match heap.get_own_property(o, key) {
        Some(Property::Data { value, .. }) => Some(value),
        _ => None,
    }
}

/// Whether %RegExp.prototype%'s property `key` (`which` of `SLOTS`) is the data property
/// holding the builtin installed for intrinsic `id`.
pub(crate) fn original_method(heap: &Heap, which: usize, key: Atom, id: u16) -> bool {
    let f = realm::installed_intrinsic(heap, id);
    f.as_object().is_some()
        && matches!(proto_property(heap, which, key), Some(Property::Data { value, .. }) if value == f)
}

/// Whether RegExpExec ( R, S ) is RegExpBuiltinExec ( R, S ) without a lookup that could
/// run user code (D3137): `r` has no own `exec` and its prototype is %RegExp.prototype%
/// with the original one.
pub(crate) fn exec_is_original(heap: &Heap, r: Obj) -> bool {
    let proto = realm::intrinsic_obj(heap, Intrinsic::RegExpProto);
    heap.get_prototype_of(r) == Some(proto)
        && (heap.only_own_key(r, atoms::LAST_INDEX)
            || heap.get_own_property(r, atoms::EXEC).is_none())
        && original_method(heap, EXEC, atoms::EXEC, REGEXP_PROTOTYPE_EXEC)
}

/// `lastIndex` of `r` when RegExpBuiltinExec reads and writes it as data: a writable
/// Number.
pub(crate) fn last_index(heap: &Heap, r: Obj) -> Option<f64> {
    match heap.get_own_property(r, atoms::LAST_INDEX) {
        Some(Property::Data { value, flags }) if flags.writable() => value.as_number(),
        _ => None,
    }
}

/// Whether `v` is a RegExp whose protocol is unobservable (D1503): its own properties
/// are just a writable Number `lastIndex`, its prototype is %RegExp.prototype% with the
/// original `exec`, `flags`, flag getters, `constructor` and @@match, and %RegExp%'s
/// @@species getter is the original. Then RegExpExec, the flags, SpeciesConstructor
/// and IsRegExp read only data, and a fast path may skip them.
pub(crate) fn pristine(heap: &Heap, v: Value) -> Option<Obj> {
    let r = as_regexp(heap, v)?;
    let proto = realm::intrinsic_obj(heap, Intrinsic::RegExpProto);
    if heap.get_prototype_of(r) != Some(proto) || !heap.only_own_key(r, atoms::LAST_INDEX) {
        return None;
    }
    last_index(heap, r)?;
    if !original_method(heap, EXEC, atoms::EXEC, REGEXP_PROTOTYPE_EXEC)
        || !original_method(heap, MATCH, atoms::SYM_MATCH, REGEXP_PROTOTYPE_MATCH)
    {
        return None;
    }
    let installed = |id: u16| realm::installed_intrinsic(heap, id);
    for (i, (key, id)) in REGEXP_GETTERS.into_iter().enumerate() {
        match proto_property(heap, GETTERS + i, key) {
            Some(Property::Accessor { getter, .. }) if getter == installed(id) => {}
            _ => return None,
        }
    }
    let ctors = realm::intrinsic_obj(heap, Intrinsic::Constructors);
    let ctor = own_data(heap, ctors, atoms::REG_EXP)?;
    match proto_property(heap, CONSTRUCTOR, atoms::CONSTRUCTOR) {
        Some(Property::Data { value, .. }) if value == ctor => {}
        _ => return None,
    }
    match heap.get_own_property(ctor.as_object()?, atoms::SYM_SPECIES) {
        Some(Property::Accessor { getter, .. }) if getter == installed(REGEXP_SPECIES) => Some(r),
        _ => None,
    }
}

/// The most words of a matcher state that stay with the heap between matches (4 KiB):
/// most patterns need a few frames, and a deep backtrack's stack is not kept.
const SPARE_STATE_WORDS: usize = 1024;

/// Runs `f` with the heap's spare matcher state, whose memory match after match reuses.
fn with_state<T>(heap: &mut Heap, f: impl FnOnce(&mut Heap, &mut ExecState) -> T) -> T {
    let mut st = core::mem::take(&mut heap.regexp_state);
    let r = f(heap, &mut st);
    if st.capacity() <= SPARE_STATE_WORDS {
        heap.regexp_state = st;
    }
    r
}

/// The most matcher steps a `@fast` entry takes. An entry runs to its end and charges
/// its steps afterwards (like the leaves that run to completion), so this is also how
/// far it can run past the end of a fuel slice: about 50 µs at 3 ns a step. A match
/// that needs more is the builtin's, which suspends when a slice ends; the entry then
/// charges nothing. Whether a match stays below it depends on the RegExp and the string
/// alone, never on the fuel left, so a run stops at the same point however its fuel is
/// sliced (D1710).
const FAST_STEPS: i64 = 16_384;

/// Whether an entry that finds every match in a string of `len` code units should not
/// start: passing a code unit costs a step, so a longer string does not fit.
fn too_long(len: usize) -> bool {
    len as u64 > FAST_STEPS as u64
}

/// Charges the steps of an entry's matcher runs that ended (with `left` of
/// [`FAST_STEPS`]); one that did not end charges nothing.
fn charge(fuel: &mut i64, left: i64, ended: bool) {
    if ended {
        *fuel -= FAST_STEPS - left;
    }
}

/// Runs the matcher of `r` on the contiguous string `flat` from `start` for at most
/// [`FAST_STEPS`]: `OutOfFuel` is a match that needs more.
fn run(
    heap: &Heap,
    r: Obj,
    flat: Str,
    start: usize,
    sticky: bool,
    fuel: &mut i64,
    st: &mut ExecState,
) -> Outcome {
    let mut left = FAST_STEPS;
    let prog = program(heap, r);
    let view = heap.str_view(flat).expect("flat string");
    let limits = ExecLimits::default();
    let o = regexp::exec_in(prog, input(view), start, sticky, &mut left, limits, st);
    charge(fuel, left, matches!(o, Outcome::Match | Outcome::NoMatch));
    o
}

/// ToLength of a Number `lastIndex`.
pub(crate) fn to_length(last: f64) -> f64 {
    if last > 0.0 {
        djs_libm::trunc(last).min(9007199254740991.0)
    } else {
        0.0
    }
}

/// A new RegExp with the program, the source and the flags of `r` and `lastIndex`
/// `last`: RegExpCreate of `r`'s source and flags (`create_like`), without the flags
/// string.
pub(crate) fn copy(heap: &mut Heap, r: Obj, last: Value) -> Result<Obj, Fault> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::RegExpProto);
    let o = heap.alloc_object(ObjectClass::RegExp, Some(proto), 3)?;
    for slot in [SLOT_PROGRAM, SLOT_SOURCE, SLOT_FLAGS] {
        let v = heap.object_slot(r, slot);
        heap.set_object_slot(o, slot, v);
    }
    let d = PropertyDescriptor::data(last, PropFlags::WRITABLE);
    heap.define_own_property(o, atoms::LAST_INDEX, &d)?;
    Ok(o)
}

/// RegExpBuiltinExec ( R, S ) once `lastIndex` was read as `last` (`last_index`), in one
/// go: the match array or null (with `test`, true or false), `lastIndex` updated. `None`,
/// with nothing changed, when the match needs more than [`FAST_STEPS`] or the matcher's
/// stack overflowed: the builtin runs the match again, and suspends or throws.
pub(crate) fn exec_now(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    last: f64,
    test: bool,
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let f = flags(heap, r);
    let sticky = f.contains(Flags::STICKY);
    let g_or_y = sticky || f.contains(Flags::GLOBAL);
    let last = if g_or_y { to_length(last) } else { 0.0 };
    let flat = heap.contiguous(s)?;
    if last > heap.str_len(flat) as f64 {
        if g_or_y {
            heap.set_own_data_value(r, atoms::LAST_INDEX, Value::int(0));
        }
        return Ok(Some(if test { Value::FALSE } else { Value::NULL }));
    }
    // The match array before lastIndex: at the heap cap nothing has changed.
    let found: Result<Option<(Value, usize)>, Fault> = with_state(heap, |heap, st| {
        Ok(match run(heap, r, flat, last as usize, sticky, fuel, st) {
            Outcome::OutOfFuel | Outcome::StackOverflow => None,
            Outcome::NoMatch => Some((if test { Value::FALSE } else { Value::NULL }, 0)),
            Outcome::Match => {
                let caps = st.captures(program(heap, r));
                let (_, end) = caps.get(0).expect("group 0 matched");
                let v = if test {
                    Value::TRUE
                } else {
                    match_result(heap, r, s, caps)?
                };
                Some((v, end))
            }
        })
    });
    let Some((v, next)) = found? else {
        return Ok(None);
    };
    if g_or_y {
        heap.set_own_data_value(r, atoms::LAST_INDEX, Value::int(next as i32));
    }
    Ok(Some(v))
}

/// Every match of the global RegExp `r` in the contiguous string `flat` from lastIndex
/// 0, as @@match and @@replace find them (after an empty match the next one starts a
/// character on, a code point with `u` or `v`). False when the matcher did not finish.
fn each_match(
    heap: &Heap,
    r: Obj,
    flat: Str,
    fuel: &mut i64,
    st: &mut ExecState,
    mut each: impl FnMut(Caps<'_>, StrView<'_>, Prog<'_>),
) -> bool {
    let prog = program(heap, r);
    let view = heap.str_view(flat).expect("flat string");
    let input = input(view);
    let unicode = prog.flags().unicode_mode();
    let sticky = prog.flags().contains(Flags::STICKY);
    let limits = ExecLimits::default();
    if too_long(view.len()) {
        return false;
    }
    let mut left = FAST_STEPS;
    let mut last = 0;
    let finished = loop {
        if last > view.len() {
            break true;
        }
        match regexp::exec_in(prog, input, last, sticky, &mut left, limits, st) {
            Outcome::Match => {}
            Outcome::NoMatch => break true,
            Outcome::OutOfFuel | Outcome::StackOverflow => break false,
        }
        let caps = st.captures(prog);
        let (m0, m1) = caps.get(0).expect("group 0 matched");
        each(caps, view, prog);
        last = if m1 == m0 {
            advance(view, m1, unicode)
        } else {
            m1
        };
    };
    charge(fuel, left, finished);
    finished
}

/// RegExp.prototype[@@match] of a pristine global RegExp: the matched strings or null,
/// and lastIndex 0. `None` as for `exec_now`.
pub(crate) fn match_all_now(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let flat = heap.contiguous(s)?;
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let finished = with_state(heap, |heap, st| {
        each_match(heap, r, flat, fuel, st, |caps, _, _| {
            cuts.push_out(caps.get(0).expect("group 0 matched"));
        })
    });
    if !finished {
        return Ok(None);
    }
    let v = if cuts.is_empty() {
        Value::NULL
    } else {
        let mut values = crate::buf::vec_with_capacity(cuts.len());
        for (a, b) in cuts {
            values.push_out(substring(heap, flat, a, b)?);
        }
        let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
        Value::object(heap.new_array_from_values(Some(proto), &values)?)
    };
    heap.set_own_data_value(r, atoms::LAST_INDEX, Value::int(0));
    Ok(Some(v))
}

/// RegExp.prototype[@@replace] of a pristine RegExp with a string replacement template:
/// every match of a global one (lastIndex 0 at the end), else the first from lastIndex
/// (0 unless sticky, which also writes it). `None` as for `exec_now`.
pub(crate) fn replace_now(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    template: Str,
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let flat = heap.contiguous(s)?;
    let size = heap.str_len(flat) as usize;
    let template = heap.str_to_utf16(template);
    let narrow = !heap.str_view(flat).expect("flat string").is_two_byte()
        && template.iter().all(|&u| u <= 0xFF);
    let mut out = if narrow {
        Units::Narrow(crate::buf::vec_with_capacity(size))
    } else {
        Units::Wide(crate::buf::vec_with_capacity(size))
    };
    let f = flags(heap, r);
    let sticky = f.contains(Flags::STICKY);
    let mut next_pos = None;
    let last = if f.contains(Flags::GLOBAL) {
        let finished = with_state(heap, |heap, st| {
            each_match(heap, r, flat, fuel, st, |caps, view, prog| {
                let (m0, m1) = caps.get(0).expect("group 0 matched");
                out.range(view, next_pos.unwrap_or(0), m0);
                substitute(&mut out, &template, view, caps, prog);
                next_pos = Some(m1);
            })
        });
        if !finished {
            return Ok(None);
        }
        Some(0)
    } else {
        let start = match last_index(heap, r) {
            Some(last) if sticky => to_length(last),
            _ => 0.0,
        };
        let found = with_state(heap, |heap, st| {
            if start > size as f64 {
                return Some(false);
            }
            match run(heap, r, flat, start as usize, sticky, fuel, st) {
                Outcome::OutOfFuel | Outcome::StackOverflow => None,
                Outcome::NoMatch => Some(false),
                Outcome::Match => {
                    let prog = program(heap, r);
                    let view = heap.str_view(flat).expect("flat string");
                    let caps = st.captures(prog);
                    let (m0, m1) = caps.get(0).expect("group 0 matched");
                    out.range(view, 0, m0);
                    substitute(&mut out, &template, view, caps, prog);
                    next_pos = Some(m1);
                    Some(true)
                }
            }
        });
        if found.is_none() {
            return Ok(None);
        }
        sticky.then_some(next_pos.unwrap_or(0))
    };
    // The result before lastIndex: at the heap cap nothing has changed.
    let result = match next_pos {
        None => s,
        Some(p) => {
            out.range(heap.str_view(flat).expect("flat string"), p.min(size), size);
            out.into_string(heap)?
        }
    };
    if let Some(last) = last {
        heap.set_own_data_value(r, atoms::LAST_INDEX, Value::int(last as i32));
    }
    Ok(Some(Value::string(result)))
}

/// RegExp.prototype[@@search] of a pristine RegExp: the index of the first match from 0
/// or -1; lastIndex ends as it was. `None` as for `exec_now`.
pub(crate) fn search_now(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let flat = heap.contiguous(s)?;
    let sticky = flags(heap, r).contains(Flags::STICKY);
    Ok(with_state(heap, |heap, st| {
        match run(heap, r, flat, 0, sticky, fuel, st) {
            Outcome::OutOfFuel | Outcome::StackOverflow => None,
            Outcome::NoMatch => Some(Value::int(-1)),
            Outcome::Match => {
                let caps = st.captures(program(heap, r));
                let (at, _) = caps.get(0).expect("group 0 matched");
                Some(Value::int(at as i32))
            }
        }
    }))
}

/// RegExp.prototype[@@split] of a pristine RegExp on a non-empty string with 0 < lim
/// (what `%RegExpSplitRaw` does, D1503). `None` as for `exec_now`.
pub(crate) fn split_now(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    lim: u32,
    fuel: &mut i64,
) -> Result<Option<Value>, Fault> {
    let flat = heap.contiguous(s)?;
    let size = heap.str_len(flat) as usize;
    // The pieces: a range of the string, or an undefined capture.
    let mut cuts: Vec<Option<(usize, usize)>> = Vec::new();
    let full = |cuts: &Vec<Option<(usize, usize)>>| cuts.len() as u32 == lim;
    let (mut p, mut q) = (0, 0);
    if too_long(size) {
        return Ok(None);
    }
    let mut left = FAST_STEPS;
    let finished = with_state(heap, |heap, st| {
        let prog = program(heap, r);
        let view = heap.str_view(flat).expect("flat string");
        let input = input(view);
        let unicode = prog.flags().unicode_mode();
        let limits = ExecLimits::default();
        'matches: loop {
            if q >= size {
                break true;
            }
            match regexp::exec_in(prog, input, q, false, &mut left, limits, st) {
                Outcome::Match => {}
                Outcome::NoMatch => break true,
                Outcome::OutOfFuel | Outcome::StackOverflow => break false,
            }
            let caps = st.captures(prog);
            let (at, e) = caps.get(0).expect("group 0 matched");
            if at >= size {
                break true;
            }
            if e == p {
                q = advance(view, at, unicode);
                continue;
            }
            cuts.push_out(Some((p, at)));
            if full(&cuts) {
                break true;
            }
            for g in 1..caps.len() {
                cuts.push_out(caps.get(g));
                if full(&cuts) {
                    break 'matches true;
                }
            }
            p = e;
            q = p;
        }
    });
    charge(fuel, left, finished);
    if !finished {
        return Ok(None);
    }
    if !full(&cuts) {
        cuts.push_out(Some((p.min(size), size)));
    }
    let mut values = crate::buf::vec_with_capacity(cuts.len());
    for cut in cuts {
        values.push_out(match cut {
            Some((a, b)) => substring(heap, flat, a, b)?,
            None => Value::UNDEFINED,
        });
    }
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(Some(Value::object(
        heap.new_array_from_values(Some(proto), &values)?,
    )))
}

// ---- A replacer function's matches (`%RegExpMatchSpans` and its readers) ----

/// The Numbers per match in a spans array of `r`: the start and the end of every group.
pub(crate) fn span_width(heap: &Heap, r: Obj) -> u32 {
    2 * program(heap, r).capture_count() as u32
}

/// The number of matches in `spans`.
pub(crate) fn span_count(heap: &Heap, r: Obj, spans: Obj) -> u32 {
    heap.array_length(spans) / span_width(heap, r)
}

/// Element `i` of a spans array: a position, or `None` for a group that did not
/// participate.
fn span(heap: &Heap, spans: Obj, i: u32) -> Option<u32> {
    heap.get_element_fast(spans, i)
        .and_then(Value::as_int)
        .and_then(|x| u32::try_from(x).ok())
}

/// The arguments of a replacer function for match `k` of `spans` (22.2.6.11 step
/// 14.l): the matched string, the captures, the position, the string, and the groups
/// object when the RegExp has named groups.
pub(crate) fn replace_args(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    spans: Obj,
    k: u32,
) -> Result<Obj, Fault> {
    let (width, names) = (span_width(heap, r), program(heap, r).has_group_names());
    let at = width * k;
    let mut values = crate::buf::vec_with_capacity(width as usize / 2 + 3);
    for g in (0..width).step_by(2) {
        values.push_out(
            match (span(heap, spans, at + g), span(heap, spans, at + g + 1)) {
                (Some(a), Some(b)) => substring(heap, s, a as usize, b as usize)?,
                _ => Value::UNDEFINED,
            },
        );
    }
    let groups = if names {
        Some(groups_object(heap, r, &values)?)
    } else {
        None
    };
    values.push_out(Value::int(span(heap, spans, at).unwrap_or(0) as i32));
    values.push_out(Value::string(s));
    values.extend(groups);
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(heap.new_array_from_values(Some(proto), &values)?)
}

/// The matched strings of `spans` as an Array, for a RegExp without capture groups;
/// undefined otherwise.
pub(crate) fn span_texts(heap: &mut Heap, r: Obj, s: Str, spans: Obj) -> Result<Value, Fault> {
    if span_width(heap, r) != 2 {
        return Ok(Value::UNDEFINED);
    }
    let count = span_count(heap, r, spans);
    let mut values = crate::buf::vec_with_capacity(count as usize);
    for k in 0..count {
        values.push_out(
            match (span(heap, spans, 2 * k), span(heap, spans, 2 * k + 1)) {
                (Some(a), Some(b)) => substring(heap, s, a as usize, b as usize)?,
                _ => Value::UNDEFINED,
            },
        );
    }
    Ok(Value::object(heap.new_array_from_values(None, &values)?))
}

/// The Array the replacements of `spans` go to: the position of each match for a RegExp
/// without capture groups, else empty.
pub(crate) fn span_starts(heap: &mut Heap, r: Obj, spans: Obj) -> Result<Obj, Fault> {
    let mut values = Vec::new();
    if span_width(heap, r) == 2 {
        let count = span_count(heap, r, spans);
        values.reserve_exact(count as usize);
        for k in 0..count {
            values.push_out(Value::int(span(heap, spans, 2 * k).unwrap_or(0) as i32));
        }
    }
    Ok(heap.new_array_from_values(None, &values)?)
}

/// `s` with each match of `spans` replaced by the string at its index in
/// `replacements`, written once.
pub(crate) fn replace_join(
    heap: &mut Heap,
    r: Obj,
    s: Str,
    spans: Obj,
    replacements: Obj,
) -> Result<Str, Fault> {
    let (width, count) = (span_width(heap, r), span_count(heap, r, spans));
    if count == 0 {
        return Ok(s);
    }
    let mut pieces = crate::buf::vec_with_capacity(2 * count as usize + 1);
    let mut from = 0;
    for k in 0..count {
        let (Some(a), Some(b), Some(to)) = (
            span(heap, spans, width * k),
            span(heap, spans, width * k + 1),
            heap.get_element_fast(replacements, k)
                .and_then(Value::as_string),
        ) else {
            continue;
        };
        pieces.push_out((s, from, a));
        pieces.push_out((to, 0, heap.str_len(to)));
        from = b;
    }
    let len = heap.str_len(s);
    if from < len {
        pieces.push_out((s, from, len));
    }
    super::string::join(heap, &mut pieces)
}

/// A heap string from UTF-16 units (one byte per unit when possible).
#[cfg(feature = "unicode-normalization")]
pub(crate) fn string_from_units(heap: &mut Heap, units: &[u16]) -> Result<Str, HeapError> {
    alloc_units(heap, units)
}

/// A string being built from code units: one byte each while every unit fits (a
/// replace of a one-byte string with a one-byte template), else two.
pub(crate) enum Units {
    Narrow(Vec<u8>),
    Wide(Vec<u16>),
}

impl Units {
    pub(crate) fn len(&self) -> usize {
        match self {
            Units::Narrow(v) => v.len(),
            Units::Wide(v) => v.len(),
        }
    }

    /// Appends `u` (at most 0xFF in a narrow buffer).
    pub(crate) fn push(&mut self, u: u16) {
        match self {
            Units::Narrow(v) => v.push_out(u as u8),
            Units::Wide(v) => v.push_out(u),
        }
    }

    /// Appends `s[a..b)` (a one-byte `s` in a narrow buffer).
    pub(crate) fn range(&mut self, s: StrView<'_>, a: usize, b: usize) {
        match (self, s) {
            (Units::Narrow(v), StrView::Latin1(bytes)) => v.extend_from_slice(&bytes[a..b]),
            // (A narrow buffer only ever gets one-byte strings.)
            (Units::Narrow(v), s) => v.extend((a..b).map(|i| s.get(i) as u8)),
            (Units::Wide(v), s) => v.extend((a..b).map(|i| s.get(i))),
        }
    }

    pub(crate) fn to_utf16(&self) -> Vec<u16> {
        match self {
            Units::Narrow(v) => v.iter().map(|&c| c as u16).collect(),
            Units::Wide(v) => v.clone(),
        }
    }

    /// The heap string (one byte per unit when possible).
    pub(crate) fn into_string(self, heap: &mut Heap) -> Result<Str, HeapError> {
        match self {
            Units::Narrow(v) => heap.alloc_latin1(&v),
            Units::Wide(v) => alloc_units(heap, &v),
        }
    }
}
