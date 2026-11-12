//! `base64 [-d|-D|--decode] [-w COLS|-b COLS] [-i FILE|--ignore-garbage] [-o FILE]
//! [FILE]`: the union of GNU's and BSD's flags (D016). Encoding wraps at 76 columns as
//! GNU does (`-w 0` / `-b 0`: one line, which GNU leaves without a newline). `-i`
//! followed by a word that is not an option is BSD's input file, else GNU's
//! `--ignore-garbage` (ARCH.md "Dialect"). Decoding follows GNU: newlines are skipped,
//! padding may end a group mid-stream, other bytes are invalid input (status 1, after
//! what decoded so far). Written for psh after RFC 4648 and the two manuals.

use psh_engine::host::OpenOptions;
use psh_engine::util::{UtilCtx, UtilFile};

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "base64",
    opts: &[
        both('d', "decode"),
        flag('D'),
        both('i', "input").value(),
        long("ignore-garbage"),
        both('w', "wrap").value(),
        both('b', "break").value(),
        both('o', "output").value(),
    ],
    posix_order: false,
};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `-i` as GNU's ignore-garbage when no input file follows it: alone (or ending a
/// cluster) before an option or at the end.
fn rewrite_i(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = vec![argv[0].clone()];
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if a == b"--" {
            out.extend_from_slice(&argv[i - 1..]);
            break;
        }
        let cluster = a.len() >= 2 && a[0] == b'-' && a[1] != b'-';
        // Only a cluster whose letters are all flags can end in a bare `-i`.
        if cluster
            && a.ends_with(b"i")
            && a[1..a.len() - 1].iter().all(|c| b"dD".contains(c))
            && argv.get(i).is_none_or(|n| n.len() > 1 && n[0] == b'-')
        {
            if a.len() > 2 {
                out.push(a[..a.len() - 1].to_vec());
            }
            out.push(b"--ignore-garbage".to_vec());
            continue;
        }
        out.push(a.clone());
    }
    out
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let args = rewrite_i(argv);
    let p = match common::parse(ctx, &SPEC, &args).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let decode = p.has(&SPEC, "decode") || p.has(&SPEC, "D");
    let garbage = p.has(&SPEC, "ignore-garbage");
    let mut wrap = 76usize;
    for (k, v) in p.iter(&SPEC) {
        if k == "wrap" || k == "break" {
            let v = v.unwrap_or_default();
            match std::str::from_utf8(v)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            {
                Some(n) => wrap = n,
                None => {
                    let q = common::quote(ctx, v);
                    ctx.error(&format!("invalid wrap size: {q}")).await;
                    return 1;
                }
            }
        }
    }
    let mut input_name = p.last(&SPEC, "input").map(<[u8]>::to_vec);
    match (p.operands.as_slice(), &input_name) {
        ([], _) => {}
        ([one], None) => input_name = Some(one.clone()),
        (ops, _) => {
            let extra = if input_name.is_some() {
                &ops[0]
            } else {
                &ops[1]
            };
            let q = common::quote(ctx, extra);
            return common::usage(ctx, &format!("extra operand {q}")).await;
        }
    }
    let name = input_name.unwrap_or_else(|| b"-".to_vec());
    let input = match Input::open(ctx, &name).await {
        Ok(i) => i,
        Err(e) => {
            common::file_error(ctx, &name, &e).await;
            return 1;
        }
    };
    let output = match p.last(&SPEC, "output") {
        Some(o) if o != b"-" => match ctx.open(o, OpenOptions::write()).await {
            Ok(f) => Some((o.to_vec(), f)),
            Err(e) => {
                common::file_error(ctx, o, &e).await;
                return 1;
            }
        },
        _ => None,
    };
    let mut out = Output { file: output };
    let r = if decode {
        run_decode(ctx, &input, &mut out, garbage).await
    } else {
        run_encode(ctx, &input, &mut out, wrap).await
    };
    match r {
        Ok(st) => st,
        Err(Failure::Read(e)) => {
            common::file_error(ctx, &name, &e).await;
            1
        }
        Err(Failure::Write(e)) => match &out.file {
            Some((n, _)) => {
                common::file_error(ctx, &n.clone(), &e).await;
                1
            }
            None => common::write_failed(ctx, &e).await,
        },
    }
}

enum Failure {
    Read(std::io::Error),
    Write(std::io::Error),
}

/// stdout or the `-o` file.
struct Output {
    file: Option<(Vec<u8>, UtilFile)>,
}

impl Output {
    async fn write(&mut self, ctx: &mut UtilCtx<'_>, data: &[u8]) -> Result<(), Failure> {
        let r = match &self.file {
            Some((_, f)) => f.write_all(data).await,
            None => ctx.write(data).await,
        };
        r.map_err(Failure::Write)
    }
}

async fn run_encode(
    ctx: &mut UtilCtx<'_>,
    input: &Input,
    out: &mut Output,
    wrap: usize,
) -> Result<i32, Failure> {
    let mut buf = vec![0u8; 48 * 1024];
    // Bytes not yet encoded (fewer than 3), and the current line's length.
    let mut carry: Vec<u8> = Vec::new();
    let mut col = 0usize;
    let mut any = false;
    loop {
        let n = input.read(ctx, &mut buf).await.map_err(Failure::Read)?;
        let eof = n == 0;
        carry.extend_from_slice(&buf[..n]);
        let whole = if eof {
            carry.len()
        } else {
            carry.len() / 3 * 3
        };
        let full = whole / 3 * 3;
        let mut syms = Vec::with_capacity(whole / 3 * 4 + 4);
        for c in carry[..full].as_chunks::<3>().0 {
            let n = u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]);
            syms.extend_from_slice(&[
                ALPHABET[(n >> 18) as usize & 63],
                ALPHABET[(n >> 12) as usize & 63],
                ALPHABET[(n >> 6) as usize & 63],
                ALPHABET[n as usize & 63],
            ]);
        }
        if whole > full {
            let c = &carry[full..whole];
            let b1 = c.get(1).copied().unwrap_or(0);
            syms.push(ALPHABET[usize::from(c[0] >> 2)]);
            syms.push(ALPHABET[usize::from((c[0] & 3) << 4 | b1 >> 4)]);
            syms.push(if c.len() > 1 {
                ALPHABET[usize::from((b1 & 15) << 2)]
            } else {
                b'='
            });
            syms.push(b'=');
        }
        let mut text = if let Some(lines) = syms.len().checked_div(wrap) {
            let mut text = Vec::with_capacity(syms.len() + lines + 2);
            let mut rest = &syms[..];
            while !rest.is_empty() {
                if col == wrap {
                    text.push(b'\n');
                    col = 0;
                }
                let take = (wrap - col).min(rest.len());
                text.extend_from_slice(&rest[..take]);
                col += take;
                rest = &rest[take..];
            }
            text
        } else {
            syms
        };
        carry.drain(..whole);
        any |= !text.is_empty();
        if eof && any && wrap > 0 {
            text.push(b'\n');
        }
        out.write(ctx, &text).await?;
        if eof {
            return Ok(0);
        }
        ctx.check().await;
    }
}

const DECODE: [u8; 256] = {
    let mut t = [0xff; 256];
    let mut i = 0;
    while i < 64 {
        t[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    t
};

fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// A decoder for one group of four symbols at a time.
#[derive(Default)]
struct Decoder {
    group: [u8; 4],
    len: usize,
    /// `=` seen in this group: only more `=` may follow until it is complete.
    pad: usize,
}

impl Decoder {
    /// Feeds one byte; `false` for invalid input.
    fn push(&mut self, c: u8, out: &mut Vec<u8>) -> bool {
        if c == b'=' {
            if self.len < 2 {
                return false;
            }
            self.pad += 1;
            self.len += 1;
        } else {
            let Some(v) = value(c) else {
                return false;
            };
            if self.pad > 0 {
                return false;
            }
            self.group[self.len] = v;
            self.len += 1;
        }
        if self.len == 4 {
            self.flush(out);
        }
        true
    }

    /// Emits the bytes the symbols so far make, ending the group.
    fn flush(&mut self, out: &mut Vec<u8>) {
        let g = self.group;
        let n = self.len - self.pad;
        if n >= 2 {
            out.push(g[0] << 2 | g[1] >> 4);
        }
        if n >= 3 {
            out.push(g[1] << 4 | g[2] >> 2);
        }
        if n >= 4 {
            out.push(g[2] << 6 | g[3]);
        }
        *self = Decoder::default();
    }
}

async fn run_decode(
    ctx: &mut UtilCtx<'_>,
    input: &Input,
    out: &mut Output,
    garbage: bool,
) -> Result<i32, Failure> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut d = Decoder::default();
    loop {
        let n = input.read(ctx, &mut buf).await.map_err(Failure::Read)?;
        let mut bytes = Vec::with_capacity(n / 4 * 3 + 3);
        if n == 0 {
            // A trailing group of two or three symbols decodes without padding; one
            // symbol alone cannot.
            let ok = d.len - d.pad != 1;
            if d.len > 0 {
                d.flush(&mut bytes);
            }
            out.write(ctx, &bytes).await?;
            if !ok {
                ctx.error("invalid input").await;
                return Ok(1);
            }
            return Ok(0);
        }
        let data = &buf[..n];
        let mut i = 0;
        while i < data.len() {
            if d.len == 0
                && let Some(g) = data.get(i..i + 4)
            {
                let v: [u8; 4] = std::array::from_fn(|k| DECODE[usize::from(g[k])]);
                if (v[0] | v[1] | v[2] | v[3]) < 64 {
                    bytes.extend_from_slice(&[
                        v[0] << 2 | v[1] >> 4,
                        v[1] << 4 | v[2] >> 2,
                        v[2] << 6 | v[3],
                    ]);
                    i += 4;
                    continue;
                }
            }
            let c = data[i];
            i += 1;
            if c == b'\n' {
                continue;
            }
            if garbage && c != b'=' && value(c).is_none() {
                continue;
            }
            if !d.push(c, &mut bytes) {
                if d.len > 0 {
                    d.flush(&mut bytes);
                }
                out.write(ctx, &bytes).await?;
                ctx.error("invalid input").await;
                return Ok(1);
            }
        }
        out.write(ctx, &bytes).await?;
        ctx.check().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(s: &[u8]) -> (Vec<u8>, bool) {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        for &c in s {
            if !d.push(c, &mut out) {
                d.flush(&mut out);
                return (out, false);
            }
        }
        let ok = d.len - d.pad != 1;
        if d.len > 0 {
            d.flush(&mut out);
        }
        (out, ok)
    }

    /// RFC 4648 §10.
    #[test]
    fn vectors() {
        for (plain, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(decode(enc.as_bytes()), (plain.as_bytes().to_vec(), true));
        }
        assert_eq!(decode(b"aGk"), (b"hi".to_vec(), true));
        assert_eq!(decode(b"aGk=aGk="), (b"hihi".to_vec(), true));
        assert_eq!(decode(b"aGV!"), (b"he".to_vec(), false));
        assert_eq!(decode(b"a"), (b"".to_vec(), false));
        assert_eq!(decode(b"a=="), (b"".to_vec(), false));
    }

    #[test]
    fn dash_i() {
        let v = |a: &[&str]| -> Vec<String> {
            let argv: Vec<Vec<u8>> = a.iter().map(|s| s.as_bytes().to_vec()).collect();
            rewrite_i(&argv)
                .into_iter()
                .map(|x| String::from_utf8(x).unwrap())
                .collect()
        };
        assert_eq!(v(&["base64", "-i", "f"]), ["base64", "-i", "f"]);
        assert_eq!(
            v(&["base64", "-d", "-i"]),
            ["base64", "-d", "--ignore-garbage"]
        );
        assert_eq!(v(&["base64", "-di"]), ["base64", "-d", "--ignore-garbage"]);
        assert_eq!(v(&["base64", "-di", "f"]), ["base64", "-di", "f"]);
        assert_eq!(
            v(&["base64", "-i", "-d"]),
            ["base64", "--ignore-garbage", "-d"]
        );
    }
}
