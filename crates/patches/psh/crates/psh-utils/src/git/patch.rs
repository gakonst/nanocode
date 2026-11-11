//! The text of a change: which lines differ (gitoxide's imara-diff: Myers with xdiff's
//! heuristics and its indent heuristic for where a change sits), written as git writes
//! unified hunks, with the line a hunk is inside of in its header.

use std::io::Write;
use std::ops::Range;

use gix_imara_diff::sources::byte_lines;
use gix_imara_diff::{Algorithm, Diff, InternedInput};

use super::Res;

/// git's test for a binary file: a NUL in its first 8000 bytes.
pub fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(8000)].contains(&0)
}

/// The white space a comparison of lines passes over (-w, -b,
/// --ignore-space-at-eol).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Space {
    #[default]
    Exact,
    AtEol,
    Change,
    All,
}

/// A text with the white space a comparison passes over taken out of each line: as
/// many lines as it had.
fn squeezed(text: &[u8], space: Space) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for line in text.split_inclusive(|&b| b == b'\n') {
        let (body, end) = match line.strip_suffix(b"\n") {
            Some(body) => (body, &b"\n"[..]),
            None => (line, &b""[..]),
        };
        let mut kept = body.len();
        while kept > 0 && body[kept - 1].is_ascii_whitespace() {
            kept -= 1;
        }
        let body = &body[..kept];
        match space {
            Space::Exact | Space::AtEol => out.extend_from_slice(body),
            Space::All => out.extend(body.iter().filter(|b| !b.is_ascii_whitespace())),
            Space::Change => {
                let mut blank = false;
                for &b in body {
                    if b.is_ascii_whitespace() {
                        blank = true;
                        continue;
                    }
                    if blank {
                        out.push(b' ');
                        blank = false;
                    }
                    out.push(b);
                }
            }
        }
        out.extend_from_slice(end);
    }
    out
}

/// Two texts by line and the lines that differ.
pub struct TextDiff<'a> {
    pub old: Vec<&'a [u8]>,
    pub new: Vec<&'a [u8]>,
    /// Lines of old replaced by lines of new, in order.
    pub changes: Vec<(Range<usize>, Range<usize>)>,
}

impl<'a> TextDiff<'a> {
    pub fn new(old: &'a [u8], new: &'a [u8]) -> TextDiff<'a> {
        TextDiff::with(old, new, Space::Exact)
    }

    /// The lines that differ when the white space named is passed over.
    pub fn with(old: &'a [u8], new: &'a [u8], space: Space) -> TextDiff<'a> {
        let squeezed =
            (space != Space::Exact).then(|| (squeezed(old, space), squeezed(new, space)));
        let input = match &squeezed {
            Some((a, b)) => InternedInput::new(byte_lines(a), byte_lines(b)),
            None => InternedInput::new(byte_lines(old), byte_lines(new)),
        };
        let mut diff = Diff::compute(Algorithm::Myers, &input);
        diff.postprocess_lines(&input);
        let changes = diff
            .hunks()
            .map(|h| {
                (
                    h.before.start as usize..h.before.end as usize,
                    h.after.start as usize..h.after.end as usize,
                )
            })
            .collect();
        TextDiff {
            old: byte_lines(old).collect(),
            new: byte_lines(new).collect(),
            changes,
        }
    }

    pub fn added(&self) -> usize {
        self.changes.iter().map(|(_, b)| b.len()).sum()
    }

    pub fn removed(&self) -> usize {
        self.changes.iter().map(|(a, _)| a.len()).sum()
    }

    /// The line a hunk header names: the nearest line above that starts a function, as
    /// git's default rule has it (a letter, an underscore or a dollar sign first).
    fn function(&self, from: usize, until: Option<usize>) -> Option<&'a [u8]> {
        let mut at = from as isize;
        let stop = until.map_or(-1, |u| u as isize);
        while at > stop && at >= 0 {
            let line = self.old.get(at as usize)?;
            if line
                .first()
                .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_' || c == b'$')
            {
                let mut len = line.len().min(80);
                while len > 0 && line[len - 1].is_ascii_whitespace() {
                    len -= 1;
                }
                return Some(&line[..len]);
            }
            at -= 1;
        }
        None
    }

    /// The hunks with context lines of context around each change.
    pub fn write(&self, out: &mut dyn Write, context: usize) -> Res<()> {
        let mut i = 0;
        let mut function: &[u8] = b"";
        let mut searched: Option<usize> = None;
        while i < self.changes.len() {
            let mut j = i;
            while j + 1 < self.changes.len()
                && self.changes[j + 1].0.start - self.changes[j].0.end <= 2 * context
            {
                j += 1;
            }
            let (first, last) = (&self.changes[i], &self.changes[j]);
            let s1 = first.0.start.saturating_sub(context);
            let s2 = first.1.start.saturating_sub(context);
            let e1 = (last.0.end + context).min(self.old.len());
            let e2 = (last.1.end + context).min(self.new.len());
            let range = |start: usize, len: usize| match len {
                0 => format!("{start},0"),
                1 => format!("{}", start + 1),
                _ => format!("{},{len}", start + 1),
            };
            write!(out, "@@ -{} +{} @@", range(s1, e1 - s1), range(s2, e2 - s2))?;
            if s1 > 0 {
                if let Some(found) = self.function(s1 - 1, searched) {
                    function = found;
                }
                searched = Some(s1 - 1);
            }
            if !function.is_empty() {
                out.write_all(b" ")?;
                out.write_all(function)?;
            }
            out.write_all(b"\n")?;
            // The lines around a change are the new text's, as git prints them: the
            // two differ there when white space is passed over.
            let mut at = s2;
            for (old, new) in &self.changes[i..=j] {
                for line in &self.new[at..new.start] {
                    line_out(out, b' ', line)?;
                }
                for line in &self.old[old.clone()] {
                    line_out(out, b'-', line)?;
                }
                for line in &self.new[new.clone()] {
                    line_out(out, b'+', line)?;
                }
                at = new.end;
            }
            for line in &self.new[at..e2] {
                line_out(out, b' ', line)?;
            }
            i = j + 1;
        }
        Ok(())
    }
}

fn line_out(out: &mut dyn Write, mark: u8, line: &[u8]) -> Res<()> {
    out.write_all(&[mark])?;
    out.write_all(line)?;
    if line.last() != Some(&b'\n') {
        out.write_all(b"\n\\ No newline at end of file\n")?;
    }
    Ok(())
}

fn trailing_blank(lines: &[&[u8]]) -> usize {
    lines
        .iter()
        .rev()
        .take_while(|l| l.iter().all(u8::is_ascii_whitespace))
        .count()
}

/// git diff --check for one file: what is wrong with the lines it adds, under git's
/// default rule (trailing whitespace, a space before a tab in the indent, a new blank
/// line at the end of the file) and leftover conflict markers. True when it found any.
pub fn check(out: &mut dyn Write, name: &[u8], old: &[u8], new: &[u8]) -> Res<bool> {
    let diff = TextDiff::new(old, new);
    let mut found = false;
    for (_, added) in &diff.changes {
        for at in added.clone() {
            let line = diff.new[at];
            let text = line.strip_suffix(b"\n").unwrap_or(line);
            let marker = text.len() > 7
                && matches!(text[0], b'<' | b'=' | b'>' | b'|')
                && text[..7].iter().all(|&b| b == text[0])
                && text[7].is_ascii_whitespace();
            let marker = marker
                || (line.len() == 8
                    && matches!(line[0], b'<' | b'=' | b'>' | b'|')
                    && line[..7].iter().all(|&b| b == line[0])
                    && line[7] == b'\n');
            if marker {
                found = true;
                out.write_all(name)?;
                writeln!(out, ":{}: leftover conflict marker", at + 1)?;
            }
            let mut end = text.len();
            while end > 0 && text[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            let trailing = end < text.len();
            let mut space_before_tab = false;
            let mut written = 0;
            for (i, &b) in text[..end].iter().enumerate() {
                match b {
                    b' ' => {}
                    b'\t' => {
                        if written < i {
                            space_before_tab = true;
                        }
                        written = i + 1;
                    }
                    _ => break,
                }
            }
            if !trailing && !space_before_tab {
                continue;
            }
            found = true;
            let mut what = Vec::new();
            if trailing {
                what.push("trailing whitespace");
            }
            if space_before_tab {
                what.push("space before tab in indent");
            }
            out.write_all(name)?;
            writeln!(out, ":{}: {}.", at + 1, what.join(", "))?;
            out.write_all(b"+")?;
            out.write_all(text)?;
            out.write_all(b"\n")?;
        }
    }
    let (before, after) = (trailing_blank(&diff.old), trailing_blank(&diff.new));
    if after > before
        && diff
            .changes
            .last()
            .is_some_and(|(_, added)| added.end == diff.new.len() && !added.is_empty())
    {
        found = true;
        out.write_all(name)?;
        writeln!(
            out,
            ":{}: new blank line at EOF.",
            diff.new.len() - after + 1
        )?;
    }
    Ok(found)
}
