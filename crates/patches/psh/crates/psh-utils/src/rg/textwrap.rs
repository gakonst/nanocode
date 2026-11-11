// Adapted from textwrap 0.16.2 (https://github.com/mgeisler/textwrap): `refill`,
// `unfill`, `fill`, `wrap_single_line`, `wrap_first_fit`, `break_words` and `indent`,
// with the options ripgrep's help uses (first fit, no hyphenation, broken words):
// Copyright (c) 2016 Martin Geisler. Licensed under the MIT License (see NOTICE).

//! The two textwrap functions ripgrep's `--help` uses, so that its text is the same
//! (D1133). No ANSI escapes occur in the help, so widths are textwrap's without them.

/// textwrap's width of a character without the `unicode-width` feature.
fn ch_width(c: char) -> usize {
    if c < '\u{1100}' { 1 } else { 2 }
}

fn width(s: &str) -> usize {
    s.chars().map(ch_width).sum()
}

/// A word with its trailing spaces.
#[derive(Clone, Copy)]
struct Word<'a> {
    word: &'a str,
    width: usize,
    whitespace: &'a str,
}

impl<'a> Word<'a> {
    fn from(s: &'a str) -> Word<'a> {
        let word = s.trim_end_matches(' ');
        Word { word, width: width(word), whitespace: &s[word.len()..] }
    }
}

/// textwrap's `AsciiSpace` word separator.
fn find_words(line: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_whitespace = false;
    for (idx, ch) in line.char_indices() {
        if in_whitespace && ch != ' ' {
            out.push(Word::from(&line[start..idx]));
            start = idx;
        }
        in_whitespace = ch == ' ';
    }
    if start < line.len() {
        out.push(Word::from(&line[start..]));
    }
    out
}

/// Words wider than `line_width` broken into pieces that fit.
fn break_words(words: Vec<Word<'_>>, line_width: usize) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    for w in words {
        if w.width <= line_width {
            out.push(w);
            continue;
        }
        let (mut offset, mut wd) = (0, 0);
        for (idx, ch) in w.word.char_indices() {
            if wd > 0 && wd + ch_width(ch) > line_width {
                out.push(Word {
                    word: &w.word[offset..idx],
                    width: wd,
                    whitespace: "",
                });
                offset = idx;
                wd = ch_width(ch);
            } else {
                wd += ch_width(ch);
            }
        }
        if offset < w.word.len() {
            out.push(Word {
                word: &w.word[offset..],
                width: wd,
                whitespace: w.whitespace,
            });
        }
    }
    out
}

fn wrap_first_fit<'w, 'a>(
    words: &'w [Word<'a>],
    line_widths: [usize; 2],
) -> Vec<&'w [Word<'a>]> {
    let mut lines = Vec::new();
    let (mut start, mut w) = (0, 0usize);
    for (idx, word) in words.iter().enumerate() {
        let line_width = line_widths[lines.len().min(1)];
        if w + word.width > line_width && idx > start {
            lines.push(&words[start..idx]);
            start = idx;
            w = 0;
        }
        w += word.width + word.whitespace.len();
    }
    lines.push(&words[start..]);
    lines
}

fn wrap_single_line(
    line: &str,
    width: usize,
    initial: &str,
    subsequent: &str,
    lines: &mut Vec<String>,
) {
    let indent = if lines.is_empty() { initial } else { subsequent };
    if line.len() < width && indent.is_empty() {
        lines.push(line.trim_end_matches(' ').to_string());
        return;
    }
    let initial_width = width.saturating_sub(self::width(initial));
    let subsequent_width = width.saturating_sub(self::width(subsequent));
    let mut words = break_words(find_words(line), subsequent_width);
    if !initial.is_empty() {
        words.insert(0, Word::from(""));
    }
    let mut idx = 0;
    for ws in wrap_first_fit(&words, [initial_width, subsequent_width]) {
        let Some(last) = ws.last() else {
            lines.push(String::new());
            continue;
        };
        let len = ws
            .iter()
            .map(|w| w.word.len() + w.whitespace.len())
            .sum::<usize>()
            - last.whitespace.len();
        let mut result = if lines.is_empty() {
            initial.to_string()
        } else {
            subsequent.to_string()
        };
        result.push_str(&line[idx..idx + len]);
        lines.push(result);
        idx += len + last.whitespace.len();
    }
}

fn fill(text: &str, width: usize, initial: &str, subsequent: &str) -> String {
    if text.len() < width && !text.contains('\n') && initial.is_empty() {
        return text.trim_end_matches(' ').to_string();
    }
    let mut lines = Vec::new();
    for line in text.split('\n') {
        wrap_single_line(line, width, initial, subsequent, &mut lines);
    }
    lines.join("\n")
}

/// textwrap's `refill(text, width)`: the indentation of the text kept, its lines
/// joined and filled again. ripgrep passes one line (newlines already replaced).
pub fn refill(text: &str, width: usize) -> String {
    const PREFIX: &[char] = &[' ', '-', '+', '*', '>', '#', '/'];
    let mut initial = "";
    let mut subsequent = "";
    for (idx, line) in text.lines().enumerate() {
        let rest = line.trim_start_matches(PREFIX);
        let prefix = &line[..line.len() - rest.len()];
        if idx == 0 {
            initial = prefix;
        } else if idx == 1 {
            subsequent = prefix;
        } else {
            for ((i, x), y) in prefix.char_indices().zip(subsequent.chars()) {
                if x != y {
                    subsequent = &prefix[..i];
                    break;
                }
            }
            if prefix.len() < subsequent.len() {
                subsequent = prefix;
            }
        }
    }
    let mut unfilled = String::with_capacity(text.len());
    let mut ends_with_newline = false;
    let mut idx = 0;
    for line in text.split_inclusive('\n') {
        let (line, nl) = match line.strip_suffix('\n') {
            Some(l) => (l.strip_suffix('\r').unwrap_or(l), true),
            None => (line, false),
        };
        if line.is_empty() && nl {
            continue;
        }
        idx += 1;
        if idx == 1 {
            unfilled.push_str(&line[initial.len().min(line.len())..]);
        } else {
            unfilled.push(' ');
            unfilled.push_str(&line[subsequent.len().min(line.len())..]);
        }
        ends_with_newline = nl;
    }
    let mut out = fill(&unfilled, width, initial, subsequent);
    if ends_with_newline && text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// textwrap's `indent`: `prefix` before every line, trimmed on blank ones.
pub fn indent(s: &str, prefix: &str) -> String {
    let mut result = String::with_capacity(2 * s.len());
    let trimmed = prefix.trim_end();
    for (idx, line) in s.split_terminator('\n').enumerate() {
        if idx > 0 {
            result.push('\n');
        }
        if line.trim().is_empty() {
            result.push_str(trimmed);
        } else {
            result.push_str(prefix);
        }
        result.push_str(line);
    }
    if s.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_first_fit() {
        assert_eq!(refill("aaa bbb ccc ddd", 8), "aaa bbb\nccc ddd");
        assert_eq!(refill("short", 71), "short");
        assert_eq!(
            refill("--flag and more words", 10),
            "--flag and\nmore words"
        );
        assert_eq!(refill("abcdefghijkl x", 5), "abcde\nfghij\nkl x");
        assert_eq!(indent("a\n\nb\n", "  "), "  a\n\n  b\n");
    }
}
