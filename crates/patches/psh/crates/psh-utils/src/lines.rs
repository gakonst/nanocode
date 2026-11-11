//! Line and field tools (nanocode's addition), after GNU coreutils and util-linux: tac,
//! rev, fold, expand, unexpand, column, join, shuf, split and numfmt. Inputs are read
//! whole.

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag, long};

async fn read_all(ctx: &mut UtilCtx<'_>, names: &[Vec<u8>]) -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    let stdin = [b"-".to_vec()];
    let names = if names.is_empty() { &stdin[..] } else { names };
    let mut status = 0;
    for name in names {
        let read = match Input::open(ctx, name).await {
            Ok(input) => input.read_to_end(ctx, &mut data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            common::file_error(ctx, name, &e).await;
            status = 1;
        }
    }
    if status != 0 && data.is_empty() {
        return Err(status);
    }
    Ok(data)
}

async fn emit(ctx: &mut UtilCtx<'_>, out: &[u8]) -> i32 {
    match ctx.write(out).await {
        Ok(()) => 0,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

fn count(text: &[u8]) -> Option<usize> {
    std::str::from_utf8(text).ok()?.parse().ok()
}

fn char_len(first: u8) -> usize {
    match first {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

pub const TAC: Spec = Spec {
    name: "tac",
    opts: &[both('b', "before"), both('s', "separator").value()],
    posix_order: false,
};

pub async fn tac(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &TAC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let before = p.has(&TAC, "before");
    let sep = p.last(&TAC, "separator").unwrap_or(b"\n").to_vec();
    if sep.is_empty() {
        return common::usage(ctx, "separator cannot be empty").await;
    }
    let mut out = Vec::new();
    let mut status = 0;
    for name in common::operands_or_stdin(&p) {
        let data = match read_all(ctx, std::slice::from_ref(&name)).await {
            Ok(data) => data,
            Err(st) => {
                status = st;
                continue;
            }
        };
        // Where each record starts: after a separator, or at one with -b.
        let mut starts = vec![0];
        let mut at = 0;
        while at + sep.len() <= data.len() {
            if data[at..].starts_with(&sep) {
                let cut = if before { at } else { at + sep.len() };
                if cut != 0 && cut != data.len() {
                    starts.push(cut);
                }
                at += sep.len();
            } else {
                at += 1;
            }
        }
        starts.dedup();
        for (n, &start) in starts.iter().enumerate().rev() {
            let end = starts.get(n + 1).copied().unwrap_or(data.len());
            out.extend_from_slice(&data[start..end]);
        }
    }
    emit(ctx, &out).await.max(status)
}

pub async fn rev(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let files: Vec<Vec<u8>> = argv[1..].iter().filter(|a| *a != b"--").cloned().collect();
    let data = match read_all(ctx, &files).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let mut out = Vec::with_capacity(data.len());
    for line in data.split_inclusive(|&b| b == b'\n') {
        let (body, end) = match line.strip_suffix(b"\n") {
            Some(body) => (body, &b"\n"[..]),
            None => (line, &b""[..]),
        };
        let mut chars: Vec<&[u8]> = Vec::new();
        let mut at = 0;
        while at < body.len() {
            let len = char_len(body[at]).min(body.len() - at);
            chars.push(&body[at..at + len]);
            at += len;
        }
        for c in chars.iter().rev() {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(end);
    }
    emit(ctx, &out).await
}

pub const FOLD: Spec = Spec {
    name: "fold",
    opts: &[
        both('b', "bytes"),
        both('s', "spaces"),
        both('w', "width").value(),
    ],
    posix_order: false,
};

pub async fn fold(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    // fold -80 is fold -w 80.
    let mut args = argv.to_vec();
    for arg in args.iter_mut().skip(1) {
        if arg.len() > 1 && arg[0] == b'-' && arg[1..].iter().all(u8::is_ascii_digit) {
            let mut wide = b"-w".to_vec();
            wide.extend_from_slice(&arg[1..]);
            *arg = wide;
        }
    }
    let p = match common::parse(ctx, &FOLD, &args).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let bytes = p.has(&FOLD, "bytes");
    let spaces = p.has(&FOLD, "spaces");
    let width = match p.last(&FOLD, "width") {
        Some(w) => match count(w) {
            Some(n) if n > 0 => n,
            _ => {
                let w = common::quote(ctx, w);
                return common::usage(ctx, &format!("invalid number of columns: {w}")).await;
            }
        },
        None => 80,
    };
    let data = match read_all(ctx, &p.operands).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let advance = |column: usize, b: u8| -> usize {
        if bytes {
            return column + 1;
        }
        match b {
            8 => column.saturating_sub(1),
            b'\r' => 0,
            b'\t' => column + 8 - column % 8,
            _ => column + 1,
        }
    };
    let mut out = Vec::with_capacity(data.len() + data.len() / width);
    let mut line: Vec<u8> = Vec::new();
    let mut column = 0;
    for &b in &data {
        if b == b'\n' {
            out.extend_from_slice(&line);
            out.push(b'\n');
            line.clear();
            column = 0;
            continue;
        }
        loop {
            let next = advance(column, b);
            if next <= width || line.is_empty() {
                column = next;
                line.push(b);
                break;
            }
            let blank = if spaces {
                line.iter().rposition(|c| c.is_ascii_whitespace())
            } else {
                None
            };
            match blank {
                Some(at) => {
                    out.extend_from_slice(&line[..=at]);
                    out.push(b'\n');
                    let rest = line.split_off(at + 1);
                    line = rest;
                    column = line.iter().fold(0, |c, &b| advance(c, b));
                }
                None => {
                    out.extend_from_slice(&line);
                    out.push(b'\n');
                    line.clear();
                    column = 0;
                }
            }
        }
    }
    out.extend_from_slice(&line);
    emit(ctx, &out).await
}

fn tab_stops(spec: Option<&[u8]>) -> Option<(Vec<usize>, usize)> {
    let Some(spec) = spec else {
        return Some((Vec::new(), 8));
    };
    let parts: Vec<&[u8]> = spec
        .split(|&b| b == b',' || b == b' ')
        .filter(|p| !p.is_empty())
        .collect();
    let mut stops = Vec::new();
    for part in &parts {
        stops.push(count(part)?);
    }
    match stops.as_slice() {
        [] => Some((Vec::new(), 8)),
        [one] if *one > 0 => Some((Vec::new(), *one)),
        [_] => None,
        _ => Some((stops, 0)),
    }
}

fn next_stop(stops: &[usize], every: usize, column: usize) -> Option<usize> {
    if every > 0 {
        return Some(column + every - column % every);
    }
    stops.iter().copied().find(|&s| s > column)
}

pub const EXPAND: Spec = Spec {
    name: "expand",
    opts: &[both('i', "initial"), both('t', "tabs").value()],
    posix_order: false,
};

pub async fn expand(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &EXPAND, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let Some((stops, every)) = tab_stops(p.last(&EXPAND, "tabs")) else {
        return common::usage(ctx, "tab size cannot be 0").await;
    };
    let initial = p.has(&EXPAND, "initial");
    let data = match read_all(ctx, &p.operands).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let mut out = Vec::with_capacity(data.len());
    let mut column = 0;
    let mut leading = true;
    for &b in &data {
        match b {
            b'\n' => {
                out.push(b);
                column = 0;
                leading = true;
            }
            b'\t' if leading || !initial => {
                let to = next_stop(&stops, every, column).unwrap_or(column + 1);
                out.resize(out.len() + (to - column), b' ');
                column = to;
            }
            8 => {
                out.push(b);
                column = column.saturating_sub(1);
            }
            _ => {
                if b != b' ' {
                    leading = false;
                }
                out.push(b);
                if b & 0xc0 != 0x80 {
                    column += 1;
                }
            }
        }
    }
    emit(ctx, &out).await
}

pub const UNEXPAND: Spec = Spec {
    name: "unexpand",
    opts: &[
        both('a', "all"),
        long("first-only"),
        both('t', "tabs").value(),
    ],
    posix_order: false,
};

pub async fn unexpand(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &UNEXPAND, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let Some((stops, every)) = tab_stops(p.last(&UNEXPAND, "tabs")) else {
        return common::usage(ctx, "tab size cannot be 0").await;
    };
    let all =
        (p.has(&UNEXPAND, "all") || p.has(&UNEXPAND, "tabs")) && !p.has(&UNEXPAND, "first-only");
    let data = match read_all(ctx, &p.operands).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let mut out = Vec::with_capacity(data.len());
    for line in data.split_inclusive(|&b| b == b'\n') {
        // GNU's loop: blanks wait until it is known whether a tab replaces them. One
        // space that ends at a tab stop becomes a tab only when more blanks follow.
        let mut column = 0;
        let mut convert = true;
        let mut pending: Vec<u8> = Vec::new();
        let mut one_before_stop = false;
        let mut prev_blank = true;
        for &b in line {
            let mut c = b;
            let blank = c == b' ' || c == b'\t';
            if convert && blank {
                let stop = next_stop(&stops, every, column);
                match stop {
                    None => convert = false,
                    Some(stop) => {
                        if c == b'\t' {
                            column = stop;
                            if !pending.is_empty() {
                                pending[0] = b'\t';
                            }
                        } else {
                            column += 1;
                            if !(prev_blank && column == stop) {
                                if column == stop {
                                    one_before_stop = true;
                                }
                                pending.push(c);
                                prev_blank = true;
                                continue;
                            }
                            c = b'\t';
                            if pending.is_empty() {
                                pending.push(c);
                            } else {
                                pending[0] = c;
                            }
                        }
                        pending.truncate(usize::from(one_before_stop));
                    }
                }
            } else if c == 8 {
                column = column.saturating_sub(1);
            } else if c != b'\n' && c & 0xc0 != 0x80 {
                column += 1;
            }
            if !pending.is_empty() {
                if pending.len() > 1 && one_before_stop {
                    pending[0] = b'\t';
                }
                out.extend_from_slice(&pending);
                pending.clear();
                one_before_stop = false;
            }
            prev_blank = blank;
            convert &= all || blank;
            out.push(c);
        }
        out.extend_from_slice(&pending);
    }
    emit(ctx, &out).await
}

pub const COLUMN: Spec = Spec {
    name: "column",
    opts: &[
        both('t', "table"),
        both('s', "separator").value(),
        both('o', "output-separator").value(),
        both('c', "output-width").value(),
        both('x', "fillrows"),
    ],
    posix_order: false,
};

fn width_of(text: &[u8]) -> usize {
    text.iter().filter(|&&b| b & 0xc0 != 0x80).count()
}

pub async fn column(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &COLUMN, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let data = match read_all(ctx, &p.operands).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let lines: Vec<&[u8]> = data
        .split(|&b| b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .collect();
    let mut out = Vec::new();
    if p.has(&COLUMN, "table") {
        let seps = p.last(&COLUMN, "separator").map(<[u8]>::to_vec);
        let join = p
            .last(&COLUMN, "output-separator")
            .unwrap_or(b"  ")
            .to_vec();
        let is_sep = |b: u8| match &seps {
            Some(set) => set.contains(&b),
            None => b == b' ' || b == b'\t',
        };
        let rows: Vec<Vec<&[u8]>> = lines
            .iter()
            .map(|l| l.split(|&b| is_sep(b)).filter(|f| !f.is_empty()).collect())
            .collect();
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0; columns];
        for row in &rows {
            for (i, field) in row.iter().enumerate() {
                widths[i] = widths[i].max(width_of(field));
            }
        }
        for row in &rows {
            for (i, field) in row.iter().enumerate() {
                out.extend_from_slice(field);
                if i + 1 < row.len() {
                    out.resize(out.len() + widths[i] - width_of(field), b' ');
                    out.extend_from_slice(&join);
                }
            }
            out.push(b'\n');
        }
        return emit(ctx, &out).await;
    }
    let total = match p.last(&COLUMN, "output-width") {
        Some(w) => count(w).unwrap_or(80),
        None => ctx.var(b"COLUMNS").and_then(count).unwrap_or(80),
    };
    let longest = lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
    let cell = 8 * ((longest + 8) / 8);
    let columns = (total / cell).max(1);
    let rows = lines.len().div_ceil(columns);
    let across = p.has(&COLUMN, "fillrows");
    for row in 0..rows {
        let mut written = 0;
        for col in 0..columns {
            let at = if across {
                row * columns + col
            } else {
                col * rows + row
            };
            let Some(entry) = lines.get(at) else { break };
            out.extend_from_slice(entry);
            written += width_of(entry);
            let more = if across {
                col + 1 < columns && at + 1 < lines.len()
            } else {
                (col + 1) * rows + row < lines.len()
            };
            if more {
                let end = (col + 1) * cell;
                while written < end {
                    out.push(b'\t');
                    written = (written + 8) & !7;
                }
            }
        }
        out.push(b'\n');
    }
    emit(ctx, &out).await
}

pub const JOIN: Spec = Spec {
    name: "join",
    opts: &[
        flag('1').value(),
        flag('2').value(),
        flag('j').value(),
        flag('t').value(),
        flag('a').value(),
        flag('v').value(),
        flag('e').value(),
        flag('o').value(),
        both('i', "ignore-case"),
        long("header"),
        long("nocheck-order"),
        long("check-order"),
    ],
    posix_order: false,
};

pub async fn join(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &JOIN, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut keys = [1usize, 1];
    let mut unpaired = [false, false];
    let mut only_unpaired = false;
    let mut sep: Option<u8> = None;
    let mut empty: Vec<u8> = Vec::new();
    let mut format: Option<Vec<(usize, usize)>> = None;
    let mut auto = false;
    for (key, value) in p.iter(&JOIN) {
        let value = value.unwrap_or_default();
        let field = || count(value).filter(|n| *n > 0);
        let which = || match value {
            b"1" => Some(0),
            b"2" => Some(1),
            _ => None,
        };
        let bad = match key.as_str() {
            "1" => field().map(|n| keys[0] = n).is_none(),
            "2" => field().map(|n| keys[1] = n).is_none(),
            "j" => field().map(|n| keys = [n, n]).is_none(),
            "a" => which().map(|w| unpaired[w] = true).is_none(),
            "v" => which()
                .map(|w| {
                    unpaired[w] = true;
                    only_unpaired = true;
                })
                .is_none(),
            "t" => {
                sep = value.first().copied();
                value.len() > 1 && value != b"\\0"
            }
            "e" => {
                empty = value.to_vec();
                false
            }
            "o" => {
                if value == b"auto" {
                    auto = true;
                    false
                } else {
                    let mut list = Vec::new();
                    let mut ok = true;
                    for item in value
                        .split(|&b| b == b',' || b == b' ')
                        .filter(|i| !i.is_empty())
                    {
                        if item == b"0" {
                            list.push((0, 0));
                            continue;
                        }
                        let parts: Vec<&[u8]> = item.split(|&b| b == b'.').collect();
                        match (parts.as_slice(), parts.get(1).and_then(|f| count(f))) {
                            ([b"1", _], Some(f)) if f > 0 => list.push((1, f)),
                            ([b"2", _], Some(f)) if f > 0 => list.push((2, f)),
                            _ => ok = false,
                        }
                    }
                    format = Some(list);
                    !ok
                }
            }
            _ => false,
        };
        if bad {
            let v = common::quote(ctx, value);
            return common::usage(ctx, &format!("invalid field specifier: {v}")).await;
        }
    }
    let ignore_case = p.has(&JOIN, "ignore-case");
    let [first, second] = p.operands.as_slice() else {
        return common::usage(ctx, "missing operand").await;
    };
    let mut tables: Vec<Vec<Vec<Vec<u8>>>> = Vec::new();
    for name in [first, second] {
        let data = match read_all(ctx, std::slice::from_ref(name)).await {
            Ok(data) => data,
            Err(st) => return st,
        };
        let mut rows = Vec::new();
        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<Vec<u8>> = match sep {
                Some(s) => line.split(|&b| b == s).map(<[u8]>::to_vec).collect(),
                None => line
                    .split(|&b| b == b' ' || b == b'\t')
                    .filter(|f| !f.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect(),
            };
            rows.push(fields);
        }
        tables.push(rows);
    }
    let key = |row: &[Vec<u8>], side: usize| -> Vec<u8> {
        let k = row.get(keys[side] - 1).cloned().unwrap_or_default();
        if ignore_case {
            k.to_ascii_lowercase()
        } else {
            k
        }
    };
    let out_sep = sep.unwrap_or(b' ');
    let widths = [
        tables[0].first().map_or(0, Vec::len),
        tables[1].first().map_or(0, Vec::len),
    ];
    let mut out = Vec::new();
    let mut line = |a: Option<&Vec<Vec<u8>>>, b: Option<&Vec<Vec<u8>>>| {
        let mut fields: Vec<Vec<u8>> = Vec::new();
        let get = |row: Option<&Vec<Vec<u8>>>, n: usize| -> Vec<u8> {
            match row.and_then(|r| r.get(n - 1)) {
                Some(f) if !f.is_empty() => f.clone(),
                _ => empty.clone(),
            }
        };
        let join_key = match (a, b) {
            (Some(a), _) => a.get(keys[0] - 1).cloned().unwrap_or_default(),
            (None, Some(b)) => b.get(keys[1] - 1).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        match &format {
            Some(list) => {
                for &(file, n) in list {
                    fields.push(match file {
                        0 => join_key.clone(),
                        1 => get(a, n),
                        _ => get(b, n),
                    });
                }
            }
            None => {
                fields.push(join_key);
                for (side, row) in [a, b].into_iter().enumerate() {
                    let len = if auto {
                        widths[side]
                    } else {
                        row.map_or(0, Vec::len)
                    };
                    for n in 1..=len {
                        if n != keys[side] {
                            fields.push(if auto {
                                get(row, n)
                            } else {
                                row.and_then(|r| r.get(n - 1)).cloned().unwrap_or_default()
                            });
                        }
                    }
                }
            }
        }
        out.extend_from_slice(&fields.join(&out_sep));
        out.push(b'\n');
    };
    let (mut i, mut j) = (0, 0);
    while i < tables[0].len() && j < tables[1].len() {
        let (ka, kb) = (key(&tables[0][i], 0), key(&tables[1][j], 1));
        match ka.cmp(&kb) {
            std::cmp::Ordering::Less => {
                if unpaired[0] {
                    line(Some(&tables[0][i]), None);
                }
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                if unpaired[1] {
                    line(None, Some(&tables[1][j]));
                }
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let i_end = (i..tables[0].len())
                    .find(|&n| key(&tables[0][n], 0) != ka)
                    .unwrap_or(tables[0].len());
                let j_end = (j..tables[1].len())
                    .find(|&n| key(&tables[1][n], 1) != kb)
                    .unwrap_or(tables[1].len());
                if !only_unpaired {
                    for a in &tables[0][i..i_end] {
                        for b in &tables[1][j..j_end] {
                            line(Some(a), Some(b));
                        }
                    }
                }
                i = i_end;
                j = j_end;
            }
        }
    }
    if unpaired[0] {
        for a in &tables[0][i..] {
            line(Some(a), None);
        }
    }
    if unpaired[1] {
        for b in &tables[1][j..] {
            line(None, Some(b));
        }
    }
    emit(ctx, &out).await
}

/// Random bytes from the host's /dev/urandom.
pub async fn entropy(ctx: &mut UtilCtx<'_>, bytes: &mut [u8]) -> bool {
    let Ok(file) = ctx.open(b"/dev/urandom", OpenOptions::read()).await else {
        return false;
    };
    let mut got = 0;
    while got < bytes.len() {
        match file.read(&mut bytes[got..]).await {
            Ok(n) if n > 0 => got += n,
            _ => return false,
        }
    }
    true
}

pub const SHUF: Spec = Spec {
    name: "shuf",
    opts: &[
        both('e', "echo"),
        both('i', "input-range").value(),
        both('n', "head-count").value(),
        both('o', "output").value(),
        both('r', "repeat"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

pub async fn shuf(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SHUF, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let end = if p.has(&SHUF, "zero-terminated") {
        0u8
    } else {
        b'\n'
    };
    let mut items: Vec<Vec<u8>> = if p.has(&SHUF, "echo") {
        p.operands.clone()
    } else if let Some(range) = p.last(&SHUF, "input-range") {
        let parsed = std::str::from_utf8(range)
            .ok()
            .and_then(|r| r.split_once('-'))
            .and_then(|(lo, hi)| Some((lo.parse::<u64>().ok()?, hi.parse::<u64>().ok()?)))
            .filter(|(lo, hi)| lo <= hi);
        let Some((lo, hi)) = parsed else {
            let range = common::quote(ctx, range);
            return common::usage(ctx, &format!("invalid input range: {range}")).await;
        };
        (lo..=hi).map(|n| n.to_string().into_bytes()).collect()
    } else {
        if p.operands.len() > 1 {
            let extra = common::quote(ctx, &p.operands[1]);
            return common::usage(ctx, &format!("extra operand {extra}")).await;
        }
        let data = match read_all(ctx, &p.operands).await {
            Ok(data) => data,
            Err(st) => return st,
        };
        let mut items: Vec<Vec<u8>> = data.split(|&b| b == end).map(<[u8]>::to_vec).collect();
        if items.last().is_some_and(Vec::is_empty) {
            items.pop();
        }
        items
    };
    let limit = match p.last(&SHUF, "head-count") {
        Some(n) => match count(n) {
            Some(n) => Some(n),
            None => {
                let n = common::quote(ctx, n);
                return common::usage(ctx, &format!("invalid line count: {n}")).await;
            }
        },
        None => None,
    };
    let mut seed = [0u8; 16];
    if !entropy(ctx, &mut seed).await {
        ctx.error("/dev/urandom: cannot read").await;
        return 1;
    }
    let mut state = u128::from_le_bytes(seed) | 1;
    let mut random = |below: usize| -> usize {
        // xorshift, and the high bits of each step.
        state ^= state << 23;
        state ^= state >> 17;
        state ^= state << 26;
        ((state >> 64) as u64 % below as u64) as usize
    };
    let mut out = Vec::new();
    if p.has(&SHUF, "repeat") {
        if items.is_empty() {
            ctx.error("no lines to repeat").await;
            return 1;
        }
        let Some(limit) = limit else {
            return common::usage(ctx, "-r without -n would not end (psh built-in shuf)").await;
        };
        for _ in 0..limit {
            out.extend_from_slice(&items[random(items.len())]);
            out.push(end);
        }
    } else {
        let take = limit.unwrap_or(items.len()).min(items.len());
        for n in 0..take {
            let pick = n + random(items.len() - n);
            items.swap(n, pick);
            out.extend_from_slice(&items[n]);
            out.push(end);
        }
    }
    if let Some(target) = p.last(&SHUF, "output") {
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

pub const SPLIT: Spec = Spec {
    name: "split",
    opts: &[
        both('l', "lines").value(),
        both('b', "bytes").value(),
        both('n', "number").value(),
        both('a', "suffix-length").value(),
        flag('d'),
        long("numeric-suffixes").optional(),
        long("additional-suffix").value(),
    ],
    posix_order: false,
};

pub async fn split(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPLIT, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    if p.operands.len() > 2 {
        let extra = common::quote(ctx, &p.operands[2]);
        return common::usage(ctx, &format!("extra operand {extra}")).await;
    }
    let input: Vec<Vec<u8>> = p.operands.first().cloned().into_iter().collect();
    let prefix = p.operands.get(1).cloned().unwrap_or_else(|| b"x".to_vec());
    let data = match read_all(ctx, &input).await {
        Ok(data) => data,
        Err(st) => return st,
    };
    let mut pieces: Vec<&[u8]> = Vec::new();
    if let Some(size) = p.last(&SPLIT, "bytes") {
        let Some(size) = common::parse_count(size).filter(|n| *n > 0) else {
            let size = common::quote(ctx, size);
            return common::usage(ctx, &format!("invalid number of bytes: {size}")).await;
        };
        pieces.extend(data.chunks(size as usize));
    } else if let Some(n) = p.last(&SPLIT, "number") {
        let by_lines = n.starts_with(b"l/");
        let Some(n) = count(n.strip_prefix(b"l/").unwrap_or(n)).filter(|n| *n > 0) else {
            let n = common::quote(ctx, n);
            return common::usage(ctx, &format!("invalid number of chunks: {n}")).await;
        };
        let share = data.len() / n;
        let mut at = 0;
        for k in 0..n {
            let mut end = if k + 1 == n {
                data.len()
            } else {
                at.max((k + 1) * share)
            };
            if by_lines && k + 1 < n && end > at {
                // A chunk ends with the line that reaches its share.
                end = match data[end - 1..].iter().position(|&b| b == b'\n') {
                    Some(nl) => end + nl,
                    None => data.len(),
                };
            }
            pieces.push(&data[at.min(data.len())..end.min(data.len())]);
            at = end.min(data.len());
        }
    } else {
        let lines = match p.last(&SPLIT, "lines") {
            Some(n) => match count(n).filter(|n| *n > 0) {
                Some(n) => n,
                None => {
                    let n = common::quote(ctx, n);
                    return common::usage(ctx, &format!("invalid number of lines: {n}")).await;
                }
            },
            None => 1000,
        };
        let mut start = 0;
        let mut seen = 0;
        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                seen += 1;
                if seen == lines {
                    pieces.push(&data[start..=i]);
                    start = i + 1;
                    seen = 0;
                }
            }
        }
        if start < data.len() {
            pieces.push(&data[start..]);
        }
    }
    let numeric = p.has(&SPLIT, "d") || p.has(&SPLIT, "numeric-suffixes");
    let length = p.last(&SPLIT, "suffix-length").and_then(count).unwrap_or(2);
    let extra = p
        .last(&SPLIT, "additional-suffix")
        .unwrap_or_default()
        .to_vec();
    let radix: usize = if numeric { 10 } else { 26 };
    if radix
        .checked_pow(length as u32)
        .is_none_or(|most| pieces.len() > most)
    {
        ctx.error("output file suffixes exhausted").await;
        return 1;
    }
    for (n, piece) in pieces.iter().enumerate() {
        let mut suffix = vec![0u8; length];
        let mut rest = n;
        for slot in suffix.iter_mut().rev() {
            let digit = (rest % radix) as u8;
            *slot = if numeric { b'0' + digit } else { b'a' + digit };
            rest /= radix;
        }
        let name = [&prefix[..], &suffix[..], &extra[..]].concat();
        let written = match ctx.open(&name, OpenOptions::write()).await {
            Ok(file) => file.write_all(piece).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            ctx.error(&format!(
                "{}: {}",
                String::from_utf8_lossy(&name),
                error_text(&e)
            ))
            .await;
            return 1;
        }
    }
    0
}
