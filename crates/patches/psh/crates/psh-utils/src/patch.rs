//! patch (nanocode's addition), after GNU patch 2.7 for unified diffs (git's included:
//! new and deleted files, renames): -p -R -N -f -t -s -i -o -d -r -F -b -z -E -l
//! --dry-run --no-backup-if-mismatch. A hunk is looked for where it says it is, then
//! ever further on both sides, then with one and two lines of context ignored (fuzz);
//! what does not apply goes to FILE.rej, and a file that was patched with an offset or
//! fuzz is kept as FILE.orig, as GNU has it. Context, normal and ed diffs are answered
//! as missing. Messages are GNU's; a question GNU would ask a terminal takes its
//! default, as GNU does without one.

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common::Input;
use crate::fsutil::join;

#[derive(Clone)]
struct Hunk {
    old_start: usize,
    new_start: usize,
    /// Each line with its mark: a space, a minus or a plus.
    lines: Vec<(u8, Vec<u8>)>,
    /// The last old or new line has no newline.
    old_open: bool,
    new_open: bool,
    /// The hunk as the patch has it, for a reject.
    text: Vec<u8>,
}

struct FilePatch {
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    rename: Option<(Vec<u8>, Vec<u8>)>,
    hunks: Vec<Hunk>,
    /// The line of the patch its first hunk starts at (its header's first line when
    /// it has none), and the header's text.
    line: usize,
    header: Vec<Vec<u8>>,
    new_mode: Option<u32>,
}

fn range(text: &[u8]) -> Option<(usize, usize)> {
    let text = std::str::from_utf8(text).ok()?;
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

/// A name from a header line: up to a tab, and without quotes.
fn header_name(rest: &[u8]) -> Option<Vec<u8>> {
    let end = rest.iter().position(|&b| b == b'\t').unwrap_or(rest.len());
    let mut name = &rest[..end];
    while name.last().is_some_and(|b| *b == b' ' || *b == b'\r') {
        name = &name[..name.len() - 1];
    }
    if name.len() >= 2 && name[0] == b'"' && name[name.len() - 1] == b'"' {
        name = &name[1..name.len() - 1];
    }
    if name.is_empty() || name == b"/dev/null" {
        return None;
    }
    Some(name.to_vec())
}

enum Parsed {
    Files(Vec<FilePatch>),
    Malformed(usize, Vec<u8>),
    Other(&'static str),
}

fn parse(text: &[u8]) -> Parsed {
    let lines: Vec<&[u8]> = text.split_inclusive(|&b| b == b'\n').collect();
    let mut files: Vec<FilePatch> = Vec::new();
    let mut header: Vec<Vec<u8>> = Vec::new();
    let mut header_at = 1;
    let mut pending: Option<FilePatch> = None;
    let mut i = 0;
    let chomp = |l: &[u8]| -> Vec<u8> {
        let l = l.strip_suffix(b"\n").unwrap_or(l);
        l.strip_suffix(b"\r").unwrap_or(l).to_vec()
    };
    while i < lines.len() {
        let line = lines[i];
        let plain = chomp(line);
        if plain.starts_with(b"*** ") && lines.get(i + 1).is_some_and(|l| l.starts_with(b"--- ")) {
            return Parsed::Other("a context diff");
        }
        if plain.starts_with(b"diff ") {
            if let Some(done) = pending.take() {
                files.push(done);
            }
            header.clear();
            header_at = i + 1;
        }
        if let Some(rest) = plain.strip_prefix(b"--- ")
            && lines.get(i + 1).is_some_and(|l| l.starts_with(b"+++ "))
        {
            if let Some(done) = pending.take() {
                files.push(done);
                header.clear();
                header_at = i + 1;
            }
            let next = chomp(lines[i + 1]);
            header.push(plain.clone());
            header.push(next.clone());
            let mut file = FilePatch {
                old: header_name(rest),
                new: header_name(&next[4..]),
                rename: None,
                hunks: Vec::new(),
                line: header_at,
                header: std::mem::take(&mut header),
                new_mode: None,
            };
            for h in &file.header {
                if let Some(mode) = h
                    .strip_prefix(b"new file mode ")
                    .or(h.strip_prefix(b"new mode "))
                {
                    file.new_mode = std::str::from_utf8(mode)
                        .ok()
                        .and_then(|m| u32::from_str_radix(m.trim(), 8).ok());
                }
            }
            pending = Some(file);
            i += 2;
            continue;
        }
        if plain.starts_with(b"@@ -") {
            let Some(file) = pending.as_mut() else {
                i += 1;
                continue;
            };
            let bad = || Parsed::Malformed(i + 1, plain.clone());
            let start = i + 1;
            let inner = &plain[4..];
            let Some(space) = inner.iter().position(|&b| b == b' ') else {
                return bad();
            };
            let Some((old_start, old_count)) = range(&inner[..space]) else {
                return bad();
            };
            let rest = &inner[space + 1..];
            if rest.first() != Some(&b'+') {
                return bad();
            }
            let end = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
            let Some((new_start, new_count)) = range(&rest[1..end]) else {
                return bad();
            };
            // An empty range names the line before it: what goes in goes after that.
            let mut hunk = Hunk {
                old_start: old_start + usize::from(old_count == 0),
                new_start: new_start + usize::from(new_count == 0),
                lines: Vec::new(),
                old_open: false,
                new_open: false,
                text: line.to_vec(),
            };
            let (mut old_left, mut new_left) = (old_count, new_count);
            i += 1;
            while old_left > 0 || new_left > 0 {
                let Some(body) = lines.get(i) else {
                    return Parsed::Malformed(i + 1, Vec::new());
                };
                let (mark, content) = match body.first() {
                    Some(b' ') => (b' ', &body[1..]),
                    Some(b'-') => (b'-', &body[1..]),
                    Some(b'+') => (b'+', &body[1..]),
                    // An empty context line whose space an editor took.
                    Some(b'\n') | Some(b'\r') => (b' ', &body[..]),
                    _ => return Parsed::Malformed(i + 1, chomp(body)),
                };
                if (mark != b'+' && old_left == 0) || (mark != b'-' && new_left == 0) {
                    return Parsed::Malformed(i + 1, chomp(body));
                }
                if mark != b'+' {
                    old_left -= 1;
                }
                if mark != b'-' {
                    new_left -= 1;
                }
                let mut content = content.to_vec();
                if content.last() != Some(&b'\n') {
                    content.push(b'\n');
                }
                hunk.lines.push((mark, content));
                hunk.text.extend_from_slice(body);
                i += 1;
                if lines.get(i).is_some_and(|l| l.starts_with(b"\\ ")) {
                    hunk.text.extend_from_slice(lines[i]);
                    if mark != b'+' {
                        hunk.old_open = true;
                    }
                    if mark != b'-' {
                        hunk.new_open = true;
                    }
                    i += 1;
                }
            }
            if file.hunks.is_empty() {
                file.line = start;
            }
            file.hunks.push(hunk);
            continue;
        }
        if pending.as_ref().is_none_or(|p| !p.hunks.is_empty()) || !plain.starts_with(b"@@") {
            if let Some(done) = pending.take_if(|p| !p.hunks.is_empty()) {
                files.push(done);
                header.clear();
                header_at = i + 1;
            }
            header.push(plain.clone());
            // A rename with nothing else changed has no hunks.
            if let Some(to) = plain.strip_prefix(b"rename to ") {
                let from = header
                    .iter()
                    .rev()
                    .find_map(|h| h.strip_prefix(b"rename from ").map(<[u8]>::to_vec));
                if let Some(from) = from {
                    let ahead = lines[i + 1..]
                        .iter()
                        .take_while(|l| !l.starts_with(b"diff "))
                        .any(|l| l.starts_with(b"--- "));
                    if !ahead {
                        files.push(FilePatch {
                            old: Some(from.clone()),
                            new: Some(to.to_vec()),
                            rename: Some((from, to.to_vec())),
                            hunks: Vec::new(),
                            line: header_at,
                            header: std::mem::take(&mut header),
                            new_mode: None,
                        });
                    }
                }
            }
        }
        i += 1;
    }
    if let Some(done) = pending {
        files.push(done);
    }
    for file in &mut files {
        let from = file
            .header
            .iter()
            .find_map(|h| h.strip_prefix(b"rename from ").map(<[u8]>::to_vec));
        let to = file
            .header
            .iter()
            .find_map(|h| h.strip_prefix(b"rename to ").map(<[u8]>::to_vec));
        if let (Some(from), Some(to), None) = (from, to, &file.rename) {
            file.rename = Some((from, to));
        }
    }
    Parsed::Files(files)
}

/// A name of the patch without its leading components: the base name without -p,
/// else what is left after count slashes, and nothing when it has fewer.
fn strip(name: &[u8], count: Option<usize>) -> Vec<u8> {
    match count {
        None => name.rsplit(|&b| b == b'/').next().unwrap_or(name).to_vec(),
        Some(n) => {
            let mut rest = name;
            for _ in 0..n {
                match rest.iter().position(|&b| b == b'/') {
                    Some(slash) => {
                        rest = &rest[slash + 1..];
                        while rest.first() == Some(&b'/') {
                            rest = &rest[1..];
                        }
                    }
                    None => return Vec::new(),
                }
            }
            rest.to_vec()
        }
    }
}

/// A hunk as a unified diff has it, for a reject of a patch applied in reverse.
fn render(hunk: &Hunk) -> Vec<u8> {
    let old = hunk.lines.iter().filter(|l| l.0 != b'+').count();
    let new = hunk.lines.iter().filter(|l| l.0 != b'-').count();
    let range = |start: usize, count: usize| match count {
        0 => format!("{},0", start - 1),
        1 => start.to_string(),
        _ => format!("{start},{count}"),
    };
    let mut out = format!(
        "@@ -{} +{} @@\n",
        range(hunk.old_start, old),
        range(hunk.new_start, new)
    )
    .into_bytes();
    let last_old = hunk.lines.iter().rposition(|l| l.0 != b'+');
    let last_new = hunk.lines.iter().rposition(|l| l.0 != b'-');
    for (i, (mark, line)) in hunk.lines.iter().enumerate() {
        out.push(*mark);
        out.extend_from_slice(line);
        if (hunk.old_open && Some(i) == last_old && *mark != b'+')
            || (hunk.new_open && Some(i) == last_new && *mark != b'-')
        {
            out.extend_from_slice(b"\\ No newline at end of file\n");
        }
    }
    out
}

struct Options {
    strip: Option<usize>,
    reverse: bool,
    forward: bool,
    force: bool,
    batch: bool,
    silent: bool,
    dry_run: bool,
    remove_empty: bool,
    loose: bool,
    backup: bool,
    backup_if_mismatch: bool,
    suffix: Vec<u8>,
    fuzz: usize,
    input: Option<Vec<u8>>,
    output: Option<Vec<u8>>,
    reject: Option<Vec<u8>>,
    dir: Option<Vec<u8>>,
    file: Option<Vec<u8>>,
}

fn lines_of(data: &[u8]) -> Vec<Vec<u8>> {
    data.split_inclusive(|&b| b == b'\n')
        .map(|l| {
            let mut l = l.to_vec();
            if l.last() != Some(&b'\n') {
                l.push(b'\n');
            }
            l
        })
        .collect()
}

fn same_line(a: &[u8], b: &[u8], loose: bool) -> bool {
    if !loose {
        return a == b;
    }
    let squeeze = |l: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        for part in l
            .split(|b| b.is_ascii_whitespace())
            .filter(|p| !p.is_empty())
        {
            if !out.is_empty() {
                out.push(b' ');
            }
            out.extend_from_slice(part);
        }
        out
    };
    squeeze(a) == squeeze(b)
}

/// GNU's locate_hunk: the line (from 1) the hunk's old lines are at, or 0. frozen is
/// the last line an earlier hunk took; offset is what earlier hunks were found off by
/// and is updated.
#[allow(clippy::too_many_arguments)]
fn locate(
    file: &[Vec<u8>],
    old: &[&[u8]],
    prefix: usize,
    suffix: usize,
    first: usize,
    frozen: usize,
    offset: &mut isize,
    fuzz: usize,
    loose: bool,
) -> usize {
    let first_guess = first as isize + *offset;
    let pat = old.len() as isize;
    let input = file.len() as isize;
    let max_pos = input - first_guess - pat + 1;
    let mut max_neg = first_guess - frozen as isize - 1;
    let context = prefix.max(suffix) as isize;
    let mut prefix_fuzz = fuzz as isize + prefix as isize - context;
    let suffix_fuzz = fuzz as isize + suffix as isize - context;
    if pat == 0 {
        return first_guess.max(0) as usize;
    }
    if first_guess <= max_neg {
        max_neg = first_guess - 1;
    }
    let matches = |at: isize, pre: isize, suf: isize| -> bool {
        // The old lines without pre at the start and suf at the end, at line at.
        if at < 1 {
            return false;
        }
        let (pre, suf) = (pre as usize, suf as usize);
        if pre + suf > old.len() {
            return false;
        }
        let start = at as usize - 1 + pre;
        let wanted = &old[pre..old.len() - suf];
        if start + wanted.len() > file.len() {
            return false;
        }
        wanted
            .iter()
            .zip(&file[start..])
            .all(|(a, b)| same_line(a, b, loose))
    };
    if prefix_fuzz < 0 && first <= 1 {
        // Less context before than after: it can only be the start of the file.
        if suffix_fuzz < 0 && pat != input {
            return 0;
        }
        let off = 1 - first_guess;
        if frozen <= prefix && off <= max_pos && matches(first_guess + off, 0, suffix_fuzz.max(0)) {
            *offset += off;
            return (first_guess + off) as usize;
        }
        return 0;
    }
    prefix_fuzz = prefix_fuzz.max(0);
    if suffix_fuzz < 0 {
        // Less context after than before: it can only be the end of the file.
        let off = first_guess - (input - pat + 1);
        if off <= max_neg && matches(first_guess - off, prefix_fuzz, 0) {
            *offset -= off;
            return (first_guess - off) as usize;
        }
        return 0;
    }
    let max = max_pos.max(max_neg);
    let mut off = 0;
    while off <= max {
        if off <= max_pos && matches(first_guess + off, prefix_fuzz, suffix_fuzz) {
            *offset += off;
            return (first_guess + off) as usize;
        }
        if off > 0 && off <= max_neg && matches(first_guess - off, prefix_fuzz, suffix_fuzz) {
            *offset -= off;
            return (first_guess - off) as usize;
        }
        off += 1;
    }
    0
}

fn reversed(hunk: &Hunk) -> Hunk {
    let mut lines: Vec<(u8, Vec<u8>)> = Vec::with_capacity(hunk.lines.len());
    // Within a change the new lines of the reverse come after its old ones.
    let mut i = 0;
    while i < hunk.lines.len() {
        if hunk.lines[i].0 == b' ' {
            lines.push(hunk.lines[i].clone());
            i += 1;
            continue;
        }
        let start = i;
        while i < hunk.lines.len() && hunk.lines[i].0 != b' ' {
            i += 1;
        }
        let run = &hunk.lines[start..i];
        lines.extend(
            run.iter()
                .filter(|l| l.0 == b'+')
                .map(|l| (b'-', l.1.clone())),
        );
        lines.extend(
            run.iter()
                .filter(|l| l.0 == b'-')
                .map(|l| (b'+', l.1.clone())),
        );
    }
    Hunk {
        old_start: hunk.new_start,
        new_start: hunk.old_start,
        lines,
        old_open: hunk.new_open,
        new_open: hunk.old_open,
        text: hunk.text.clone(),
    }
}

/// A hunk that did not apply, as the reject file has it: the way it was tried.
fn rejected(reverse: bool, hunk: &Hunk, original: &Hunk) -> Vec<u8> {
    if reverse {
        render(hunk)
    } else {
        original.text.clone()
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut o = Options {
        strip: None,
        reverse: false,
        forward: false,
        force: false,
        batch: false,
        silent: false,
        dry_run: false,
        remove_empty: false,
        loose: false,
        backup: false,
        backup_if_mismatch: true,
        suffix: b".orig".to_vec(),
        fuzz: 2,
        input: None,
        output: None,
        reject: None,
        dir: None,
        file: None,
    };
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    let lacks = |what: String| what;
    let mut missing: Option<String> = None;
    while i < argv.len() {
        let arg = argv[i].clone();
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            operands.push(arg);
            continue;
        }
        let text = String::from_utf8_lossy(&arg).into_owned();
        let mut take = |attached: &str| -> Option<Vec<u8>> {
            if !attached.is_empty() {
                return Some(attached.as_bytes().to_vec());
            }
            let v = argv.get(i).cloned();
            i += 1;
            v
        };
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, v),
                None => (long, ""),
            };
            match name {
                "strip" => {
                    o.strip = take(value).and_then(|v| String::from_utf8_lossy(&v).parse().ok())
                }
                "reverse" => o.reverse = true,
                "forward" => o.forward = true,
                "force" => o.force = true,
                "batch" => o.batch = true,
                "silent" | "quiet" => o.silent = true,
                "dry-run" => o.dry_run = true,
                "remove-empty-files" => o.remove_empty = true,
                "ignore-whitespace" => o.loose = true,
                "backup" => o.backup = true,
                "no-backup-if-mismatch" => o.backup_if_mismatch = false,
                "backup-if-mismatch" => o.backup_if_mismatch = true,
                "suffix" => o.suffix = take(value).unwrap_or_default(),
                "fuzz" => {
                    o.fuzz = take(value)
                        .and_then(|v| String::from_utf8_lossy(&v).parse().ok())
                        .unwrap_or(2)
                }
                "input" => o.input = take(value),
                "output" => o.output = take(value),
                "reject-file" => o.reject = take(value),
                "directory" => o.dir = take(value),
                "unified" | "verbose" | "posix" | "binary" => {}
                _ => missing = Some(lacks(format!("the option {text}"))),
            }
            continue;
        }
        let letters = &text[1..];
        let mut k = 0;
        let bytes = letters.as_bytes();
        while k < bytes.len() {
            let c = bytes[k];
            k += 1;
            let rest = &letters[k..];
            match c {
                b'p' => {
                    o.strip = take(rest).and_then(|v| String::from_utf8_lossy(&v).parse().ok());
                    break;
                }
                b'i' => {
                    o.input = take(rest);
                    break;
                }
                b'o' => {
                    o.output = take(rest);
                    break;
                }
                b'r' => {
                    o.reject = take(rest);
                    break;
                }
                b'd' => {
                    o.dir = take(rest);
                    break;
                }
                b'z' => {
                    o.suffix = take(rest).unwrap_or_default();
                    break;
                }
                b'F' => {
                    o.fuzz = take(rest)
                        .and_then(|v| String::from_utf8_lossy(&v).parse().ok())
                        .unwrap_or(2);
                    break;
                }
                b'R' => o.reverse = true,
                b'N' => o.forward = true,
                b'f' => o.force = true,
                b't' => o.batch = true,
                b's' => o.silent = true,
                b'E' => o.remove_empty = true,
                b'l' => o.loose = true,
                b'b' => o.backup = true,
                b'u' | b'v' => {}
                _ => {
                    missing = Some(lacks(format!("the option -{}", c as char)));
                    break;
                }
            }
        }
    }
    if let Some(what) = missing {
        if let Some(status) = ctx.run_program(argv.to_vec()).await {
            return status;
        }
        ctx.error(&format!(
            "{what} (this simulated shell has no patch for it: it applies unified diffs)"
        ))
        .await;
        return 127;
    }
    if let Some(file) = operands.first() {
        o.file = Some(file.clone());
    }
    if o.input.is_none()
        && let Some(patch) = operands.get(1)
    {
        o.input = Some(patch.clone());
    }
    let at = |o: &Options, name: &[u8]| -> Vec<u8> {
        match &o.dir {
            Some(dir) if name.first() != Some(&b'/') => join(dir, name),
            _ => name.to_vec(),
        }
    };
    let mut text = Vec::new();
    let source = o.input.clone().unwrap_or_else(|| b"-".to_vec());
    let read = match Input::open(ctx, &source).await {
        Ok(input) => input.read_to_end(ctx, &mut text).await,
        Err(e) => Err(e),
    };
    if let Err(e) = read {
        ctx.error(&format!(
            "**** Can't open patch file {} : {}",
            String::from_utf8_lossy(&source),
            error_text(&e)
        ))
        .await;
        return 2;
    }
    if text.is_empty() {
        return 0;
    }
    let files = match parse(&text) {
        Parsed::Files(files) => files,
        Parsed::Malformed(line, content) => {
            ctx.error(&format!(
                "**** malformed patch at line {line}: {}\n",
                String::from_utf8_lossy(&content)
            ))
            .await;
            return 2;
        }
        Parsed::Other(what) => {
            if let Some(status) = ctx.run_program(argv.to_vec()).await {
                return status;
            }
            ctx.error(&format!(
                "{what} (this simulated shell has no patch for it: it applies unified diffs)"
            ))
            .await;
            return 127;
        }
    };
    if files.is_empty() {
        ctx.error("**** Only garbage was found in the patch input.")
            .await;
        return 2;
    }
    let mut out = Vec::new();
    macro_rules! say {
        ($($arg:tt)*) => {
            if !o.silent {
                out.extend_from_slice(format!($($arg)*).as_bytes());
            }
        };
    }
    let mut status = 0;
    for patch in &files {
        ctx.check().await;
        let (from, to) = if o.reverse {
            (&patch.new, &patch.old)
        } else {
            (&patch.old, &patch.new)
        };
        let creates = from.is_none();
        let deletes = to.is_none();
        // The file: the one named on the command line, else the patch's name that is
        // there (the new one first), else the new name for a file to create.
        let mut target: Option<Vec<u8>> = o.file.clone();
        if target.is_none() {
            let candidates: Vec<Vec<u8>> = [to, from]
                .into_iter()
                .flatten()
                .map(|n| strip(n, o.strip))
                .filter(|n| !n.is_empty())
                .collect();
            for candidate in &candidates {
                if ctx.stat(&at(&o, candidate), true).await.is_ok() {
                    target = Some(candidate.clone());
                    break;
                }
            }
            if let (None, Some((source, _))) = (&target, &patch.rename) {
                let source = strip(source, o.strip.map(|n| n.saturating_sub(1)));
                if ctx.stat(&at(&o, &source), true).await.is_ok() {
                    target = Some(source);
                }
            }
            if target.is_none() && creates {
                target = candidates.first().cloned();
            }
        }
        let total = patch.hunks.len();
        let Some(target) = target else {
            let _ = ctx.write(&std::mem::take(&mut out)).await;
            let mut text = format!("can't find file to patch at input line {}\n", patch.line);
            text.push_str("Perhaps you used the wrong -p or --strip option?\n");
            text.push_str("The text leading up to this was:\n--------------------------\n");
            for h in &patch.header {
                text.push_str(&format!("|{}\n", String::from_utf8_lossy(h)));
            }
            text.push_str("--------------------------\n");
            if o.force || o.batch {
                text.push_str("No file to patch.  Skipping patch.\n");
            } else {
                text.push_str("File to patch: \nSkip this patch? [y] \nSkipping patch.\n");
            }
            text.push_str(&format!(
                "{total} out of {total} hunk{} ignored\n",
                plural(total)
            ));
            let _ = ctx.write(text.as_bytes()).await;
            status = status.max(1);
            continue;
        };
        let shown = String::from_utf8_lossy(&target).into_owned();
        let path = at(&o, &target);
        let renamed_to = match &patch.rename {
            // git's rename lines name the files without the a/ and b/ of its headers.
            Some((a, b)) => Some(strip(
                if o.reverse { a } else { b },
                o.strip.map(|n| n.saturating_sub(1)),
            )),
            None => None,
        };
        let out_name = o
            .output
            .clone()
            .or_else(|| renamed_to.clone())
            .unwrap_or_else(|| target.clone());
        let out_path = at(&o, &out_name);
        let mut data = Vec::new();
        let existed = match Input::open(ctx, &path).await {
            Ok(input) => input.read_to_end(ctx, &mut data).await.is_ok(),
            Err(_) => false,
        };
        let mut reverse = o.reverse;
        let mut skip = false;
        let mut apply_anyway = false;
        let mut save_rejects = true;
        // A patch that makes a file that is there: made already, or the other way.
        let there = creates && existed && !data.is_empty() && !patch.hunks.is_empty();
        if there {
            say!("The next patch would create the file {shown},\nwhich already exists!");
            if o.forward {
                say!("  Skipping patch.\n");
                skip = true;
            } else if o.force {
                say!("  Applying it anyway.\n");
                apply_anyway = true;
            } else if o.batch {
                say!("  Assuming -R.\n");
                reverse = !reverse;
            } else {
                say!("  Assume -R? [n] \nApply anyway? [n] \nSkipping patch.\n");
                skip = true;
            }
            save_rejects = !skip;
        }
        let verb = if o.dry_run { "checking" } else { "patching" };
        match &renamed_to {
            _ if skip => {}
            Some(new_name) if *new_name != target => {
                say!(
                    "{verb} file {} (renamed from {shown})\n",
                    String::from_utf8_lossy(new_name)
                );
            }
            _ if o.output.is_some() => {
                say!(
                    "{verb} file {} (read from {shown})\n",
                    String::from_utf8_lossy(&out_name)
                );
            }
            _ => say!("{verb} file {shown}\n"),
        }
        let meta = ctx.stat(&path, true).await.ok();
        let mut file = lines_of(&data);
        let mut open_end = !data.is_empty() && data.last() != Some(&b'\n');
        let mut result: Vec<Vec<u8>> = Vec::new();
        let mut frozen = 0usize;
        let mut offset = 0isize;
        let mut failed: Vec<Vec<u8>> = Vec::new();
        let mut mismatch = false;
        for (n, original) in patch.hunks.iter().enumerate() {
            let number = n + 1;
            let mut hunk = if reverse {
                reversed(original)
            } else {
                original.clone()
            };
            if skip {
                failed.push(rejected(reverse, &hunk, original));
                continue;
            }
            let mut fuzz = 0;
            let mut found;
            loop {
                let old: Vec<&[u8]> = hunk
                    .lines
                    .iter()
                    .filter(|l| l.0 != b'+')
                    .map(|l| &l.1[..])
                    .collect();
                let prefix = hunk.lines.iter().take_while(|l| l.0 == b' ').count();
                let suffix = hunk.lines.iter().rev().take_while(|l| l.0 == b' ').count();
                found = locate(
                    &file,
                    &old,
                    prefix,
                    suffix,
                    hunk.old_start,
                    frozen,
                    &mut offset,
                    fuzz,
                    o.loose,
                );
                if found == 0 || fuzz > 0 || offset != 0 {
                    mismatch = true;
                }
                if number == 1 && found == 0 && !(o.force || apply_anyway) && reverse == o.reverse {
                    // Applied already, or made the other way around?
                    let other = reversed(&hunk);
                    let old: Vec<&[u8]> = other
                        .lines
                        .iter()
                        .filter(|l| l.0 != b'+')
                        .map(|l| &l.1[..])
                        .collect();
                    let prefix = other.lines.iter().take_while(|l| l.0 == b' ').count();
                    let suffix = other.lines.iter().rev().take_while(|l| l.0 == b' ').count();
                    let mut probe = offset;
                    let there = locate(
                        &file,
                        &old,
                        prefix,
                        suffix,
                        other.old_start,
                        frozen,
                        &mut probe,
                        fuzz,
                        o.loose,
                    );
                    if there != 0 {
                        let what = if reverse {
                            "Unreversed"
                        } else {
                            "Reversed (or previously applied)"
                        };
                        if o.forward || !(o.force && o.silent) {
                            say!("{what} patch detected!");
                        }
                        if o.forward {
                            say!("  Skipping patch.\n");
                            skip = true;
                        } else if o.force {
                            say!("  Applying it anyway.\n");
                            apply_anyway = true;
                        } else if o.batch {
                            say!(
                                "{}",
                                if reverse {
                                    "  Ignoring -R.\n"
                                } else {
                                    "  Assuming -R.\n"
                                }
                            );
                            reverse = !reverse;
                            hunk = other;
                            offset = probe;
                            found = there;
                        } else {
                            say!(
                                "{}",
                                if reverse {
                                    "  Ignore -R? [n] \n"
                                } else {
                                    "  Assume -R? [n] \n"
                                }
                            );
                            say!("Apply anyway? [n] \n");
                            say!("Skipping patch.\n");
                            skip = true;
                        }
                    }
                }
                if skip || found != 0 {
                    break;
                }
                fuzz += 1;
                if fuzz > o.fuzz {
                    break;
                }
            }
            let landed = hunk.new_start as isize + offset;
            if skip {
                failed.push(rejected(reverse, &hunk, original));
                continue;
            }
            if found == 0 {
                failed.push(rejected(reverse, &hunk, original));
                say!(
                    "Hunk #{number} FAILED at {}.\n",
                    hunk.old_start as isize + offset
                );
                continue;
            }
            // Apply: the file's lines up to it, then the hunk: its context is the
            // file's own lines, its minus lines are passed over, its plus lines go in.
            let mut cursor = found.max(1) - 1;
            result.extend(
                file[frozen.min(cursor)..cursor.min(file.len())]
                    .iter()
                    .cloned(),
            );
            for (mark, line) in &hunk.lines {
                match mark {
                    b'+' => result.push(line.clone()),
                    b'-' => cursor += 1,
                    _ => {
                        if let Some(own) = file.get(cursor) {
                            result.push(own.clone());
                        }
                        cursor += 1;
                    }
                }
            }
            frozen = cursor.min(file.len());
            if cursor >= file.len() {
                // The hunk reached the end: it says how the file ends.
                open_end = hunk.new_open;
            }
            if fuzz > 0 || offset != 0 {
                say!("Hunk #{number} succeeded at {landed}");
                if fuzz > 0 {
                    say!(" with fuzz {fuzz}");
                }
                if offset != 0 {
                    say!(" (offset {offset} line{})", plural(offset.unsigned_abs()));
                }
                say!(".\n");
            }
        }
        result.extend(file[frozen.min(file.len())..].iter().cloned());
        file = result;
        let mut new_data = file.concat();
        if open_end && new_data.last() == Some(&b'\n') {
            new_data.pop();
        }
        let reject_name = o.reject.clone().unwrap_or_else(|| {
            let mut name = out_name.clone();
            name.extend_from_slice(b".rej");
            name
        });
        // The file as it was is kept when asked for, and when the patch did not fit
        // where and as it said.
        let keep_original = !o.dry_run && (o.backup || (mismatch && o.backup_if_mismatch && !skip));
        if keep_original {
            if existed {
                let mut backup = path.clone();
                backup.extend_from_slice(&o.suffix);
                if let Ok(file) = ctx.open(&backup, OpenOptions::write()).await {
                    let _ = file.write_all(&data).await;
                }
            } else if !creates {
                say!("Cannot stat file {shown}, skipping backup\n");
            }
        }
        if !failed.is_empty() {
            status = status.max(1);
            let what = if skip { "ignored" } else { "FAILED" };
            let mut line = format!(
                "{} out of {total} hunk{} {what}",
                failed.len(),
                plural(total)
            );
            if !o.dry_run && save_rejects {
                line.push_str(&format!(
                    " -- saving rejects to file {}",
                    String::from_utf8_lossy(&reject_name)
                ));
            }
            line.push('\n');
            // The count is said to the silent too.
            out.extend_from_slice(line.as_bytes());
            if !o.dry_run && save_rejects {
                // The names are the patch's, as the patch was applied.
                let named = |name: &Option<Vec<u8>>| match name {
                    Some(name) if !strip(name, o.strip).is_empty() => strip(name, o.strip),
                    _ => out_name.clone(),
                };
                let mut reject = Vec::new();
                reject.extend_from_slice(b"--- ");
                reject.extend_from_slice(&named(from));
                reject.extend_from_slice(b"\n+++ ");
                reject.extend_from_slice(&named(to));
                reject.push(b'\n');
                for text in &failed {
                    reject.extend_from_slice(text);
                }
                if let Ok(file) = ctx.open(&at(&o, &reject_name), OpenOptions::write()).await {
                    let _ = file.write_all(&reject).await;
                }
            }
        }
        if o.dry_run || (failed.len() == total && total > 0 && (!existed || new_data == data)) {
            continue;
        }
        if deletes && !new_data.is_empty() && o.output.is_none() {
            say!("Not deleting file {shown} as content differs from patch\n");
            status = status.max(1);
        }
        let gone = (deletes || o.remove_empty) && new_data.is_empty() && o.output.is_none();
        if gone {
            let _ = ctx.remove_file(&path).await;
            continue;
        }
        if let Some(slash) = out_path.iter().rposition(|&b| b == b'/')
            && slash > 0
        {
            let mut dir = Vec::new();
            for part in out_path[..slash].split(|&b| b == b'/') {
                if !dir.is_empty() || out_path.first() == Some(&b'/') {
                    dir.push(b'/');
                }
                dir.extend_from_slice(part);
                if !part.is_empty() && ctx.stat(&dir, true).await.is_err() {
                    let _ = ctx.mkdir(&dir, 0o777 & !ctx.umask()).await;
                }
            }
        }
        let mode = patch
            .new_mode
            .map(|m| m & 0o777)
            .or(meta.map(|m| m.mode & 0o777));
        let options = OpenOptions {
            mode,
            ..OpenOptions::write()
        };
        // GNU puts a new file in the old one's place: one that may not be written
        // still can be patched.
        if meta.is_some_and(|m| m.mode & 0o200 == 0) && out_path == path {
            let _ = ctx.remove_file(&path).await;
        }
        let written = match ctx.open(&out_path, options).await {
            Ok(file) => file.write_all(&new_data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            let _ = ctx.write(&std::mem::take(&mut out)).await;
            ctx.error(&format!(
                "**** Can't create file {} : {}",
                String::from_utf8_lossy(&out_name),
                error_text(&e)
            ))
            .await;
            return 2;
        }
        if let Some(mode) = mode {
            let _ = ctx.set_mode(&out_path, mode).await;
        }
        if out_path != path && o.output.is_none() && existed {
            let _ = ctx.remove_file(&path).await;
        }
    }
    let _ = ctx.write(&out).await;
    status
}
