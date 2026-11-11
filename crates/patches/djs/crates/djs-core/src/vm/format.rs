//! Value formatting for `console.log` in `djs run` (a simple printer, D1100: strings raw,
//! Errors as name and message, BigInts as `10n`, everything else JSON with a `String()`
//! fallback).

use alloc::string::String;

use crate::heap::{Heap, ObjectClass};
use crate::interp::error_summary;
use crate::json::stringify_fast;
use crate::num;
use crate::value::{Kind, Value};

pub(crate) fn display(heap: &mut Heap, v: Value) -> String {
    if let Some(s) = v.as_string() {
        return heap.str_to_string(s);
    }
    if let Some(e) = error_summary(heap, v) {
        return e;
    }
    match v.kind() {
        Kind::Undefined => return String::from("undefined"),
        Kind::Number => {
            let n = v.as_number().unwrap();
            if n == 0.0 && n.is_sign_negative() {
                return String::from("-0");
            }
            if !n.is_finite() {
                return String::from(num::to_string(n).as_str());
            }
        }
        Kind::Symbol => {
            let d = heap.symbol_description(v.as_symbol().unwrap());
            let d = d
                .as_string()
                .map(|s| heap.str_to_string(s))
                .unwrap_or_default();
            return crate::text::cat!("Symbol(", d, ")");
        }
        Kind::Object => {
            let o = v.as_object().unwrap();
            if heap.is_callable(o) {
                return String::from("function");
            }
        }
        // As qjs's console.log prints a BigInt.
        Kind::BigInt => return crate::text::cat!(heap.bigint(v).to_string_radix(10), "n"),
        _ => {}
    }
    match stringify_fast(heap, v, &[]) {
        Ok(Some(s)) => heap.str_to_string(s),
        _ => match v.as_object().map(|o| heap.object_class(o)) {
            Some(ObjectClass::Array) => String::from("[array]"),
            _ => String::from("[object Object]"),
        },
    }
}
