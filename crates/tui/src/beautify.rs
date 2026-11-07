#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Word,
    Punct,
    Literal,
    Comment,
    Newline,
}

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
    spaced: bool,
}

const REGEX_KEYWORDS: [&str; 14] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "await",
    "yield",
];

const BLOCK_CONTINUATIONS: [&str; 4] = ["else", "catch", "finally", "while"];

pub fn beautify(source: &str, indent: &str) -> String {
    let _span = tracing::info_span!("tui.code.beautify", text.bytes = source.len()).entered();
    let tokens = tokenize(source);
    let lines = split_lines(&tokens);
    render(&lines, indent)
}

fn tokenize(source: &str) -> Vec<Token<'_>> {
    scan(source, 0, false).0
}

fn scan(source: &str, start: usize, nested: bool) -> (Vec<Token<'_>>, usize) {
    let bytes = source.as_bytes();
    let mut tokens: Vec<Token<'_>> = Vec::new();
    let mut index = start;
    let mut spaced = false;
    let mut depth = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        let start = index;
        let kind = match byte {
            b'\n' => {
                index += 1;
                Kind::Newline
            }
            b' ' | b'\t' | b'\r' | b'\x0c' => {
                index += 1;
                spaced = true;
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = source[index..]
                    .find('\n')
                    .map_or(bytes.len(), |end| index + end);
                Kind::Comment
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = source[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| index + 2 + end + 2);
                Kind::Comment
            }
            b'/' if regex_allowed(&tokens) => {
                index = scan_regex(bytes, index);
                Kind::Literal
            }
            b'"' | b'\'' => {
                index = scan_string(bytes, index);
                Kind::Literal
            }
            b'`' => {
                index = scan_template(source, index);
                Kind::Literal
            }
            byte if is_word(byte) => {
                while index < bytes.len() && is_word(bytes[index]) {
                    index += 1;
                }
                Kind::Word
            }
            b'{' if nested => {
                depth += 1;
                index += 1;
                Kind::Punct
            }
            b'}' if nested => {
                index += 1;
                if depth == 0 {
                    return (tokens, index);
                }
                depth -= 1;
                Kind::Punct
            }
            _ => {
                index += source[index..].chars().next().map_or(1, char::len_utf8);
                Kind::Punct
            }
        };
        tokens.push(Token {
            kind,
            text: &source[start..index],
            spaced,
        });
        spaced = false;
    }
    (tokens, bytes.len())
}

const fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn regex_allowed(tokens: &[Token<'_>]) -> bool {
    let previous = tokens
        .iter()
        .rev()
        .find(|token| !matches!(token.kind, Kind::Newline | Kind::Comment));
    match previous {
        None => true,
        Some(token) => match token.kind {
            Kind::Punct if token.text == ")" => closes_control_header(tokens),
            Kind::Punct => token.text != "]",
            Kind::Word => REGEX_KEYWORDS.contains(&token.text),
            _ => false,
        },
    }
}

fn closes_control_header(tokens: &[Token<'_>]) -> bool {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().rev() {
        if token.kind != Kind::Punct {
            continue;
        }
        match token.text {
            ")" => depth += 1,
            "(" => {
                depth -= 1;
                if depth == 0 {
                    return tokens[..index]
                        .iter()
                        .rev()
                        .find(|token| !matches!(token.kind, Kind::Newline | Kind::Comment))
                        .is_some_and(|token| {
                            token.kind == Kind::Word
                                && matches!(token.text, "if" | "while" | "for" | "with")
                        });
                }
            }
            _ => {}
        }
    }
    false
}

const fn scan_regex(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    let mut class = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                    index += 1;
                }
                return index;
            }
            b'\n' => return index,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

fn scan_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while let Some(offset) = memchr::memchr3(b'\\', b'\n', quote, &bytes[index..]) {
        index += offset;
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'\n' => return index,
            _ => return index + 1,
        }
    }
    bytes.len()
}

fn scan_template(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    while let Some(offset) = memchr::memchr3(b'\\', b'`', b'$', &bytes[index..]) {
        index += offset;
        match bytes[index] {
            b'\\' => index = (index + 1).min(bytes.len() - 1),
            b'`' => return index + 1,
            b'$' if bytes.get(index + 1) == Some(&b'{') => {
                index = scan(source, index + 2, true).1;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

fn is_open(token: &Token<'_>) -> bool {
    token.kind == Kind::Punct && matches!(token.text, "(" | "[" | "{")
}

fn is_close(token: &Token<'_>) -> bool {
    token.kind == Kind::Punct && matches!(token.text, ")" | "]" | "}")
}

fn blocks(tokens: &[Token<'_>]) -> (Vec<bool>, Vec<Option<usize>>) {
    let mut block = vec![false; tokens.len()];
    let mut closes = vec![None; tokens.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if is_open(token) {
            stack.push(index);
        } else if is_close(token) {
            if let Some(open) = stack.pop() {
                closes[index] = Some(open);
            }
        } else if token.kind == Kind::Punct
            && token.text == ";"
            && let Some(&open) = stack.last()
            && tokens[open].text == "{"
        {
            block[open] = true;
        }
    }
    (block, closes)
}

fn split_lines<'a>(tokens: &[Token<'a>]) -> Vec<Vec<Token<'a>>> {
    let (block, closes) = blocks(tokens);
    let mut stack: Vec<&str> = Vec::new();
    let mut lines = vec![Vec::new()];
    let mut break_after = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == Kind::Newline {
            let open_empty = lines.last().is_some_and(Vec::is_empty);
            let previous_empty = lines.len() < 2 || lines[lines.len() - 2].is_empty();
            if !open_empty || !previous_empty {
                lines.push(Vec::new());
            }
            break_after = false;
            continue;
        }
        let line_empty = lines.last().is_some_and(Vec::is_empty);
        let closes_block = is_close(token) && closes[index].is_some_and(|open| block[open]);
        let continues = token.kind == Kind::Comment && !token.text.starts_with("/*")
            || is_close(token) && !closes_block
            || token.kind == Kind::Punct && matches!(token.text, ";" | "," | ")")
            || after_block_close(index, &closes, &block)
                && (token.kind == Kind::Punct && matches!(token.text, "." | ")" | "]" | ",")
                    || token.kind == Kind::Word && BLOCK_CONTINUATIONS.contains(&token.text));
        if !line_empty && (closes_block || break_after && !continues) {
            lines.push(Vec::new());
        }
        if let Some(line) = lines.last_mut() {
            line.push(*token);
        }
        break_after = false;
        if is_open(token) {
            stack.push(token.text);
            if token.text == "{" && block[index] {
                break_after = true;
            }
        } else if is_close(token) {
            stack.pop();
            if closes_block {
                break_after = true;
            }
        } else if token.kind == Kind::Punct
            && token.text == ";"
            && stack.last().is_none_or(|open| *open == "{")
        {
            break_after = true;
        }
    }
    while lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

fn after_block_close(index: usize, closes: &[Option<usize>], block: &[bool]) -> bool {
    index
        .checked_sub(1)
        .is_some_and(|previous| closes[previous].is_some_and(|open| block[open]))
}

fn render(lines: &[Vec<Token<'_>>], indent: &str) -> String {
    let mut output = String::new();
    let mut stack: Vec<usize> = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if number > 0 {
            output.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        let leading = line.iter().take_while(|token| is_close(token)).count();
        stack.truncate(stack.len().saturating_sub(leading));
        let mut depth = distinct(&stack);
        if leading == 0 && continuation(line) {
            depth += 1;
        }
        for _ in 0..depth {
            output.push_str(indent);
        }
        for (position, token) in line.iter().enumerate() {
            if position > 0 && token.spaced {
                output.push(' ');
            }
            output.push_str(token.text);
            if position >= leading {
                if is_open(token) {
                    stack.push(number);
                } else if is_close(token) {
                    stack.pop();
                }
            }
        }
    }
    output
}

fn continuation(line: &[Token<'_>]) -> bool {
    let Some(first) = line.first().filter(|token| token.kind == Kind::Punct) else {
        return false;
    };
    let second = line
        .get(1)
        .filter(|token| !token.spaced && token.kind == Kind::Punct)
        .map(|token| token.text);
    matches!(
        (first.text, second),
        (".", None | Some("#")) | ("?" | ":", _) | ("&", Some("&")) | ("|", Some("|"))
    )
}

fn distinct(stack: &[usize]) -> usize {
    let mut count = 0;
    let mut previous = None;
    for line in stack {
        if previous != Some(*line) {
            count += 1;
            previous = Some(*line);
        }
    }
    count
}
