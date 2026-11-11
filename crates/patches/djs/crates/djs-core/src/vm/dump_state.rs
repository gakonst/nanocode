//! The host's state of an open call (D2770): JSON text the dumper writes as the fourth
//! argument of `$dump.pending`, where the loader evaluates it as a JavaScript expression.
//! [`check`] accepts exactly the JSON texts (RFC 8259) that mean the same as an
//! expression: everything but an object key `__proto__`, which an object literal takes
//! as its prototype (and refuses twice) where `JSON.parse` makes an own property. So the
//! host's text cannot inject code, and the loader gives back what the host saved.

use alloc::string::String;

/// Nesting deeper than this is refused: the loader's parser recurses on it.
const MAX_DEPTH: u32 = 512;

/// Whether `text` is JSON that reads the same as a JavaScript expression; the reason if not.
pub(crate) fn check(text: &str) -> Result<(), &'static str> {
    let mut p = Parser {
        b: text.as_bytes(),
        at: 0,
    };
    p.ws();
    p.value(0)?;
    p.ws();
    if p.at != p.b.len() {
        return Err("the call's state is not JSON (text after the value)");
    }
    Ok(())
}

struct Parser<'a> {
    b: &'a [u8],
    at: usize,
}

const NOT_JSON: &str = "the call's state is not JSON";

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.at).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        let yes = self.peek() == Some(c);
        if yes {
            self.at += 1;
        }
        yes
    }

    fn word(&mut self, w: &[u8]) -> Result<(), &'static str> {
        if self.b[self.at..].starts_with(w) {
            self.at += w.len();
            Ok(())
        } else {
            Err(NOT_JSON)
        }
    }

    fn value(&mut self, depth: u32) -> Result<(), &'static str> {
        if depth > MAX_DEPTH {
            return Err("the call's state nests too deeply");
        }
        match self.peek().ok_or(NOT_JSON)? {
            b'{' => {
                self.at += 1;
                self.ws();
                if self.eat(b'}') {
                    return Ok(());
                }
                loop {
                    self.ws();
                    if self.string()? == "__proto__" {
                        return Err("the call's state has a key __proto__");
                    }
                    self.ws();
                    if !self.eat(b':') {
                        return Err(NOT_JSON);
                    }
                    self.ws();
                    self.value(depth + 1)?;
                    self.ws();
                    if self.eat(b'}') {
                        return Ok(());
                    }
                    if !self.eat(b',') {
                        return Err(NOT_JSON);
                    }
                }
            }
            b'[' => {
                self.at += 1;
                self.ws();
                if self.eat(b']') {
                    return Ok(());
                }
                loop {
                    self.ws();
                    self.value(depth + 1)?;
                    self.ws();
                    if self.eat(b']') {
                        return Ok(());
                    }
                    if !self.eat(b',') {
                        return Err(NOT_JSON);
                    }
                }
            }
            b'"' => self.string().map(drop),
            b't' => self.word(b"true"),
            b'f' => self.word(b"false"),
            b'n' => self.word(b"null"),
            _ => self.number(),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<(), &'static str> {
        self.eat(b'-');
        if !self.eat(b'0') && self.digits() == 0 {
            return Err(NOT_JSON);
        }
        if self.eat(b'.') && self.digits() == 0 {
            return Err(NOT_JSON);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err(NOT_JSON);
            }
        }
        Ok(())
    }

    /// A string; its value only as far as it may be `__proto__` (keys), else empty.
    fn string(&mut self) -> Result<String, &'static str> {
        if !self.eat(b'"') {
            return Err(NOT_JSON);
        }
        let mut s = String::new();
        loop {
            let c = self.peek().ok_or(NOT_JSON)?;
            self.at += 1;
            match c {
                b'"' => return Ok(s),
                0..=0x1f => return Err(NOT_JSON),
                b'\\' => {
                    let e = self.peek().ok_or(NOT_JSON)?;
                    self.at += 1;
                    let ch = match e {
                        b'"' | b'\\' | b'/' => e as char,
                        b'b' | b'f' | b'n' | b'r' | b't' => ' ',
                        b'u' => {
                            let hex = self.b.get(self.at..self.at + 4).ok_or(NOT_JSON)?;
                            let mut n = 0u32;
                            for &d in hex {
                                n = n * 16 + (d as char).to_digit(16).ok_or(NOT_JSON)?;
                            }
                            self.at += 4;
                            // A surrogate cannot be part of `__proto__`.
                            char::from_u32(n).unwrap_or(' ')
                        }
                        _ => return Err(NOT_JSON),
                    };
                    if s.len() < 16 {
                        s.push(ch);
                    }
                }
                // UTF-8 continues as is: `text` is a str.
                _ => {
                    if s.len() < 16 {
                        s.push(c as char);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn accepts_json() {
        for t in [
            "null",
            " {\"due\": 1767225600000, \"ms\": [1, -2.5e3, 0], \"s\": \"a\\\"\\u00e9\\n\\u2028\"} ",
            "[]",
            "{}",
            "\"x\"",
            "true",
            "-0",
            "{\"a\":{\"b\":[{\"__proto_\":1}]}}",
            "\"\u{2028}\"",
        ] {
            assert_eq!(check(t), Ok(()), "{t}");
        }
    }

    #[test]
    fn refuses_what_is_not_json_or_reads_differently() {
        for t in [
            "",
            "{a: 1}",
            "1); evil(",
            "[1,]",
            "01",
            "1.",
            "'x'",
            "undefined",
            "\"\n\"",
            "{\"__proto__\": {}}",
            "{\"a\": {\"\\u005f_proto__\": 1}}",
            "[1] [2]",
            "NaN",
        ] {
            assert!(check(t).is_err(), "{t}");
        }
        assert!(check(&"[".repeat(600)).is_err());
    }
}

/// Through the dumper and the loader: the state is written as `$dump.pending`'s fourth
/// argument and comes back as JSON text; what is not JSON is refused with its call.
#[cfg(test)]
mod round_trip {
    extern crate std;

    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::compiler::CompileOptions;
    use crate::value::Value;
    use crate::vm::dump::DumpOptions;
    use crate::vm::{Status, Vm, VmOptions};

    const LATER: u32 = 1;

    fn base() -> &'static [u8] {
        static BASE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        BASE.get_or_init(|| {
            let mut v = Vm::new(VmOptions::default()).unwrap();
            v.prepare_base_image().unwrap();
            v.bytes().to_vec()
        })
    }

    fn vm() -> Vm {
        // Dumps need frozen builtins (D2836).
        let opts = VmOptions {
            frozen_builtins: true,
            ..VmOptions::default()
        };
        let mut vm = Vm::from_base(base(), opts).unwrap();
        let later = vm.new_async_host_function(LATER, "later").unwrap();
        vm.define_global("later", later).unwrap();
        vm
    }

    fn name(id: u32) -> Option<String> {
        (id == LATER).then(|| "later".into())
    }

    /// A VM waiting on `later(5, "x")`, and the call's id.
    fn waiting() -> (Vm, u32) {
        let mut vm = vm();
        let f = vm
            .compile(
                b"var r = later(5, 'x').then((v) => v + 1);",
                &CompileOptions::script("t.js"),
            )
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        assert!(matches!(vm.run(1_000_000), Status::Idle | Status::Done(_)));
        let id = vm.pending_host_calls()[0];
        (vm, id)
    }

    fn dump(vm: &Vm, states: &[(u32, &str)]) -> Result<String, String> {
        let opts = DumpOptions {
            base: base(),
            host: &["later"],
            host_fn_name: &name,
            call_states: states,
            ..Default::default()
        };
        vm.dump(&opts, None).map_err(|e| e.message())
    }

    /// The value of Script `src` in `vm`, as `display` shows it.
    fn eval(vm: &mut Vm, src: &str) -> String {
        let f = vm
            .compile(src.as_bytes(), &CompileOptions::script("e.js"))
            .unwrap();
        let this = Value::object(vm.global());
        vm.call(f, this, &[]).unwrap();
        match vm.run(1_000_000) {
            Status::Done(v) => vm.display(v),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn the_state_goes_through_a_dump() {
        let (vm, id) = waiting();
        let json = "{ \"due\": 1767225600000, \"s\": \"a\\u2028\\\"\", \"l\": [1, null] }";
        let text = dump(&vm, &[(id, json)]).unwrap();
        // `$djs.resume`'s fourth argument (D2890), JSON data again.
        let call = alloc::format!("$djs.resume({id}, \"later\", [5, \"x\"], ");
        let at = text.find(&call).unwrap_or_else(|| panic!("{text}")) + call.len();
        // (The call may be written where its promise is used, D3001: up to its `)`.)
        let end = at + text[at..].find("})").unwrap() + 1;
        let mut again = self::vm();
        let src = alloc::format!("JSON.stringify({})", &text[at..end]);
        assert_eq!(
            eval(&mut again, &src),
            "{\"due\":1767225600000,\"s\":\"a\u{2028}\\\"\",\"l\":[1,null]}"
        );
        // Without a state: three arguments.
        let text = dump(&vm, &[]).unwrap();
        let call = alloc::format!("$djs.resume({id}, \"later\", [5, \"x\"])");
        assert!(text.contains(&call), "{text}");
    }

    #[test]
    fn states_that_are_not_json_are_refused() {
        let (vm, id) = waiting();
        for (json, why) in [
            ("1); evil(", "is not JSON"),
            ("{\"__proto__\": 1}", "has a key __proto__"),
        ] {
            let e = dump(&vm, &[(id, json)]).unwrap_err();
            assert!(
                e.contains(&alloc::format!("<call {id}>.state: the call's state {why}")),
                "{e}"
            );
        }
        let e = dump(&vm, &[(id + 1, "1")]).unwrap_err();
        assert!(e.contains("a state for a call that is not open"), "{e}");
    }
}
