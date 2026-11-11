//! psh for `wasm32-unknown-unknown`: a plain `extern "C"` ABI over the module's memory,
//! driven by the hand-written JS glue in `js/psh.mjs` (D1110). No wasm-bindgen, no
//! Asyncify or JSPI: the shell is stepped (`psh_shell_step`), and every host operation
//! is a request the JS side answers with `psh_complete`, during the call or later from
//! its event loop.
//!
//! Two modules come from this crate:
//! - the parser module (`--no-default-features`): `psh_parse` and `psh_classify`, JSON
//!   out as `psh --parse` / `psh --classify` print it;
//! - the shell module (default feature `shell`): the same plus the shell exports in
//!   `shell.rs`, over the host in `host.rs`.
//!
//! Data crosses the boundary through two buffers: the input buffer (`psh_in(len)`
//! returns where the JS side writes `len` bytes for the next call) and the output buffer
//! (`psh_out_ptr`/`psh_out_len`, the last call's result).

mod sys;

#[cfg(feature = "shell")]
mod host;
#[cfg(feature = "shell")]
mod ops;
#[cfg(feature = "shell")]
mod shell;

// The JSON of `psh --parse` and `psh --classify`, shared with the psh binary until it
// moves into a crate of its own (0011 follow-up).
#[path = "../../psh/src/classify_json.rs"]
#[allow(dead_code)]
mod classify_json;
#[path = "../../psh/src/json.rs"]
#[allow(dead_code)]
mod json;
#[path = "../../psh/src/parse_json.rs"]
#[allow(dead_code)]
mod parse_json;

use std::cell::RefCell;

/// Bumped when an export, the payload layouts or the `host_call` kinds change; the JS
/// glue checks it.
pub const ABI_VERSION: u32 = 3;

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The bytes the JS side wrote into the input buffer, taken out of it.
pub(crate) fn take_input(len: usize) -> Vec<u8> {
    INPUT.with(|i| {
        let mut buf = std::mem::take(&mut *i.borrow_mut());
        buf.truncate(len);
        buf
    })
}

pub(crate) fn set_output(data: Vec<u8>) {
    OUTPUT.with(|o| *o.borrow_mut() = data);
}

/// Exports: `#[unsafe(no_mangle)]` is what makes them reachable from JS; each is a plain
/// function over integers, sound to call in any order.
macro_rules! export {
    ($(#[$m:meta])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)? $body:block) => {
        $(#[$m])*
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) $(-> $ret)? $body
    };
}
#[allow(unused_imports)]
pub(crate) use export;

export! {
    /// The ABI version the JS glue was written for.
    fn psh_abi_version() -> u32 {
        ABI_VERSION
    }
}

export! {
    /// Makes room for `len` bytes of input and returns where to write them. The buffer
    /// is valid until the next export call.
    fn psh_in(len: usize) -> usize {
        INPUT.with(|i| {
            let mut buf = i.borrow_mut();
            buf.clear();
            buf.resize(len, 0);
            buf.as_mut_ptr() as usize
        })
    }
}

export! {
    /// Where the last result is.
    fn psh_out_ptr() -> usize {
        OUTPUT.with(|o| o.borrow().as_ptr() as usize)
    }
}

export! {
    /// How long the last result is.
    fn psh_out_len() -> usize {
        OUTPUT.with(|o| o.borrow().len())
    }
}

export! {
    /// Reports panics to the JS side (`K_PANIC`) before the module traps.
    fn psh_init() {
        std::panic::set_hook(Box::new(|info| {
            let msg = info.to_string();
            sys::call(sys::K_PANIC, 0, 0, msg.as_bytes());
        }));
    }
}

/// `flags` of `psh_parse`: `shopt -s extglob`.
const PARSE_EXTGLOB: u32 = 1;
/// `flags` of `psh_parse` and `psh_classify`: compact JSON (else pretty).
const COMPACT: u32 = 2;
/// `flags` of `psh_parse`: the outline rows (`psh_syntax::outline`) instead of the tree.
const PARSE_OUTLINE: u32 = 4;

fn json_out(json: &json::Json, flags: u32) {
    let text = if flags & COMPACT != 0 {
        json.to_compact()
    } else {
        json.to_pretty()
    };
    set_output(text.into_bytes());
}

export! {
    /// Parses the `len` input bytes; the output is `psh --parse`'s JSON, or with
    /// `PARSE_OUTLINE` the outline's rows. Returns 1 when the script parses, 0 when not.
    fn psh_parse(len: usize, flags: u32) -> u32 {
        let source = take_input(len);
        let opts = psh_syntax::Options {
            extglob: flags & PARSE_EXTGLOB != 0,
        };
        let (json, ok) = if flags & PARSE_OUTLINE != 0 {
            parse_json::outline_to_json(&source, opts)
        } else {
            parse_json::parse_to_json(&source, opts)
        };
        json_out(&json, flags);
        u32::from(ok)
    }
}

export! {
    /// Classifies the `len` input bytes (UTF-8; invalid bytes become U+FFFD); the output
    /// is `psh --classify`'s JSON. Returns 1 when the script parses, 0 when not.
    fn psh_classify(len: usize, flags: u32) -> u32 {
        let text = String::from_utf8_lossy(&take_input(len)).into_owned();
        let c = psh_classify::classify(&text);
        let ok = c.error.is_none();
        json_out(&classify_json::to_json(&c, &text), flags);
        u32::from(ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(data: &[u8]) -> usize {
        let ptr = psh_in(data.len());
        INPUT.with(|i| i.borrow_mut().copy_from_slice(data));
        let _ = ptr;
        data.len()
    }

    fn output() -> String {
        OUTPUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    #[test]
    fn parse_and_classify() {
        let len = input(b"echo hi | wc -l");
        assert_eq!(psh_parse(len, COMPACT), 1);
        assert!(output().starts_with("{\"ok\":true"), "{}", output());
        let len = input(b"if then");
        assert_eq!(psh_parse(len, COMPACT), 0);
        assert!(output().contains("\"ok\":false"));
        let len = input(b"echo hi");
        assert_eq!(psh_parse(len, COMPACT | PARSE_OUTLINE), 1);
        assert!(
            output().starts_with(r#"{"ok":true,"rows":[{"depth":0,"role":"","kind":"simple""#),
            "{}",
            output()
        );
        let len = input(b"rm -rf /tmp/x");
        assert_eq!(psh_classify(len, COMPACT), 1);
        assert!(output().contains("rm"), "{}", output());
    }
}
