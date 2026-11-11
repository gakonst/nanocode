//! Source files, spans and diagnostics.

use std::fmt;

/// A byte range in one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: u32, start: usize, end: usize) -> Span {
        Span {
            file,
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn to(self, other: Span) -> Span {
        Span {
            file: self.file,
            start: self.start,
            end: other.end.max(self.end),
        }
    }
}

pub struct SourceFile {
    /// Path as shown in messages (relative to the workspace, e.g. `builtins/array.ts`).
    pub name: String,
    pub text: String,
}

impl SourceFile {
    /// 1-based line and column (in characters) of a byte offset.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = (offset as usize).min(self.text.len());
        let before = &self.text[..offset];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        (line, before[line_start..].chars().count() + 1)
    }

    fn line_text(&self, line: usize) -> &str {
        self.text.lines().nth(line - 1).unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            span,
            message: message.into(),
        }
    }
}

/// All source files of one run, indexed by `Span::file`.
#[derive(Default)]
pub struct Sources {
    pub files: Vec<SourceFile>,
}

impl Sources {
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> u32 {
        self.files.push(SourceFile {
            name: name.into(),
            text: text.into(),
        });
        (self.files.len() - 1) as u32
    }

    pub fn file(&self, span: Span) -> &SourceFile {
        &self.files[span.file as usize]
    }

    /// `file:line:col`
    pub fn position(&self, span: Span) -> String {
        let f = self.file(span);
        let (line, col) = f.line_col(span.start);
        format!("{}:{line}:{col}", f.name)
    }

    /// `file:line:col: error: message`, then the source line with a caret marker.
    pub fn render(&self, d: &Diagnostic) -> String {
        let f = self.file(d.span);
        let (line, col) = f.line_col(d.span.start);
        let text = f.line_text(line);
        let (end_line, end_col) = f.line_col(d.span.end);
        let width = if end_line == line && end_col > col {
            end_col - col
        } else {
            1
        };
        format!(
            "{}:{line}:{col}: error: {}\n  {text}\n  {}{}\n",
            f.name,
            d.message,
            " ".repeat(col - 1),
            "^".repeat(width)
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render() {
        let mut s = Sources::default();
        let id = s.add("a.ts", "line one\n  let x = é1;\n");
        let start = s.files[0].text.find('é').unwrap();
        let d = Diagnostic::new(Span::new(id, start, start + 3), "bad");
        assert_eq!(s.position(d.span), "a.ts:2:11");
        assert_eq!(
            s.render(&d),
            "a.ts:2:11: error: bad\n    let x = é1;\n            ^^\n"
        );
    }
}
