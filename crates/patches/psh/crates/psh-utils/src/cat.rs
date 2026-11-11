//! `cat` with GNU's options (`-A -b -e -E -n -s -t -T -u -v`); BSD's `-e -n -s -t -u -v`
//! are the same letters. Written for psh after GNU's documented behavior.

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag};

pub const SPEC: Spec = Spec {
    name: "cat",
    opts: &[
        both('A', "show-all"),
        both('b', "number-nonblank"),
        flag('e'),
        both('E', "show-ends"),
        both('n', "number"),
        both('s', "squeeze-blank"),
        flag('t'),
        both('T', "show-tabs"),
        flag('u'),
        both('v', "show-nonprinting"),
    ],
    posix_order: false,
};

#[derive(Default)]
struct Style {
    number: bool,
    nonblank: bool,
    ends: bool,
    tabs: bool,
    nonprinting: bool,
    squeeze: bool,
}

impl Style {
    fn plain(&self) -> bool {
        !(self.number
            || self.nonblank
            || self.ends
            || self.tabs
            || self.nonprinting
            || self.squeeze)
    }
}

/// Line state that carries across files, as in GNU: numbering and blank squeezing
/// continue from one file to the next.
#[derive(Default)]
struct State {
    line: u64,
    at_line_start: bool,
    blanks: u32,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut st = Style::default();
    for (key, _) in p.iter(&SPEC) {
        match key.as_str() {
            "show-all" => (st.nonprinting, st.ends, st.tabs) = (true, true, true),
            "number-nonblank" => st.nonblank = true,
            "e" => (st.nonprinting, st.ends) = (true, true),
            "show-ends" => st.ends = true,
            "number" => st.number = true,
            "squeeze-blank" => st.squeeze = true,
            "t" => (st.nonprinting, st.tabs) = (true, true),
            "show-tabs" => st.tabs = true,
            "show-nonprinting" => st.nonprinting = true,
            _ => {}
        }
    }
    if st.nonblank {
        st.number = false;
    }
    let mut state = State {
        at_line_start: true,
        ..State::default()
    };
    let mut status = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for name in common::operands_or_stdin(&p) {
        let input = match Input::open(ctx, &name).await {
            Ok(i) => i,
            Err(e) => {
                common::file_error(ctx, &name, &e).await;
                status = 1;
                continue;
            }
        };
        loop {
            let n = match input.read(ctx, &mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    common::file_error(ctx, &name, &e).await;
                    status = 1;
                    break;
                }
            };
            let out = if st.plain() {
                buf[..n].to_vec()
            } else {
                format(&st, &mut state, &buf[..n])
            };
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
            ctx.check().await;
        }
    }
    status
}

fn format(st: &Style, state: &mut State, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 4);
    for &b in data {
        if state.at_line_start {
            if b == b'\n' {
                state.blanks += 1;
                if st.squeeze && state.blanks > 1 {
                    continue;
                }
            } else {
                state.blanks = 0;
            }
            if st.number || (st.nonblank && b != b'\n') {
                state.line += 1;
                out.extend_from_slice(format!("{:>6}\t", state.line).as_bytes());
            }
            state.at_line_start = false;
        }
        match b {
            b'\n' => {
                if st.ends {
                    out.push(b'$');
                }
                out.push(b'\n');
                state.at_line_start = true;
            }
            b'\t' if st.tabs => out.extend_from_slice(b"^I"),
            b'\t' => out.push(b),
            _ if st.nonprinting => nonprinting(&mut out, b),
            _ => out.push(b),
        }
    }
    out
}

/// `-v`: control characters as `^X`, DEL as `^?`, bytes above 127 as `M-` and the same.
fn nonprinting(out: &mut Vec<u8>, b: u8) {
    let mut c = b;
    if c >= 128 {
        out.extend_from_slice(b"M-");
        c -= 128;
    }
    match c {
        0..=31 => {
            out.push(b'^');
            out.push(c + 64);
        }
        127 => out.extend_from_slice(b"^?"),
        _ => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_all() {
        let st = Style {
            nonprinting: true,
            ends: true,
            tabs: true,
            ..Style::default()
        };
        let mut s = State {
            at_line_start: true,
            ..State::default()
        };
        assert_eq!(format(&st, &mut s, b"a\tb\x01\x80\r\n"), b"a^Ib^AM-^@^M$\n");
    }

    #[test]
    fn number_and_squeeze_across_chunks() {
        let st = Style {
            number: true,
            squeeze: true,
            ..Style::default()
        };
        let mut s = State {
            at_line_start: true,
            ..State::default()
        };
        let mut out = format(&st, &mut s, b"a\n\n");
        out.extend(format(&st, &mut s, b"\n\nb"));
        assert_eq!(out, b"     1\ta\n     2\t\n     3\tb");
    }
}
