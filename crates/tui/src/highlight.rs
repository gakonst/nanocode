mod grammar;
mod highlighter;
mod parser;
mod regex;
mod syntaxes;

use crate::ui::{
    style::{Color, Style},
    text::Span,
};
use crate::unicode;
pub use grammar::{Syntax, SyntaxSet};
use highlighter::FontStyle;
pub use highlighter::{HighlightLines, HighlightState, Theme};
pub use parser::{ParseError, ParseState};
use std::{path::Path, sync::OnceLock};

pub struct Assets {
    pub syntaxes: SyntaxSet,
}

pub fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| Assets {
        syntaxes: SyntaxSet::load(),
    })
}

pub fn prewarm() {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("tui-highlighting".to_owned())
            .spawn(|| {
                let _ = assets();
                let _ = theme();
            });
    });
}

pub fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        Theme::new(
            Color::Reset,
            &[
                (&["comment"], Color::DarkGray, Some(FontStyle::ITALIC)),
                (&["string"], Color::Green, None),
                (
                    &[
                        "constant.numeric",
                        "constant.language",
                        "constant.character",
                    ],
                    Color::Magenta,
                    None,
                ),
                (&["keyword", "storage"], Color::Blue, Some(FontStyle::BOLD)),
                (
                    &["entity.name.function", "support.function"],
                    Color::Cyan,
                    None,
                ),
                (
                    &[
                        "entity.name.type",
                        "entity.name.class",
                        "entity.name.struct",
                        "entity.name.enum",
                        "entity.name.trait",
                        "support.type",
                    ],
                    Color::Yellow,
                    None,
                ),
                (
                    &["variable.parameter"],
                    Color::Reset,
                    Some(FontStyle::ITALIC),
                ),
                (&["invalid"], Color::Red, Some(FontStyle::UNDERLINE)),
            ],
        )
    })
}

pub fn syntax_for_token(syntaxes: &SyntaxSet, token: &str) -> &'static Syntax {
    let token = token.split_ascii_whitespace().next().unwrap_or_default();
    highlightable(
        syntaxes,
        syntaxes
            .find_syntax_by_token(token)
            .or_else(|| syntaxes.find_syntax_by_extension(token))
            .or_else(|| syntaxes.find_syntax_by_name(token)),
    )
}

pub fn syntax_for_path(syntaxes: &SyntaxSet, path: &str) -> &'static Syntax {
    let path = Path::new(path);
    let syntax = path
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| syntaxes.find_syntax_by_extension(extension))
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| syntaxes.find_syntax_by_extension(name))
        });
    highlightable(syntaxes, syntax)
}

fn highlightable(syntaxes: &SyntaxSet, syntax: Option<&'static Syntax>) -> &'static Syntax {
    syntax
        .filter(|syntax| !matches!(syntax.name, "Markdown" | "MultiMarkdown"))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

pub fn line(
    highlighter: &mut HighlightLines<'_>,
    text: &str,
    syntaxes: &SyntaxSet,
) -> Vec<Span<'static>> {
    let mut start = 0;
    runs(highlighter, text, syntaxes)
        .into_iter()
        .map(|(style, end)| {
            let span = Span::styled(text[start..end].to_owned(), style);
            start = end;
            span
        })
        .collect()
}

pub(crate) fn runs(
    highlighter: &mut HighlightLines<'_>,
    text: &str,
    syntaxes: &SyntaxSet,
) -> Vec<(Style, usize)> {
    let line = format!("{text}\n");
    let Ok(regions) = highlighter.highlight_line(&line, syntaxes) else {
        return vec![(code_style(), text.len())];
    };
    let regions = regions
        .into_iter()
        .map(|(style, region)| (style, region.trim_end_matches('\n')))
        .filter(|(_, region)| !region.is_empty())
        .collect::<Vec<_>>();
    if regions.is_empty() {
        return vec![(code_style(), 0)];
    }
    let joined = |left: &(Style, &str), right: &(Style, &str)| {
        left.0 == right.0 && printable(left.1) && printable(right.1)
    };
    let mut end = 0;
    let mut runs = Vec::with_capacity(regions.chunk_by(joined).count());
    for run in regions.chunk_by(joined) {
        end += run.iter().map(|(_, region)| region.len()).sum::<usize>();
        runs.push((run[0].0, end));
    }
    runs
}

fn printable(text: &str) -> bool {
    crate::text::ascii_prefix(text, false) == text.len()
}

pub fn code_style() -> Style {
    Style::default().fg(Color::Reset)
}

pub fn wrap(spans: Vec<Span<'static>>, width: u16) -> Vec<Vec<Span<'static>>> {
    wrap_spans(&spans, width)
}

pub(crate) fn wrap_spans(spans: &[Span<'_>], width: u16) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::<Span<'static>>::new()];
    let mut used = 0_u16;
    for span in spans {
        if width > 0 && crate::text::ascii_prefix(&span.content, false) == span.content.len() {
            let mut remaining = span.content.as_ref();
            while !remaining.is_empty() {
                if used == width {
                    lines.push(Vec::new());
                    used = 0;
                }
                let length = remaining.len().min(usize::from(width - used));
                push_grapheme(
                    lines.last_mut().expect("a line always exists"),
                    &remaining[..length],
                    span.style,
                );
                used += length as u16;
                remaining = &remaining[length..];
            }
            continue;
        }
        for grapheme in unicode::graphemes(&span.content) {
            let grapheme_width = u16::try_from(unicode::width(grapheme)).unwrap_or(u16::MAX);
            if grapheme_width > width {
                if used > 0 {
                    lines.push(Vec::new());
                }
                push_grapheme(
                    lines.last_mut().expect("a line always exists"),
                    "�",
                    span.style,
                );
                used = 1;
                continue;
            }
            if used.saturating_add(grapheme_width) > width && used > 0 {
                lines.push(Vec::new());
                used = 0;
            }
            push_grapheme(
                lines.last_mut().expect("a line always exists"),
                grapheme,
                span.style,
            );
            used = used.saturating_add(grapheme_width);
        }
    }
    lines
}

fn push_grapheme(spans: &mut Vec<Span<'static>>, grapheme: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(grapheme);
        return;
    }
    spans.push(Span::styled(grapheme.to_owned(), style));
}
