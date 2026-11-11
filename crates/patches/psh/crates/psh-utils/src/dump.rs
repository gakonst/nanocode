//! Bytes as text (nanocode's addition): xxd (as vim's), od (GNU), hexdump (-C and the
//! default words), strings (binutils) and base32 (GNU). Inputs are read whole.

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};

async fn slurp(ctx: &mut UtilCtx<'_>, names: &[Vec<u8>]) -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    let stdin = [b"-".to_vec()];
    let names = if names.is_empty() { &stdin[..] } else { names };
    for name in names {
        let read = match Input::open(ctx, name).await {
            Ok(input) => input.read_to_end(ctx, &mut data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            common::file_error(ctx, name, &e).await;
            return Err(1);
        }
    }
    Ok(data)
}

async fn emit(ctx: &mut UtilCtx<'_>, out: &[u8]) -> i32 {
    match ctx.write(out).await {
        Ok(()) => 0,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

fn number(text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?.trim();
    let (digits, radix) = match text.strip_prefix("0x").or(text.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None if text.len() > 1 && text.starts_with('0') => (&text[1..], 8),
        None => (text, 10),
    };
    let (digits, mult) = match digits.as_bytes().last() {
        Some(b'k' | b'K') if radix != 16 => (&digits[..digits.len() - 1], 1024),
        Some(b'm' | b'M') if radix != 16 => (&digits[..digits.len() - 1], 1024 * 1024),
        Some(b'b') if radix != 16 => (&digits[..digits.len() - 1], 512),
        _ => (digits, 1),
    };
    u64::from_str_radix(digits, radix).ok()?.checked_mul(mult)
}

fn printable(b: u8) -> u8 {
    if (0x20..0x7f).contains(&b) { b } else { b'.' }
}

pub async fn xxd(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let (mut plain, mut reverse, mut bits, mut include, mut upper) =
        (false, false, false, false, false);
    let mut cols: Option<usize> = None;
    let mut group: Option<usize> = None;
    let mut len: Option<u64> = None;
    let mut seek = 0u64;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            files.push(arg.clone());
            continue;
        }
        let name = String::from_utf8_lossy(arg).into_owned();
        let (flag, attached) = name.split_at(2.min(name.len()));
        let value = |i: &mut usize| -> Option<u64> {
            if !attached.is_empty() {
                return number(attached.as_bytes());
            }
            *i += 1;
            argv.get(*i - 1).and_then(|v| number(v))
        };
        match flag {
            "-p" => plain = true,
            "-r" => reverse = true,
            "-b" => bits = true,
            "-i" => include = true,
            "-u" => upper = true,
            "-a" | "-E" => {}
            "-c" => cols = value(&mut i).map(|n| n as usize),
            "-g" => group = value(&mut i).map(|n| n as usize),
            "-l" => len = value(&mut i),
            "-s" => seek = value(&mut i).unwrap_or(0),
            _ if name == "-ps" || name == "-postscript" || name == "-plain" => plain = true,
            _ if name == "-rp" || name == "-pr" => {
                plain = true;
                reverse = true;
            }
            _ => {
                if let Some(status) = ctx.run_program(argv.to_vec()).await {
                    return status;
                }
                ctx.error(&format!("unsupported option '{name}' (psh built-in xxd)"))
                    .await;
                return 2;
            }
        }
    }
    let input: Vec<Vec<u8>> = files.first().cloned().into_iter().collect();
    let mut data = match slurp(ctx, &input).await {
        Ok(data) => data,
        Err(status) => return status,
    };
    let mut out = Vec::new();
    if reverse {
        if plain {
            let digits: Vec<u8> = data.iter().copied().filter(u8::is_ascii_hexdigit).collect();
            for pair in digits.chunks_exact(2) {
                let text = std::str::from_utf8(pair).unwrap_or("00");
                out.push(u8::from_str_radix(text, 16).unwrap_or(0));
            }
        } else {
            for line in data.split(|&b| b == b'\n') {
                let Some(colon) = line.iter().position(|&b| b == b':') else {
                    continue;
                };
                let Some(at) = std::str::from_utf8(&line[..colon])
                    .ok()
                    .and_then(|o| usize::from_str_radix(o.trim(), 16).ok())
                else {
                    continue;
                };
                if out.len() < at {
                    out.resize(at, 0);
                }
                // The hex ends where two spaces start the text.
                let body = &line[colon + 1..];
                let end = body
                    .windows(2)
                    .position(|w| w == b"  ")
                    .unwrap_or(body.len());
                let digits: Vec<u8> = body[..end]
                    .iter()
                    .copied()
                    .filter(u8::is_ascii_hexdigit)
                    .collect();
                for (k, pair) in digits.chunks_exact(2).enumerate() {
                    let text = std::str::from_utf8(pair).unwrap_or("00");
                    let byte = u8::from_str_radix(text, 16).unwrap_or(0);
                    if at + k < out.len() {
                        out[at + k] = byte;
                    } else {
                        out.push(byte);
                    }
                }
            }
        }
    } else {
        let start = (seek as usize).min(data.len());
        data.drain(..start);
        if let Some(len) = len {
            data.truncate(len as usize);
        }
        let hex = |b: u8| {
            if upper {
                format!("{b:02X}")
            } else {
                format!("{b:02x}")
            }
        };
        if include {
            let name = files.first().map(|f| {
                let mut name: String = String::from_utf8_lossy(f)
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                if name.starts_with(|c: char| c.is_ascii_digit()) {
                    name.insert_str(0, "__");
                }
                name
            });
            if let Some(name) = &name {
                out.extend_from_slice(format!("unsigned char {name}[] = {{\n").as_bytes());
            }
            let per = cols.unwrap_or(12).max(1);
            for (n, chunk) in data.chunks(per).enumerate() {
                out.extend_from_slice(b"  ");
                let items: Vec<String> = chunk.iter().map(|&b| format!("0x{}", hex(b))).collect();
                out.extend_from_slice(items.join(", ").as_bytes());
                if (n + 1) * per < data.len() {
                    out.push(b',');
                }
                out.push(b'\n');
            }
            if let Some(name) = &name {
                out.extend_from_slice(
                    format!("}};\nunsigned int {name}_len = {};\n", data.len()).as_bytes(),
                );
            }
        } else if plain {
            let per = cols.unwrap_or(30);
            if per == 0 {
                for &b in &data {
                    out.extend_from_slice(hex(b).as_bytes());
                }
                out.push(b'\n');
            } else {
                for chunk in data.chunks(per) {
                    for &b in chunk {
                        out.extend_from_slice(hex(b).as_bytes());
                    }
                    out.push(b'\n');
                }
            }
        } else {
            let per = cols.unwrap_or(if bits { 6 } else { 16 }).max(1);
            let group = group.unwrap_or(if bits { 1 } else { 2 });
            let digits = if bits { 8 } else { 2 };
            let groups = if group == 0 { 1 } else { per.div_ceil(group) };
            let width = per * digits + groups;
            let digit: &[u8; 16] = if upper {
                b"0123456789ABCDEF"
            } else {
                b"0123456789abcdef"
            };
            out.reserve(data.len() / per * (width + 12 + per));
            for (n, chunk) in data.chunks(per).enumerate() {
                out.extend_from_slice(format!("{:08x}:", seek as usize + n * per).as_bytes());
                let before = out.len();
                for (k, &b) in chunk.iter().enumerate() {
                    if (group == 0 && k == 0) || (group != 0 && k % group == 0) {
                        out.push(b' ');
                    }
                    if bits {
                        out.extend((0..8).rev().map(|bit| b'0' + (b >> bit & 1)));
                    } else {
                        out.push(digit[(b >> 4) as usize]);
                        out.push(digit[(b & 15) as usize]);
                    }
                }
                out.resize(before + width + 2, b' ');
                out.extend(chunk.iter().map(|&b| printable(b)));
                out.push(b'\n');
            }
        }
    }
    if let Some(target) = files.get(1) {
        let written = match ctx.open(target, OpenOptions::write()).await {
            Ok(file) => file.write_all(&out).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            ctx.error(&format!(
                "{}: {}",
                String::from_utf8_lossy(target),
                error_text(&e)
            ))
            .await;
            return 1;
        }
        return 0;
    }
    emit(ctx, &out).await
}

/// One od type: how many bytes a value takes, how wide it prints and how.
#[derive(Clone, Copy)]
struct OdType {
    kind: u8,
    size: usize,
    width: usize,
    text: bool,
}

fn od_type(kind: u8, size: usize) -> Option<OdType> {
    let width = match (kind, size) {
        (b'o', 1) => 3,
        (b'o', 2) => 6,
        (b'o', 4) => 11,
        (b'o', 8) => 22,
        (b'x', n @ (1 | 2 | 4 | 8)) => n * 2,
        (b'd', 1) => 4,
        (b'd', 2) => 6,
        (b'd', 4) => 11,
        (b'd', 8) => 20,
        (b'u', 1) => 3,
        (b'u', 2) => 5,
        (b'u', 4) => 10,
        (b'u', 8) => 20,
        (b'c' | b'a', 1) => 3,
        (b'f', 4) => 15,
        (b'f', 8) => 24,
        _ => return None,
    };
    Some(OdType {
        kind,
        size,
        width,
        text: false,
    })
}

fn od_types(spec: &[u8]) -> Option<Vec<OdType>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spec.len() {
        let kind = spec[i];
        i += 1;
        let mut size = match kind {
            b'c' | b'a' => 1,
            b'f' => 8,
            b'o' | b'x' | b'd' | b'u' => 4,
            _ => return None,
        };
        let digits = spec[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0 {
            size = std::str::from_utf8(&spec[i..i + digits])
                .ok()?
                .parse()
                .ok()?;
            i += digits;
        } else if let Some(letter) = spec.get(i) {
            let named = match (kind, letter) {
                (b'f', b'F') => Some(4),
                (b'f', b'D') => Some(8),
                (b'f', _) => None,
                (_, b'C') => Some(1),
                (_, b'S') => Some(2),
                (_, b'I') => Some(4),
                (_, b'L') => Some(8),
                _ => None,
            };
            if let Some(named) = named {
                size = named;
                i += 1;
            }
        }
        let mut t = od_type(kind, size)?;
        if spec.get(i) == Some(&b'z') {
            t.text = true;
            i += 1;
        }
        out.push(t);
    }
    Some(out)
}

fn od_value(t: &OdType, bytes: &[u8]) -> String {
    let mut wide = [0u8; 8];
    wide[..bytes.len()].copy_from_slice(bytes);
    let v = u64::from_le_bytes(wide);
    match t.kind {
        b'o' => format!("{v:0w$o}", w = t.width),
        b'x' => format!("{v:0w$x}", w = t.width),
        b'u' => v.to_string(),
        b'd' => {
            let shift = 64 - 8 * t.size as u32;
            (((v << shift) as i64) >> shift).to_string()
        }
        b'f' if t.size == 4 => {
            let f = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            format!("{f:e}")
        }
        b'f' => format!("{:e}", f64::from_le_bytes(wide)),
        b'a' => {
            const NAMES: [&str; 33] = [
                "nul", "soh", "stx", "etx", "eot", "enq", "ack", "bel", "bs", "ht", "nl", "vt",
                "ff", "cr", "so", "si", "dle", "dc1", "dc2", "dc3", "dc4", "nak", "syn", "etb",
                "can", "em", "sub", "esc", "fs", "gs", "rs", "us", "sp",
            ];
            let b = bytes[0] & 0x7f;
            match b {
                0..=32 => NAMES[b as usize].to_string(),
                127 => "del".to_string(),
                _ => (b as char).to_string(),
            }
        }
        _ => match bytes[0] {
            0 => "\\0".to_string(),
            7 => "\\a".to_string(),
            8 => "\\b".to_string(),
            9 => "\\t".to_string(),
            10 => "\\n".to_string(),
            11 => "\\v".to_string(),
            12 => "\\f".to_string(),
            13 => "\\r".to_string(),
            b @ 0x20..=0x7e => (b as char).to_string(),
            b => format!("{b:03o}"),
        },
    }
}

pub async fn od(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut radix = b'o';
    let mut types: Vec<OdType> = Vec::new();
    let mut skip = 0u64;
    let mut count: Option<u64> = None;
    let mut width = 16usize;
    let mut all = false;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    let bad = |what: String| what;
    let mut problem: Option<String> = None;
    while i < argv.len() {
        let arg = argv[i].clone();
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' || arg == b"--" {
            if arg != b"--" {
                files.push(arg);
            }
            continue;
        }
        let text = String::from_utf8_lossy(&arg).into_owned();
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = long.split_once('=').unwrap_or((long, ""));
            match name {
                "address-radix" => radix = value.bytes().next().unwrap_or(b'o'),
                "format" => match od_types(value.as_bytes()) {
                    Some(t) => types.extend(t),
                    None => problem = Some(bad(format!("invalid type string '{value}'"))),
                },
                "skip-bytes" => skip = number(value.as_bytes()).unwrap_or(0),
                "read-bytes" => count = number(value.as_bytes()),
                "width" => width = number(value.as_bytes()).unwrap_or(32) as usize,
                "output-duplicates" => all = true,
                _ => problem = Some(bad(format!("unrecognized option '{text}'"))),
            }
            continue;
        }
        let bytes = text.as_bytes();
        let mut k = 1;
        while k < bytes.len() {
            let c = bytes[k];
            k += 1;
            let mut value = |i: &mut usize| -> Vec<u8> {
                if k < bytes.len() {
                    let v = bytes[k..].to_vec();
                    k = bytes.len();
                    return v;
                }
                *i += 1;
                argv.get(*i - 1).cloned().unwrap_or_default()
            };
            let mut short = |spec: &[u8]| types.extend(od_types(spec).unwrap_or_default());
            match c {
                b'A' => radix = value(&mut i).first().copied().unwrap_or(b'o'),
                b't' => {
                    let v = value(&mut i);
                    match od_types(&v) {
                        Some(t) => types.extend(t),
                        None => {
                            problem = Some(bad(format!(
                                "invalid type string '{}'",
                                String::from_utf8_lossy(&v)
                            )));
                        }
                    }
                }
                b'j' => skip = number(&value(&mut i)).unwrap_or(0),
                b'N' => count = number(&value(&mut i)),
                b'w' => {
                    let attached = bytes[k..].to_vec();
                    k = bytes.len();
                    width = if attached.is_empty() {
                        32
                    } else {
                        number(&attached).unwrap_or(32) as usize
                    };
                }
                b'v' => all = true,
                b'a' => short(b"a"),
                b'b' => short(b"o1"),
                b'c' => short(b"c"),
                b'd' => short(b"u2"),
                b'f' => short(b"fF"),
                b'i' => short(b"dI"),
                b'l' => short(b"dL"),
                b'o' => short(b"o2"),
                b's' => short(b"d2"),
                b'x' => short(b"x2"),
                _ => problem = Some(bad(format!("invalid option -- '{}'", c as char))),
            }
        }
    }
    if let Some(what) = problem {
        return common::usage(ctx, &what).await;
    }
    if types.is_empty() {
        types = od_types(b"o2").unwrap_or_default();
    }
    let mut data = match slurp(ctx, &files).await {
        Ok(data) => data,
        Err(status) => return status,
    };
    let start = (skip as usize).min(data.len());
    data.drain(..start);
    if let Some(count) = count {
        data.truncate(count as usize);
    }
    let width = width.max(1);
    // Every type's line is as wide as the widest.
    let line_width = types
        .iter()
        .map(|t| (width / t.size) * (t.width + 1))
        .max()
        .unwrap_or(0);
    let offset = |at: usize| -> String {
        match radix {
            b'd' => format!("{at:07}"),
            b'x' => format!("{at:06x}"),
            b'n' => String::new(),
            _ => format!("{at:07o}"),
        }
    };
    let mut out = Vec::new();
    let mut previous: Option<&[u8]> = None;
    let mut starred = false;
    let mut at = 0;
    for chunk in data.chunks(width) {
        if !all && chunk.len() == width && previous == Some(chunk) {
            if !starred {
                out.extend_from_slice(b"*\n");
                starred = true;
            }
            at += chunk.len();
            continue;
        }
        starred = false;
        previous = Some(chunk);
        for (n, t) in types.iter().enumerate() {
            let mut line = if n == 0 {
                offset(skip as usize + at)
            } else {
                " ".repeat(offset(0).len())
            };
            let fields = width / t.size;
            let pad = line_width - fields * (t.width + 1);
            let mut remaining = pad;
            let present = chunk.len().div_ceil(t.size);
            for (j, piece) in chunk.chunks(t.size).enumerate() {
                let mut padded = piece.to_vec();
                padded.resize(t.size, 0);
                let next = pad * (fields - 1 - j) / fields;
                let field = remaining - next + t.width;
                remaining = next;
                // The common types without a string of their own for each value.
                if field == t.width && t.size == 1 && matches!(t.kind, b'c' | b'x' | b'o') {
                    let b = padded[0];
                    match t.kind {
                        b'x' => {
                            line.push(' ');
                            line.push(char::from(b"0123456789abcdef"[(b >> 4) as usize]));
                            line.push(char::from(b"0123456789abcdef"[(b & 15) as usize]));
                            continue;
                        }
                        b'o' => {
                            line.push(' ');
                            line.push(char::from(b'0' + (b >> 6)));
                            line.push(char::from(b'0' + (b >> 3 & 7)));
                            line.push(char::from(b'0' + (b & 7)));
                            continue;
                        }
                        _ if (0x20..0x7f).contains(&b) => {
                            line.push_str("   ");
                            line.push(char::from(b));
                            continue;
                        }
                        _ => {}
                    }
                }
                line.push_str(&format!(" {:>field$}", od_value(t, &padded)));
            }
            if t.text {
                let missing = fields - present;
                line.push_str(&" ".repeat(missing * (t.width + 1) + remaining));
                line.push_str("  >");
                line.extend(chunk.iter().map(|&b| printable(b) as char));
                line.push('<');
            }
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
        }
        at += chunk.len();
    }
    if radix != b'n' {
        out.extend_from_slice(offset(skip as usize + at).as_bytes());
        out.push(b'\n');
    }
    emit(ctx, &out).await
}

pub async fn hexdump(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut mode = b'w';
    let mut all = false;
    let mut skip = 0u64;
    let mut len: Option<u64> = None;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            files.push(arg.clone());
            continue;
        }
        let mut k = 1;
        while k < arg.len() {
            let c = arg[k];
            k += 1;
            match c {
                b'C' | b'x' | b'd' | b'o' | b'b' | b'c' => mode = c,
                b'v' => all = true,
                b'n' | b's' => {
                    let value = if k < arg.len() {
                        arg[k..].to_vec()
                    } else {
                        i += 1;
                        argv.get(i - 1).cloned().unwrap_or_default()
                    };
                    k = arg.len();
                    if c == b'n' {
                        len = number(&value);
                    } else {
                        skip = number(&value).unwrap_or(0);
                    }
                }
                _ => {
                    if let Some(status) = ctx.run_program(argv.to_vec()).await {
                        return status;
                    }
                    ctx.error(&format!(
                        "unsupported option '{}' (psh built-in hexdump)",
                        String::from_utf8_lossy(arg)
                    ))
                    .await;
                    return 2;
                }
            }
        }
    }
    let mut data = match slurp(ctx, &files).await {
        Ok(data) => data,
        Err(status) => return status,
    };
    let start = (skip as usize).min(data.len());
    data.drain(..start);
    if let Some(len) = len {
        data.truncate(len as usize);
    }
    let mut out = Vec::new();
    let mut previous: Option<&[u8]> = None;
    let mut starred = false;
    let mut at = skip as usize;
    for chunk in data.chunks(16) {
        if !all && chunk.len() == 16 && previous == Some(chunk) {
            if !starred {
                out.extend_from_slice(b"*\n");
                starred = true;
            }
            at += 16;
            continue;
        }
        starred = false;
        previous = Some(chunk);
        let mut line = String::new();
        match mode {
            b'C' => {
                line.push_str(&format!("{at:08x} "));
                for k in 0..16 {
                    if k % 8 == 0 {
                        line.push(' ');
                    }
                    match chunk.get(k) {
                        Some(b) => {
                            line.push(char::from(b"0123456789abcdef"[(b >> 4) as usize]));
                            line.push(char::from(b"0123456789abcdef"[(b & 15) as usize]));
                            line.push(' ');
                        }
                        None => line.push_str("   "),
                    }
                }
                line.push_str(" |");
                line.extend(chunk.iter().map(|&b| printable(b) as char));
                line.push('|');
            }
            b'b' => {
                line.push_str(&format!("{at:07x}"));
                for b in chunk {
                    line.push_str(&format!(" {b:03o}"));
                }
            }
            b'c' => {
                line.push_str(&format!("{at:07x}"));
                let t = OdType {
                    kind: b'c',
                    size: 1,
                    width: 3,
                    text: false,
                };
                for b in chunk {
                    line.push_str(&format!(" {:>3}", od_value(&t, &[*b])));
                }
            }
            _ => {
                line.push_str(&format!("{at:07x}"));
                for pair in chunk.chunks(2) {
                    let v = u16::from(pair[0]) | u16::from(*pair.get(1).unwrap_or(&0)) << 8;
                    match mode {
                        b'd' => line.push_str(&format!("   {v:05}")),
                        b'o' => line.push_str(&format!("  {v:06o}")),
                        b'x' => line.push_str(&format!("    {v:04x}")),
                        _ => line.push_str(&format!(" {v:04x}")),
                    }
                }
            }
        }
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
        at += chunk.len();
    }
    if !data.is_empty() || mode != b'C' {
        let end = if mode == b'C' {
            format!("{at:08x}\n")
        } else {
            format!("{at:07x}\n")
        };
        if !data.is_empty() {
            out.extend_from_slice(end.as_bytes());
        }
    }
    emit(ctx, &out).await
}

pub async fn strings(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut min = 4usize;
    let mut radix: Option<u8> = None;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            files.push(arg.clone());
            continue;
        }
        let text = String::from_utf8_lossy(arg).into_owned();
        let value = |i: &mut usize, attached: &str| -> Vec<u8> {
            if !attached.is_empty() {
                return attached.as_bytes().to_vec();
            }
            *i += 1;
            argv.get(*i - 1).cloned().unwrap_or_default()
        };
        if text == "-a" || text == "--all" || text == "-" {
        } else if let Some(n) = text.strip_prefix("-n") {
            min = number(&value(&mut i, n)).unwrap_or(4) as usize;
        } else if let Some(n) = text.strip_prefix("--bytes=") {
            min = number(n.as_bytes()).unwrap_or(4) as usize;
        } else if let Some(r) = text.strip_prefix("-t") {
            radix = value(&mut i, r).first().copied();
        } else if let Some(r) = text.strip_prefix("--radix=") {
            radix = r.bytes().next();
        } else if text == "-o" {
            radix = Some(b'o');
        } else if text[1..].bytes().all(|b| b.is_ascii_digit()) {
            min = text[1..].parse().unwrap_or(4);
        } else {
            if let Some(status) = ctx.run_program(argv.to_vec()).await {
                return status;
            }
            ctx.error(&format!(
                "unsupported option '{text}' (psh built-in strings)"
            ))
            .await;
            return 2;
        }
    }
    let data = match slurp(ctx, &files).await {
        Ok(data) => data,
        Err(status) => return status,
    };
    let min = min.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    for at in 0..=data.len() {
        let text = data
            .get(at)
            .is_some_and(|&b| (0x20..0x7f).contains(&b) || b == b'\t');
        if text {
            continue;
        }
        if at - start >= min {
            match radix {
                Some(b'x') => out.extend_from_slice(format!("{start:7x} ").as_bytes()),
                Some(b'd') => out.extend_from_slice(format!("{start:7} ").as_bytes()),
                Some(b'o') => out.extend_from_slice(format!("{start:7o} ").as_bytes()),
                _ => {}
            }
            out.extend_from_slice(&data[start..at]);
            out.push(b'\n');
        }
        start = at + 1;
    }
    emit(ctx, &out).await
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub async fn base32(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut decode = false;
    let mut ignore = false;
    let mut wrap = 76usize;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        let text = String::from_utf8_lossy(arg).into_owned();
        match text.as_str() {
            "-d" | "--decode" => decode = true,
            "-i" | "--ignore-garbage" => ignore = true,
            "-di" | "-id" => {
                decode = true;
                ignore = true;
            }
            "-w" | "--wrap" => {
                i += 1;
                wrap = argv.get(i - 1).and_then(|v| number(v)).unwrap_or(76) as usize;
            }
            _ if text.starts_with("-w") => wrap = number(&arg[2..]).unwrap_or(76) as usize,
            _ if text.starts_with("--wrap=") => wrap = number(&arg[7..]).unwrap_or(76) as usize,
            "--" => {}
            _ if text.len() > 1 && text.starts_with('-') => {
                return common::usage(ctx, &format!("unrecognized option '{text}'")).await;
            }
            _ => files.push(arg.clone()),
        }
    }
    if files.len() > 1 {
        let extra = common::quote(ctx, &files[1]);
        return common::usage(ctx, &format!("extra operand {extra}")).await;
    }
    let data = match slurp(ctx, &files).await {
        Ok(data) => data,
        Err(status) => return status,
    };
    let mut out = Vec::new();
    if decode {
        let mut bits = 0u64;
        let mut have = 0;
        let mut bad = false;
        for &b in &data {
            if b == b'\n' || b == b'=' {
                continue;
            }
            let Some(v) = BASE32.iter().position(|&c| c == b) else {
                if ignore {
                    continue;
                }
                bad = true;
                break;
            };
            bits = bits << 5 | v as u64;
            have += 5;
            if have >= 8 {
                have -= 8;
                out.push((bits >> have) as u8);
                bits &= (1 << have) - 1;
            }
        }
        let status = emit(ctx, &out).await;
        if bad {
            ctx.error("invalid input").await;
            return 1;
        }
        return status;
    }
    let mut text = Vec::new();
    for chunk in data.chunks(5) {
        let mut wide = [0u8; 5];
        wide[..chunk.len()].copy_from_slice(chunk);
        let v = wide.iter().fold(0u64, |acc, &b| acc << 8 | u64::from(b));
        let symbols = (chunk.len() * 8).div_ceil(5);
        for k in 0..8 {
            if k < symbols {
                text.push(BASE32[(v >> (35 - 5 * k) & 31) as usize]);
            } else {
                text.push(b'=');
            }
        }
    }
    if wrap == 0 {
        out = text;
    } else {
        for line in text.chunks(wrap) {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
    }
    emit(ctx, &out).await
}
