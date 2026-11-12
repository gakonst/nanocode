//! Small tools (nanocode's addition), after GNU coreutils, gettext, moreutils and
//! util-linux: expr, factor, envsubst, truncate, sponge, yes, uuidgen, and the ones
//! that answer from what the host says of itself: uname, hostname, whoami, id, nproc.

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::search::posix::for_lines;

async fn emit(ctx: &mut UtilCtx<'_>, out: &[u8]) -> i32 {
    match ctx.write(out).await {
        Ok(()) => 0,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

enum ExprError {
    Syntax(String),
    Other(String),
}

struct Expr<'a> {
    args: &'a [Vec<u8>],
    at: usize,
}

fn integer(v: &[u8]) -> Option<i128> {
    let text = std::str::from_utf8(v).ok()?;
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn truthy(v: &[u8]) -> bool {
    !v.is_empty() && integer(v) != Some(0)
}

impl Expr<'_> {
    fn peek(&self) -> Option<&[u8]> {
        self.args.get(self.at).map(Vec::as_slice)
    }

    fn take(&mut self, token: &[u8]) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    fn next(&mut self) -> Result<Vec<u8>, ExprError> {
        match self.args.get(self.at) {
            Some(arg) => {
                self.at += 1;
                Ok(arg.clone())
            }
            None => Err(ExprError::Syntax("missing argument after ".to_string())),
        }
    }

    fn primary(&mut self) -> Result<Vec<u8>, ExprError> {
        let Some(token) = self.peek().map(<[u8]>::to_vec) else {
            let last = self.args.last().cloned().unwrap_or_default();
            return Err(ExprError::Syntax(format!(
                "missing argument after '{}'",
                String::from_utf8_lossy(&last)
            )));
        };
        self.at += 1;
        match token.as_slice() {
            b"(" => {
                let value = self.or()?;
                if !self.take(b")") {
                    return Err(match self.peek() {
                        Some(found) => ExprError::Syntax(format!(
                            "expecting ')' instead of '{}'",
                            String::from_utf8_lossy(found)
                        )),
                        None => ExprError::Syntax(format!(
                            "expecting ')' after '{}'",
                            String::from_utf8_lossy(&self.args[self.at - 1])
                        )),
                    });
                }
                Ok(value)
            }
            b")" => Err(ExprError::Syntax("unexpected ')'".to_string())),
            b"+" if self.peek().is_some() => self.next(),
            b"length" if self.peek().is_some() => {
                let s = self.primary()?;
                Ok(String::from_utf8_lossy(&s)
                    .chars()
                    .count()
                    .to_string()
                    .into_bytes())
            }
            b"index" if self.peek().is_some() => {
                let s = self.primary()?;
                let chars = self.primary()?;
                let at = s
                    .iter()
                    .position(|b| chars.contains(b))
                    .map_or(0, |i| i + 1);
                Ok(at.to_string().into_bytes())
            }
            b"substr" if self.peek().is_some() => {
                let s = self.primary()?;
                let pos = self.primary()?;
                let len = self.primary()?;
                let text: Vec<char> = String::from_utf8_lossy(&s).chars().collect();
                match (integer(&pos), integer(&len)) {
                    (Some(pos), Some(len)) if pos > 0 && len > 0 => {
                        let start = (pos - 1) as usize;
                        let piece: String = text.iter().skip(start).take(len as usize).collect();
                        Ok(piece.into_bytes())
                    }
                    _ => Ok(Vec::new()),
                }
            }
            b"match" if self.peek().is_some() => {
                let s = self.primary()?;
                let pattern = self.primary()?;
                matched(&s, &pattern)
            }
            _ => Ok(token),
        }
    }

    fn colon(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.primary()?;
        while self.take(b":") {
            let pattern = self.primary()?;
            left = matched(&left, &pattern)?;
        }
        Ok(left)
    }

    fn numbers(a: &[u8], b: &[u8]) -> Result<(i128, i128), ExprError> {
        match (integer(a), integer(b)) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => Err(ExprError::Other("non-integer argument".to_string())),
        }
    }

    fn product(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.colon()?;
        loop {
            let op = match self.peek() {
                Some(b"*") => b'*',
                Some(b"/") => b'/',
                Some(b"%") => b'%',
                _ => return Ok(left),
            };
            self.at += 1;
            let right = self.colon()?;
            let (a, b) = Self::numbers(&left, &right)?;
            if op != b'*' && b == 0 {
                return Err(ExprError::Other("division by zero".to_string()));
            }
            let value = match op {
                b'*' => a.checked_mul(b),
                b'/' => a.checked_div(b),
                _ => a.checked_rem(b),
            };
            left = value
                .ok_or_else(|| ExprError::Other("integer overflow".to_string()))?
                .to_string()
                .into_bytes();
        }
    }

    fn sum(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.product()?;
        loop {
            let add = match self.peek() {
                Some(b"+") => true,
                Some(b"-") => false,
                _ => return Ok(left),
            };
            self.at += 1;
            let right = self.product()?;
            let (a, b) = Self::numbers(&left, &right)?;
            let value = if add {
                a.checked_add(b)
            } else {
                a.checked_sub(b)
            };
            left = value
                .ok_or_else(|| ExprError::Other("integer overflow".to_string()))?
                .to_string()
                .into_bytes();
        }
    }

    fn compare(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.sum()?;
        loop {
            let Some(op) = self.peek().map(<[u8]>::to_vec) else {
                return Ok(left);
            };
            if !matches!(
                op.as_slice(),
                b"=" | b"==" | b"!=" | b"<" | b"<=" | b">" | b">="
            ) {
                return Ok(left);
            }
            self.at += 1;
            let right = self.sum()?;
            let order = match (integer(&left), integer(&right)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => left.cmp(&right),
            };
            let holds = match op.as_slice() {
                b"=" | b"==" => order.is_eq(),
                b"!=" => order.is_ne(),
                b"<" => order.is_lt(),
                b"<=" => order.is_le(),
                b">" => order.is_gt(),
                _ => order.is_ge(),
            };
            left = if holds { b"1".to_vec() } else { b"0".to_vec() };
        }
    }

    fn and(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.compare()?;
        while self.take(b"&") {
            let right = self.compare()?;
            if !truthy(&left) || !truthy(&right) {
                left = b"0".to_vec();
            }
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<Vec<u8>, ExprError> {
        let mut left = self.and()?;
        while self.take(b"|") {
            let right = self.and()?;
            if !truthy(&left) {
                left = if truthy(&right) { right } else { b"0".to_vec() };
            }
        }
        Ok(left)
    }
}

/// STRING : REGEX: the first group's text when the pattern has one, else how many
/// characters matched at the start.
fn matched(s: &[u8], pattern: &[u8]) -> Result<Vec<u8>, ExprError> {
    let source = for_lines(pattern, false)
        .ok()
        .and_then(|t| t.pattern)
        .ok_or_else(|| ExprError::Other("Invalid regular expression".to_string()))?;
    let regex = psh_pattern::bytes::Regex::new(&format!("^(?:{source})"))
        .map_err(|_| ExprError::Other("Invalid regular expression".to_string()))?;
    let groups = regex.captures_len() > 1;
    match regex.captures(s) {
        Some(found) if groups => Ok(found
            .get(1)
            .map(|m| m.as_bytes().to_vec())
            .unwrap_or_default()),
        Some(found) => {
            let text = found.get(0).map(|m| m.as_bytes()).unwrap_or_default();
            Ok(String::from_utf8_lossy(text)
                .chars()
                .count()
                .to_string()
                .into_bytes())
        }
        None if groups => Ok(Vec::new()),
        None => Ok(b"0".to_vec()),
    }
}

pub async fn expr(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let args = match argv.get(1).map(Vec::as_slice) {
        Some(b"--") => &argv[2..],
        _ => &argv[1..],
    };
    if args.is_empty() {
        ctx.error("missing operand").await;
        ctx.error_raw(b"Try 'expr --help' for more information.\n")
            .await;
        return 2;
    }
    let mut parser = Expr { args, at: 0 };
    let result = parser.or().and_then(|value| match parser.peek() {
        Some(extra) => Err(ExprError::Syntax(format!(
            "unexpected argument '{}'",
            String::from_utf8_lossy(extra)
        ))),
        None => Ok(value),
    });
    match result {
        Ok(mut value) => {
            let status = i32::from(!truthy(&value));
            value.push(b'\n');
            emit(ctx, &value).await.max(status)
        }
        Err(ExprError::Syntax(what)) => {
            ctx.error(&format!("syntax error: {what}")).await;
            2
        }
        Err(ExprError::Other(what)) => {
            ctx.error(&what).await;
            2
        }
    }
}

fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    (u128::from(a) * u128::from(b) % u128::from(m)) as u64
}

fn pow_mod(mut base: u64, mut exp: u64, m: u64) -> u64 {
    let mut result = 1;
    base %= m;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_mod(result, base, m);
        }
        base = mul_mod(base, base, m);
        exp >>= 1;
    }
    result
}

/// Miller-Rabin with the bases that decide every 64-bit number.
fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    for p in [2u64, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37] {
        if n.is_multiple_of(p) {
            return n == p;
        }
    }
    let (mut d, mut r) = (n - 1, 0);
    while d % 2 == 0 {
        d /= 2;
        r += 1;
    }
    'bases: for a in [2u64, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37] {
        let mut x = pow_mod(a, d, n);
        if x == 1 || x == n - 1 {
            continue;
        }
        for _ in 1..r {
            x = mul_mod(x, x, n);
            if x == n - 1 {
                continue 'bases;
            }
        }
        return false;
    }
    true
}

/// Pollard's rho: a factor of a composite n that is not n.
fn rho(n: u64) -> u64 {
    if n.is_multiple_of(2) {
        return 2;
    }
    let mut c = 1;
    loop {
        let step = |x: u64| (mul_mod(x, x, n) + c) % n;
        let (mut x, mut y, mut d) = (2, 2, 1);
        while d == 1 {
            x = step(x);
            y = step(step(y));
            let diff = x.abs_diff(y);
            let (mut a, mut b) = (diff, n);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            d = a;
        }
        if d != n {
            return d;
        }
        c += 1;
    }
}

fn factors(n: u64, out: &mut Vec<u64>) {
    if n < 2 {
        return;
    }
    if is_prime(n) {
        out.push(n);
        return;
    }
    let d = rho(n);
    factors(d, out);
    factors(n / d, out);
}

pub async fn factor(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut words: Vec<Vec<u8>> = argv[1..].iter().filter(|a| *a != b"--").cloned().collect();
    if words.is_empty() {
        let mut data = Vec::new();
        if let Err(e) = ctx.read_to_end(&mut data).await {
            ctx.error(&format!("read error: {}", error_text(&e))).await;
            return 1;
        }
        words = data
            .split(u8::is_ascii_whitespace)
            .filter(|w| !w.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
    }
    let mut out = Vec::new();
    let mut status = 0;
    for word in &words {
        let text = String::from_utf8_lossy(word).into_owned();
        let digits = text.strip_prefix('+').unwrap_or(&text);
        let Ok(n) = digits.parse::<u64>() else {
            let _ = ctx.write(&std::mem::take(&mut out)).await;
            let shown = common::quote(ctx, word);
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                ctx.error(&format!("{shown} is too large")).await;
            } else {
                ctx.error(&format!("{shown} is not a valid positive integer"))
                    .await;
            }
            status = 1;
            continue;
        };
        let mut found = Vec::new();
        let mut rest = n;
        for p in [2u64, 3, 5, 7, 11, 13] {
            while rest > 1 && rest % p == 0 {
                found.push(p);
                rest /= p;
            }
        }
        factors(rest, &mut found);
        found.sort_unstable();
        out.extend_from_slice(format!("{n}:").as_bytes());
        for f in found {
            out.extend_from_slice(format!(" {f}").as_bytes());
        }
        out.push(b'\n');
    }
    emit(ctx, &out).await.max(status)
}

pub async fn envsubst(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut list = false;
    let mut format: Option<Vec<u8>> = None;
    for arg in &argv[1..] {
        match arg.as_slice() {
            b"-v" | b"--variables" => list = true,
            _ if arg.starts_with(b"-") && arg.len() > 1 => {
                return common::usage(
                    ctx,
                    &format!("unrecognized option '{}'", String::from_utf8_lossy(arg)),
                )
                .await;
            }
            _ => format = Some(arg.clone()),
        }
    }
    // The names a text refers to, with where each reference is.
    fn references(text: &[u8]) -> Vec<(usize, usize, Vec<u8>)> {
        let mut found = Vec::new();
        let mut i = 0;
        while i < text.len() {
            if text[i] != b'$' {
                i += 1;
                continue;
            }
            let braced = text.get(i + 1) == Some(&b'{');
            let start = i + 1 + usize::from(braced);
            let mut end = start;
            while end < text.len()
                && (text[end].is_ascii_alphanumeric() || text[end] == b'_')
                && !(end == start && text[end].is_ascii_digit())
            {
                end += 1;
            }
            if end == start || (braced && text.get(end) != Some(&b'}')) {
                i += 1;
                continue;
            }
            let after = end + usize::from(braced);
            found.push((i, after, text[start..end].to_vec()));
            i = after;
        }
        found
    }
    let allowed: Option<Vec<Vec<u8>>> = format
        .as_ref()
        .map(|f| references(f).into_iter().map(|(_, _, name)| name).collect());
    if list {
        let Some(names) = allowed else {
            return common::usage(ctx, "missing arguments").await;
        };
        let mut out = Vec::new();
        for name in names {
            out.extend_from_slice(&name);
            out.push(b'\n');
        }
        return emit(ctx, &out).await;
    }
    let mut data = Vec::new();
    if let Err(e) = ctx.read_to_end(&mut data).await {
        ctx.error(&format!(
            "error while reading \"standard input\": {}",
            error_text(&e)
        ))
        .await;
        return 1;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for (start, end, name) in references(&data) {
        if allowed.as_ref().is_some_and(|names| !names.contains(&name)) {
            continue;
        }
        out.extend_from_slice(&data[at..start]);
        out.extend_from_slice(ctx.var(&name).unwrap_or_default());
        at = end;
    }
    out.extend_from_slice(&data[at..]);
    emit(ctx, &out).await
}

pub async fn truncate(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut size: Option<Vec<u8>> = None;
    let mut reference: Option<Vec<u8>> = None;
    let mut no_create = false;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        let text = String::from_utf8_lossy(arg).into_owned();
        let value = |i: &mut usize, attached: &str| -> Vec<u8> {
            if !attached.is_empty() {
                return attached.as_bytes().to_vec();
            }
            *i += 1;
            argv.get(*i - 1).cloned().unwrap_or_default()
        };
        if text == "-c" || text == "--no-create" {
            no_create = true;
        } else if let Some(v) = text.strip_prefix("--size=") {
            size = Some(v.as_bytes().to_vec());
        } else if let Some(v) = text.strip_prefix("--reference=") {
            reference = Some(v.as_bytes().to_vec());
        } else if let Some(v) = text.strip_prefix("-s") {
            size = Some(value(&mut i, v));
        } else if let Some(v) = text.strip_prefix("-r") {
            reference = Some(value(&mut i, v));
        } else if text == "--" {
        } else if text.len() > 1 && text.starts_with('-') {
            return common::usage(ctx, &format!("invalid option -- '{}'", &text[1..])).await;
        } else {
            files.push(arg.clone());
        }
    }
    if size.is_none() && reference.is_none() {
        return common::usage(ctx, "you must specify either '--size' or '--reference'").await;
    }
    if files.is_empty() {
        return common::usage(ctx, "missing file operand").await;
    }
    let base = match &reference {
        Some(name) => match ctx.stat(name, true).await {
            Ok(meta) => Some(meta.size),
            Err(e) => {
                ctx.error(&format!(
                    "cannot stat '{}': {}",
                    String::from_utf8_lossy(name),
                    error_text(&e)
                ))
                .await;
                return 1;
            }
        },
        None => None,
    };
    let (mode, amount) = match &size {
        Some(spec) => {
            let (mode, digits) = match spec.first() {
                Some(m @ (b'+' | b'-' | b'<' | b'>' | b'/' | b'%')) => (*m, &spec[1..]),
                _ => (b'=', &spec[..]),
            };
            match common::parse_count(digits) {
                Some(n) => (mode, n),
                None => {
                    let spec = common::quote(ctx, spec);
                    return common::usage(ctx, &format!("Invalid number: {spec}")).await;
                }
            }
        }
        None => (b'=', 0),
    };
    let mut status = 0;
    for name in &files {
        let shown = String::from_utf8_lossy(name).into_owned();
        let existing = ctx.stat(name, true).await.ok();
        if existing.is_none() && no_create {
            continue;
        }
        let mut data = Vec::new();
        if existing.is_some() {
            let read = match Input::open(ctx, name).await {
                Ok(input) => input.read_to_end(ctx, &mut data).await,
                Err(e) => Err(e),
            };
            if let Err(e) = read {
                ctx.error(&format!(
                    "cannot open '{shown}' for writing: {}",
                    error_text(&e)
                ))
                .await;
                status = 1;
                continue;
            }
        }
        let current = base.unwrap_or(data.len() as u64);
        let target = match (size.is_some(), mode) {
            (false, _) => current,
            (_, b'+') => current.saturating_add(amount),
            (_, b'-') => current.saturating_sub(amount),
            (_, b'<') => current.min(amount).min(data.len() as u64),
            (_, b'>') => (data.len() as u64).max(amount),
            (_, b'/') if amount > 0 => current / amount * amount,
            (_, b'%') if amount > 0 => current.div_ceil(amount) * amount,
            (_, b'/' | b'%') => {
                ctx.error("division by zero").await;
                return 1;
            }
            _ => amount,
        };
        data.resize(target as usize, 0);
        let written = match ctx.open(name, OpenOptions::write()).await {
            Ok(file) => file.write_all(&data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            ctx.error(&format!(
                "cannot open '{shown}' for writing: {}",
                error_text(&e)
            ))
            .await;
            status = 1;
        }
    }
    status
}

pub async fn sponge(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut append = false;
    let mut target: Option<Vec<u8>> = None;
    for arg in &argv[1..] {
        match arg.as_slice() {
            b"-a" => append = true,
            _ => target = Some(arg.clone()),
        }
    }
    let mut data = Vec::new();
    if let Err(e) = ctx.read_to_end(&mut data).await {
        ctx.error(&format!("read error: {}", error_text(&e))).await;
        return 1;
    }
    let Some(target) = target else {
        return emit(ctx, &data).await;
    };
    let options = if append {
        OpenOptions::append()
    } else {
        OpenOptions::write()
    };
    let written = match ctx.open(&target, options).await {
        Ok(file) => file.write_all(&data).await,
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        ctx.error(&format!(
            "{}: {}",
            String::from_utf8_lossy(&target),
            error_text(&e)
        ))
        .await;
        return 1;
    }
    0
}

pub async fn yes(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut line = if argv.len() > 1 {
        argv[1..].join(&b' ')
    } else {
        b"y".to_vec()
    };
    line.push(b'\n');
    let mut block = Vec::new();
    while block.len() < 8192 {
        block.extend_from_slice(&line);
    }
    loop {
        let wrote = match ctx.write(&block).await {
            Ok(()) => ctx.flush().await,
            Err(e) => Err(e),
        };
        if let Err(e) = wrote {
            return common::write_failed(ctx, &e).await;
        }
        ctx.check().await;
    }
}

pub async fn uuidgen(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    for arg in &argv[1..] {
        match arg.as_slice() {
            b"-r" | b"--random" => {}
            _ => {
                if let Some(status) = ctx.run_program(argv.to_vec()).await {
                    return status;
                }
                ctx.error(&format!(
                    "unsupported option '{}' (psh built-in uuidgen: random UUIDs)",
                    String::from_utf8_lossy(arg)
                ))
                .await;
                return 2;
            }
        }
    }
    let mut bytes = [0u8; 16];
    if !crate::lines::entropy(ctx, &mut bytes).await {
        ctx.error("/dev/urandom: cannot read").await;
        return 1;
    }
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let text = format!(
        "{}-{}-{}-{}-{}\n",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    emit(ctx, text.as_bytes()).await
}

pub async fn uname(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let info = ctx.host_info();
    let (system, os) = if info.ostype.starts_with("linux") {
        ("Linux".to_string(), "GNU/Linux".to_string())
    } else if info.ostype.starts_with("darwin") {
        ("Darwin".to_string(), "Darwin".to_string())
    } else {
        (info.ostype.clone(), info.ostype.clone())
    };
    let machine = match (info.hosttype.as_str(), system.as_str()) {
        ("aarch64", "Darwin") => "arm64".to_string(),
        (other, _) => other.to_string(),
    };
    let node = String::from_utf8_lossy(&info.hostname).into_owned();
    let unknown = "unknown".to_string();
    // In GNU's order: -s -n -r -v -m -p -i -o. What the host does not say is unknown.
    let fields = [
        &system, &node, &unknown, &unknown, &machine, &unknown, &unknown, &os,
    ];
    let mut wanted = [false; 8];
    let mut all = false;
    for arg in &argv[1..] {
        let text = String::from_utf8_lossy(arg).into_owned();
        let long = match text.as_str() {
            "--all" => Some("a"),
            "--kernel-name" => Some("s"),
            "--nodename" => Some("n"),
            "--kernel-release" => Some("r"),
            "--kernel-version" => Some("v"),
            "--machine" => Some("m"),
            "--processor" => Some("p"),
            "--hardware-platform" => Some("i"),
            "--operating-system" => Some("o"),
            _ => None,
        };
        let letters = match long {
            Some(letters) => letters,
            None if text.starts_with('-') && !text.starts_with("--") && text.len() > 1 => {
                &text[1..]
            }
            None => {
                let shown = common::quote(ctx, arg);
                return common::usage(ctx, &format!("extra operand {shown}")).await;
            }
        };
        for c in letters.chars() {
            match "snrvmpio".find(c) {
                Some(at) => wanted[at] = true,
                None if c == 'a' => all = true,
                None => return common::usage(ctx, &format!("invalid option -- '{c}'")).await,
            }
        }
    }
    if all {
        // -a leaves out the processor and the platform when they are unknown.
        wanted = [true, true, true, true, true, false, false, true];
    } else if !wanted.contains(&true) {
        wanted[0] = true;
    }
    let shown: Vec<&str> = fields
        .iter()
        .zip(wanted)
        .filter(|(_, w)| *w)
        .map(|(f, _)| f.as_str())
        .collect();
    emit(ctx, format!("{}\n", shown.join(" ")).as_bytes()).await
}

pub async fn hostname(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let info = ctx.host_info();
    let mut name = info.hostname.clone();
    for arg in &argv[1..] {
        match arg.as_slice() {
            b"-s" | b"--short" => {
                name = name
                    .split(|&b| b == b'.')
                    .next()
                    .unwrap_or_default()
                    .to_vec();
            }
            b"-f" | b"--fqdn" | b"--long" => {}
            _ => {
                ctx.error("you must be root to change the host name").await;
                return 1;
            }
        }
    }
    name.push(b'\n');
    emit(ctx, &name).await
}

pub async fn whoami(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    if let Some(extra) = argv.get(1) {
        let extra = common::quote(ctx, extra);
        return common::usage(ctx, &format!("extra operand {extra}")).await;
    }
    let uid = ctx.host_info().euid;
    match ctx.user_name(uid) {
        Some(mut name) => {
            name.push(b'\n');
            emit(ctx, &name).await
        }
        None => {
            ctx.error(&format!("cannot find name for user ID {uid}"))
                .await;
            1
        }
    }
}

pub async fn id(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let info = ctx.host_info();
    let (mut user, mut group, mut groups, mut names, mut real) =
        (false, false, false, false, false);
    for arg in &argv[1..] {
        let text = String::from_utf8_lossy(arg).into_owned();
        let letters = match text.as_str() {
            "--user" => "u",
            "--group" => "g",
            "--groups" => "G",
            "--name" => "n",
            "--real" => "r",
            _ if text.starts_with('-') && text.len() > 1 => &text[1..],
            _ => {
                // Another user: the host says nothing of them.
                ctx.error(&format!("'{text}': no such user")).await;
                return 1;
            }
        };
        for c in letters.chars() {
            match c {
                'u' => user = true,
                'g' => group = true,
                'G' => groups = true,
                'n' => names = true,
                'r' => real = true,
                _ => return common::usage(ctx, &format!("invalid option -- '{c}'")).await,
            }
        }
    }
    let uid = if real { info.uid } else { info.euid };
    let gid = info.groups.first().copied().unwrap_or(uid);
    let user_name = |ctx: &UtilCtx<'_>, id: u32| {
        ctx.user_name(id)
            .map(|n| String::from_utf8_lossy(&n).into_owned())
    };
    let group_name = |ctx: &UtilCtx<'_>, id: u32| {
        ctx.group_name(id)
            .map(|n| String::from_utf8_lossy(&n).into_owned())
    };
    let show = |name: Option<String>, id: u32| match (names, name) {
        (true, Some(name)) => name,
        _ => id.to_string(),
    };
    let text = if user {
        show(user_name(ctx, uid), uid)
    } else if group {
        show(group_name(ctx, gid), gid)
    } else if groups {
        let list: Vec<String> = info
            .groups
            .iter()
            .map(|&g| show(group_name(ctx, g), g))
            .collect();
        list.join(" ")
    } else {
        if names {
            return common::usage(ctx, "cannot print only names or real IDs in default format")
                .await;
        }
        let pair = |name: Option<String>, id: u32| match name {
            Some(name) => format!("{id}({name})"),
            None => id.to_string(),
        };
        let list: Vec<String> = info
            .groups
            .iter()
            .map(|&g| pair(group_name(ctx, g), g))
            .collect();
        format!(
            "uid={} gid={} groups={}",
            pair(user_name(ctx, uid), uid),
            pair(group_name(ctx, gid), gid),
            list.join(",")
        )
    };
    emit(ctx, format!("{text}\n").as_bytes()).await
}

pub async fn nproc(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut ignore = 0usize;
    for arg in &argv[1..] {
        let text = String::from_utf8_lossy(arg).into_owned();
        if text == "--all" {
        } else if let Some(n) = text.strip_prefix("--ignore=") {
            match n.parse() {
                Ok(n) => ignore = n,
                Err(_) => {
                    let n = common::quote(ctx, n.as_bytes());
                    return common::usage(ctx, &format!("invalid number: {n}")).await;
                }
            }
        } else {
            let shown = common::quote(ctx, arg);
            return common::usage(ctx, &format!("extra operand {shown}")).await;
        }
    }
    let count = std::thread::available_parallelism().map_or(1, usize::from);
    let shown = count.saturating_sub(ignore).max(1);
    emit(ctx, format!("{shown}\n").as_bytes()).await
}
