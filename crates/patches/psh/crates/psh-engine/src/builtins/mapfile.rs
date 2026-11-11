//! `mapfile` / `readarray [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd]
//! [-C callback] [-c quantum] [array]`: lines of input into an indexed array.

use std::future::poll_fn;

use crate::state::{R, Vp, is_name};
use crate::vars::Value;

use super::{s, shell_quote};

const USAGE: &[u8] = b"mapfile: usage: mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]\n";

fn number(v: &[u8]) -> Option<i64> {
    std::str::from_utf8(v).ok()?.trim().parse().ok()
}

pub(super) async fn mapfile(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let bname = s(&args[0]).into_owned();
    let mut delim = b'\n';
    let mut count: i64 = 0;
    let mut origin: Option<i64> = None;
    let mut skip: i64 = 0;
    let mut strip = false;
    let mut fd: u32 = 0;
    let mut callback: Option<Vec<u8>> = None;
    let mut quantum: i64 = 5000;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        let mut j = 1;
        while j < a.len() {
            let c = a[j];
            j += 1;
            if c == b't' {
                strip = true;
                continue;
            }
            if !b"dnOsuCc".contains(&c) {
                vp.error(&format!("{bname}: -{}: invalid option", c as char))
                    .await;
                vp.error_raw(USAGE).await;
                return Ok(2);
            }
            let value = if j < a.len() {
                let v = a[j..].to_vec();
                j = a.len();
                v
            } else {
                i += 1;
                match args.get(i) {
                    Some(v) => v.clone(),
                    None => {
                        vp.error(&format!(
                            "{bname}: -{}: option requires an argument",
                            c as char
                        ))
                        .await;
                        vp.error_raw(USAGE).await;
                        return Ok(2);
                    }
                }
            };
            let num = || number(&value).filter(|&n| n >= 0);
            match c {
                b'd' => delim = value.first().copied().unwrap_or(0),
                b'C' => callback = Some(value),
                b'n' | b'O' | b's' | b'u' | b'c' => {
                    let Some(n) = num() else {
                        let what = match c {
                            b'n' => "invalid line count",
                            b'O' => "invalid array origin",
                            b's' => "invalid line count",
                            b'u' => "invalid file descriptor specification",
                            _ => "invalid callback quantum",
                        };
                        vp.error(&format!("{bname}: {}: {what}", s(&value))).await;
                        return Ok(1);
                    };
                    match c {
                        b'n' => count = n,
                        b'O' => origin = Some(n),
                        b's' => skip = n,
                        b'u' => fd = n as u32,
                        _ => {
                            if n == 0 {
                                vp.error(&format!(
                                    "{bname}: {}: invalid callback quantum",
                                    s(&value)
                                ))
                                .await;
                                return Ok(1);
                            }
                            quantum = n;
                        }
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    let name = match args.get(i) {
        Some(n) => n.clone(),
        None => b"MAPFILE".to_vec(),
    };
    if !is_name(&name) {
        vp.error(&format!("{bname}: `{}': not a valid identifier", s(&name)))
            .await;
        return Ok(1);
    }
    let name = vp.resolve(&String::from_utf8_lossy(&name)).into_owned();
    if let Some(v) = vp.vars.get(&name) {
        if v.readonly {
            vp.error(&format!("{bname}: {name}: readonly variable"))
                .await;
            return Ok(1);
        }
        if v.is_assoc() {
            vp.error(&format!("{bname}: {name}: not an indexed array"))
                .await;
            return Ok(1);
        }
    }
    let Some(input) = vp.fd(fd) else {
        vp.error(&format!(
            "{bname}: {fd}: invalid file descriptor: Bad file descriptor"
        ))
        .await;
        return Ok(1);
    };
    // Without -O the array starts empty.
    let mut index = match origin {
        Some(o) => o,
        None => {
            vp.vars.entry(&name).value = Value::Indexed(Default::default());
            0
        }
    };
    if origin.is_some() {
        crate::arrays::to_indexed(&mut vp.vars.entry(&name).value);
    }
    // With a count, read byte by byte so that nothing past the last line is consumed
    // (the input may be shared); otherwise read in chunks.
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut eof = false;
    let mut chunk = vec![0u8; if count > 0 { 1 } else { 16 * 1024 }];
    let mut stored = 0i64;
    let mut skipped = 0i64;
    loop {
        if count > 0 && stored >= count {
            break;
        }
        // The next line, or what is left at end of input.
        let line = loop {
            if let Some(n) = buf[pos..].iter().position(|&b| b == delim) {
                let l = buf[pos..pos + n + 1].to_vec();
                pos += n + 1;
                break Some(l);
            }
            if eof {
                if pos < buf.len() {
                    let l = buf[pos..].to_vec();
                    pos = buf.len();
                    break Some(l);
                }
                break None;
            }
            let n = poll_fn(|cx| input.poll_read(cx, &mut chunk))
                .await
                .unwrap_or(0);
            if n == 0 {
                eof = true;
            } else {
                buf.extend_from_slice(&chunk[..n]);
            }
        };
        let Some(mut line) = line else { break };
        if skipped < skip {
            skipped += 1;
            continue;
        }
        if strip && line.last() == Some(&delim) {
            line.pop();
        }
        if let Some(cb) = &callback
            && stored % quantum == quantum - 1
        {
            let mut text = cb.clone();
            text.extend_from_slice(format!(" {index} ").as_bytes());
            text.extend_from_slice(&shell_quote(&line));
            text.push(b'\n');
            super::eval::run_text(
                vp,
                &text,
                vp.line.saturating_sub(1),
                super::eval::Origin::Eval,
            )
            .await?;
        }
        if let Value::Indexed(m) = &mut vp.vars.entry(&name).value {
            m.insert(index, line);
        }
        index += 1;
        stored += 1;
    }
    Ok(0)
}
