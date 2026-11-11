//! JSON leaf intrinsics over `crate::json` (D1331): the parse without a reviver, the
//! stringify fast path, and QuoteJSONString. builtins/json.ts does the rest.
use crate::PushOut;

use alloc::string::String;
use alloc::vec::Vec;

use crate::heap::{Atom, Heap, ObjectClass, PropFlags, PropertyDescriptor, atoms};
use crate::interp::conv::Fault;
use crate::interp::{ErrorKind, Intrinsic, realm};
use crate::json::{self, ParseError, ParseOptions, StringifyError};
use crate::value::{Obj, Str, Value};

/// `%JsonParse(text)`: the parsed value, or the SyntaxError (qjs's message and
/// position) for the caller to throw.
pub(crate) fn parse(heap: &mut Heap, text: Str) -> Result<Result<Value, json::SyntaxError>, Fault> {
    let opts = ParseOptions {
        object_proto: Some(realm::intrinsic_obj(heap, Intrinsic::ObjectProto)),
        array_proto: Some(realm::intrinsic_obj(heap, Intrinsic::ArrayProto)),
        ..Default::default()
    };
    match json::parse(heap, text, &opts) {
        Ok(v) => Ok(Ok(v)),
        Err(ParseError::Syntax(e)) => Ok(Err(e)),
        Err(ParseError::Heap(e)) => Err(Fault::heap(e)),
    }
}

/// `%JsonStringifyFast(value, gap)`: the result string, `undefined` for an undefined
/// result, or `null` when the value needs the full algorithm (nothing observable has
/// happened then).
pub(crate) fn stringify_fast(heap: &mut Heap, v: Value, gap: Str) -> Result<Value, Fault> {
    stringify_fast_at(heap, v, gap, &[], true, None)
}

/// `%JsonStringifyFastAt(value, gap, stack)`: [`stringify_fast`] for a value the full
/// algorithm reached (its toJSON and replacer done), inside the objects of `stack` (an
/// Array): its plain subtrees take the fast path (D1901).
pub(crate) fn stringify_fast_in(
    heap: &mut Heap,
    v: Value,
    gap: Str,
    stack: Obj,
    list: Value,
) -> Result<Value, Fault> {
    let n = heap.dense_len(stack);
    let mut outer = crate::buf::vec_with_capacity(n as usize);
    for i in 0..n {
        match heap.get_element_fast(stack, i).and_then(|v| v.as_object()) {
            Some(o) => outer.push_out(o),
            None => return Ok(Value::NULL),
        }
    }
    if heap.object_class(stack) != ObjectClass::Array || heap.array_length(stack) != n {
        return Ok(Value::NULL);
    }
    // The array replacer's PropertyList (strings, from JSONPropertyList) as keys.
    let keys = match list.as_object() {
        None => None,
        Some(l) => {
            let n = heap.dense_len(l);
            if heap.object_class(l) != ObjectClass::Array || heap.array_length(l) != n {
                return Ok(Value::NULL);
            }
            let mut keys = crate::buf::vec_with_capacity(n as usize);
            for i in 0..n {
                match heap.get_element_fast(l, i).and_then(|v| v.as_string()) {
                    Some(s) => keys.push_out(heap.intern(s)?),
                    None => return Ok(Value::NULL),
                }
            }
            Some(keys)
        }
    };
    stringify_fast_at(heap, v, gap, &outer, false, keys.as_deref())
}

fn stringify_fast_at(
    heap: &mut Heap,
    v: Value,
    gap: Str,
    outer: &[Obj],
    to_json: bool,
    list: Option<&[Atom]>,
) -> Result<Value, Fault> {
    let gap = heap.str_to_utf16(gap);
    match json::stringify_fast_at(heap, v, &gap, outer, to_json, list) {
        Ok(Some(s)) => Ok(Value::string(s)),
        Ok(None) => Ok(Value::UNDEFINED),
        // The full algorithm throws for a BigInt (after a replacer had its say).
        Err(StringifyError::Slow | StringifyError::BigInt) => Ok(Value::NULL),
        Err(StringifyError::Heap(e)) => Err(Fault::heap(e)),
        Err(StringifyError::Circular) => Err(Fault::error(
            ErrorKind::TypeError,
            String::from("circular reference"),
        )),
        Err(StringifyError::StackOverflow) => Err(Fault::error(
            ErrorKind::RangeError,
            String::from(crate::interp::STACK_OVERFLOW),
        )),
    }
}

/// `%JsonQuote(s)`: QuoteJSONString ( value ), 25.5.2.3.
pub(crate) fn quote(heap: &mut Heap, s: Str) -> Result<Str, Fault> {
    Ok(json::quote(heap, s)?)
}

/// One open container while walking the source for [`source_records`].
enum Open {
    Array {
        value: Value,
        items: Vec<Value>,
    },
    Object {
        value: Value,
        entries: Obj,
        key: Option<Atom>,
    },
}

/// A JSON Parse Record of JSON.parse source text access (D1611), as an internal
/// 4-element Array: `[value, source, elements, entries]`. `source` is the source text of
/// a primitive (else undefined), `elements` an Array of the element records of an array,
/// `entries` a null-prototype object from key to record of an object (the last of
/// duplicate keys wins, like the value).
fn record(heap: &mut Heap, v: [Value; 4]) -> Result<Value, Fault> {
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(Value::object(heap.new_array_from_values(Some(proto), &v)?))
}

/// The value of `key` (an element index or a property) of a parsed container, or
/// undefined if `parent` is not one (a duplicate key whose later value replaced it).
fn child(heap: &Heap, parent: Value, key: Atom) -> Value {
    match parent.as_object() {
        Some(o) => heap
            .get_own_property(o, key)
            .and_then(|p| p.value())
            .unwrap_or(Value::UNDEFINED),
        None => Value::UNDEFINED,
    }
}

/// Decodes the JSON string literal starting at `units[*i]` (the opening quote) and moves
/// past it. The text was validated by the parse.
fn decode_string(units: &[u16], i: &mut usize) -> Vec<u16> {
    let mut out = Vec::new();
    *i += 1;
    while units[*i] != b'"' as u16 {
        let c = units[*i];
        if c == b'\\' as u16 {
            let e = units[*i + 1];
            *i += 2;
            let u = match e as u8 {
                b'b' => 0x08,
                b'f' => 0x0C,
                b'n' => 0x0A,
                b'r' => 0x0D,
                b't' => 0x09,
                b'u' => {
                    let hex = &units[*i..*i + 4];
                    *i += 4;
                    hex.iter().fold(0u16, |acc, &h| {
                        acc * 16 + (h as u8 as char).to_digit(16).unwrap_or(0) as u16
                    })
                }
                _ => e, // `"`, `\`, `/`
            };
            out.push_out(u);
        } else {
            out.push_out(c);
            *i += 1;
        }
    }
    *i += 1;
    out
}

/// The JSON Parse Record tree of `text` (already parsed to `root`), D1611: the source
/// text of every primitive and the records of every element and member, walked with an
/// explicit stack next to the parsed values.
pub(crate) fn source_records(heap: &mut Heap, text: Str, root: Value) -> Result<Value, Fault> {
    let units = heap.str_to_utf16(text);
    let is_ws = |u: u16| matches!(u, 0x09 | 0x0A | 0x0D | 0x20);
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 0;
    let mut current = root;
    let mut done: Option<Value> = None;
    loop {
        while is_ws(units[i]) {
            i += 1;
        }
        // A value starts at `i`; `current` is its parsed value.
        let finished = match units[i] as u8 {
            b'[' => {
                i += 1;
                stack.push_out(Open::Array {
                    value: current,
                    items: Vec::new(),
                });
                None
            }
            b'{' => {
                i += 1;
                let entries = heap.new_object(None)?;
                stack.push_out(Open::Object {
                    value: current,
                    entries,
                    key: None,
                });
                None
            }
            _ => {
                let start = i;
                if units[i] == b'"' as u16 {
                    decode_string(&units, &mut i);
                } else {
                    while i < units.len()
                        && !is_ws(units[i])
                        && !matches!(units[i] as u8, b',' | b']' | b'}')
                    {
                        i += 1;
                    }
                }
                let src = heap.alloc_utf16(&units[start..i])?;
                let src = Value::string(src);
                Some(record(
                    heap,
                    [current, src, Value::UNDEFINED, Value::UNDEFINED],
                )?)
            }
        };
        let mut finished = finished;
        // Close containers and hand finished records to their parents.
        loop {
            while i < units.len() && is_ws(units[i]) {
                i += 1;
            }
            if let Some(rec) = finished.take() {
                match stack.last_mut() {
                    None => {
                        done = Some(rec);
                        break;
                    }
                    Some(Open::Array { items, .. }) => items.push_out(rec),
                    Some(Open::Object { entries, key, .. }) => {
                        let k = key.take().expect("a member key");
                        heap.create_data_property(*entries, k, rec)?;
                    }
                }
            }
            let c = units[i] as u8;
            if c == b',' {
                i += 1;
                break;
            }
            match (c, stack.last()) {
                (b']', Some(Open::Array { .. })) | (b'}', Some(Open::Object { .. })) => {
                    i += 1;
                    finished = Some(match stack.pop().expect("open") {
                        Open::Array { value, items } => {
                            let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
                            let elems = heap.new_array_from_values(Some(proto), &items)?;
                            record(
                                heap,
                                [
                                    value,
                                    Value::UNDEFINED,
                                    Value::object(elems),
                                    Value::UNDEFINED,
                                ],
                            )?
                        }
                        Open::Object { value, entries, .. } => record(
                            heap,
                            [
                                value,
                                Value::UNDEFINED,
                                Value::UNDEFINED,
                                Value::object(entries),
                            ],
                        )?,
                    });
                }
                // The first element or member of a container just opened.
                _ => break,
            }
        }
        if let Some(rec) = done {
            return Ok(rec);
        }
        while is_ws(units[i]) {
            i += 1;
        }
        // The next value: an element, or a member after its key.
        current = match stack.last_mut().expect("an open container") {
            Open::Array { value, items } => {
                let idx = heap.atom_from_index(items.len() as u32)?;
                child(heap, *value, idx)
            }
            Open::Object { value, key, .. } => {
                let k = decode_string(&units, &mut i);
                while is_ws(units[i]) {
                    i += 1;
                }
                i += 1; // ':'
                let atom = heap.atom_utf16(&k)?;
                *key = Some(atom);
                child(heap, *value, atom)
            }
        };
    }
}

/// `%JsonRawJSON(text)`: a frozen, null-prototype RawJSON object whose `rawJSON` is
/// `text`, which must be a JSON primitive without surrounding whitespace (qjs's
/// SyntaxError "invalid rawJSON string" otherwise).
pub(crate) fn raw_json(heap: &mut Heap, text: Str) -> Result<Value, Fault> {
    let invalid = || {
        Err(Fault::error(
            ErrorKind::SyntaxError,
            String::from("invalid rawJSON string"),
        ))
    };
    let len = heap.str_len(text);
    let ws = |u: Option<u16>| matches!(u, Some(0x09 | 0x0A | 0x0D | 0x20));
    if len == 0 || ws(heap.char_code_at(text, 0)) || ws(heap.char_code_at(text, len - 1)) {
        return invalid();
    }
    match parse(heap, text)? {
        Ok(v) if v.as_object().is_none() => {}
        _ => return invalid(),
    }
    let o = heap.alloc_object(ObjectClass::RawJSON, None, 0)?;
    let d = PropertyDescriptor::data(Value::string(text), PropFlags::ENUMERABLE);
    heap.define_own_property(o, atoms::RAW_JSON, &d)?;
    heap.prevent_extensions(o);
    Ok(Value::object(o))
}
