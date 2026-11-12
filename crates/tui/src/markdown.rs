use crate::commonmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use crate::ui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use crate::{format::sanitize_terminal_text, theme::Theme};
use std::{borrow::Cow, ops::Range, sync::Arc};
use syntect::{easy::HighlightLines, highlighting::HighlightState, parsing::ParseState};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct Layout {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<Vec<LinkSpan>>,
    pub sources: Vec<Option<Range<usize>>>,
}

#[derive(Clone)]
pub struct LinkSpan {
    pub destination: Arc<str>,
    pub start: u16,
    pub end: u16,
}

#[derive(Clone, Default)]
pub(crate) struct Highlights {
    blocks: Vec<HighlightedBlock>,
}

impl Highlights {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.blocks.shrink_to_fit();
        for block in &mut self.blocks {
            block.code.shrink_to_fit();
            block.runs.shrink_to_fit();
            block.lines.shrink_to_fit();
        }
    }
}

#[derive(Clone)]
struct HighlightedBlock {
    language: Option<String>,
    code: String,
    runs: Vec<(Style, usize)>,
    lines: Vec<(usize, usize)>,
    resume: Option<Resume>,
}

impl HighlightedBlock {
    fn spans<'a>(&'a self, line: usize, spans: &mut Vec<Span<'a>>) {
        let first = line
            .checked_sub(1)
            .map_or(0, |previous| self.lines[previous].1);
        let (mut start, end) = self.lines[line];
        spans.clear();
        spans.extend(self.runs[first..end].iter().map(|&(style, run)| {
            let span = Span::styled(&self.code[start..run], style);
            start = run;
            span
        }));
    }
}

fn push_runs(
    runs: &mut Vec<(Style, usize)>,
    lines: &mut Vec<(usize, usize)>,
    start: usize,
    line: Vec<(Style, usize)>,
) {
    runs.extend(line.into_iter().map(|(style, end)| (style, start + end)));
    lines.push((start, runs.len()));
}

#[derive(Clone)]
struct Resume {
    offset: usize,
    lines: usize,
    highlight: HighlightState,
    parse: ParseState,
}

pub fn render(markdown: &str, width: u16, theme: &Theme) -> Layout {
    render_with(markdown, width, theme, &mut Highlights::default())
}

pub(crate) fn render_with(
    markdown: &str,
    width: u16,
    theme: &Theme,
    highlights: &mut Highlights,
) -> Layout {
    render_from(
        markdown,
        0,
        0,
        Continuation::Block,
        width,
        theme,
        highlights,
    )
    .layout
}

#[derive(Clone, Copy)]
pub(crate) struct Checkpoint {
    pub(crate) bytes: usize,
    pub(crate) lines: usize,
    pub(crate) blocks: usize,
    pub(crate) continuation: Continuation,
    pub(crate) group: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Default)]
pub(crate) enum Continuation {
    #[default]
    Block,
    Item {
        next: Option<u64>,
        loose: bool,
    },
    Text,
}

pub(crate) struct Rendered {
    pub(crate) layout: Layout,
    pub(crate) boundary: Option<Checkpoint>,
    pub(crate) block: Option<Checkpoint>,
    pub(crate) resumed: bool,
    pub(crate) group: Option<(usize, usize)>,
}

enum Trailing {
    Text { clean: bool, quoted: bool },
    List(Items),
}

#[derive(Default)]
struct Items {
    starts: Vec<(Option<Checkpoint>, Option<u64>, bool)>,
    loose: Option<bool>,
}

pub(crate) fn render_from(
    markdown: &str,
    start: usize,
    blocks: usize,
    continuation: Continuation,
    width: u16,
    theme: &Theme,
    highlights: &mut Highlights,
) -> Rendered {
    let text = &markdown[start..];
    let span = tracing::info_span!(
        "tui.markdown.render",
        width,
        text.bytes = text.len(),
        text.start = start,
        code.blocks = tracing::field::Empty,
        code.highlighted = tracing::field::Empty
    );
    let _entered = span.enter();
    let skipped = |resumed| Rendered {
        layout: Layout {
            lines: Vec::new(),
            links: Vec::new(),
            sources: Vec::new(),
        },
        boundary: None,
        block: None,
        resumed,
        group: None,
    };
    if width == 0 {
        return skipped(true);
    }
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_GFM;
    let mut renderer = Renderer::new(text, width, theme, highlights);
    renderer.block = blocks;
    renderer.continued = start > 0;
    let mut depth = 0_usize;
    let mut boundary = None;
    let mut trailing = None;
    let mut opened = 0_usize;
    let mut observed = None;
    let mut events = Parser::new_ext(text, options).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let (lines, blocks) = (renderer.lines.len(), renderer.block);
        let at = range.start;
        let checkpoint = move || {
            let line = text[..at].rfind('\n').map_or(0, |index| index + 1);
            let complete = memchr::memchr(b'\n', &text.as_bytes()[at..]).is_some();
            let checkpoint = Checkpoint {
                bytes: start + line,
                lines,
                blocks,
                continuation: Continuation::Block,
                group: None,
            };
            (checkpoint, complete, line)
        };
        let event = if depth == 0 && matches!(event, Event::Start(_) | Event::Rule) {
            let (checkpoint, complete, _) = checkpoint();
            if complete && (opened > 0 || matches!(continuation, Continuation::Block)) {
                boundary = Some(checkpoint);
            }
            if let Some(Trailing::List(items)) = trailing.take()
                && opened == 1
            {
                observed = items.loose;
            }
            opened += 1;
            renderer.cut = None;
            trailing = match event {
                Event::Start(Tag::Paragraph) => Some(Trailing::Text {
                    clean: true,
                    quoted: false,
                }),
                Event::Start(Tag::List(_)) => Some(Trailing::List(Items::default())),
                _ => None,
            };
            match (opened, continuation, event) {
                (1, Continuation::Item { next, .. }, Event::Start(Tag::List(_))) => {
                    Event::Start(Tag::List(next))
                }
                (1, Continuation::Text, event @ Event::Start(Tag::Paragraph)) => event,
                (1, Continuation::Item { .. } | Continuation::Text, _) => return skipped(false),
                (_, _, event) => event,
            }
        } else {
            event
        };
        match (&mut trailing, &event) {
            (Some(Trailing::Text { clean, quoted }), Event::Text(content)) => {
                let source = &text[range.clone()];
                let unsettled = unsettled(text, range.clone());
                renderer.anchor = *clean
                    && !*quoted
                    && !unsettled
                    && depth == 1
                    && content.as_ref() == source
                    && matches!(sanitize_terminal_text(source), Cow::Borrowed(_));
                if source == "\"" {
                    *quoted = content.as_ref() == "\u{201c}";
                } else if unsettled {
                    *clean = false;
                }
            }
            (Some(Trailing::List(items)), Event::Start(Tag::Item)) if depth == 1 => {
                let (checkpoint, complete, line) = checkpoint();
                let next = renderer.lists.last().and_then(|list| list.next);
                let gap = line > 0
                    && text[..line - 1]
                        .rsplit('\n')
                        .next()
                        .is_some_and(|previous| previous.trim().is_empty());
                items
                    .starts
                    .push((complete.then_some(checkpoint), next, gap));
            }
            (Some(Trailing::List(items)), Event::Start(Tag::Paragraph)) if depth == 2 => {
                items.loose = Some(true);
            }
            (
                Some(Trailing::List(items)),
                Event::Text(_)
                | Event::Code(_)
                | Event::InlineHtml(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Start(
                    Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. },
                ),
            ) if depth == 2 => {
                items.loose.get_or_insert(false);
            }
            _ => {}
        }
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                renderer.flush();
                renderer.code_block(kind, range, &mut events);
            }
            Event::Start(Tag::Table(_)) => {
                renderer.flush();
                renderer.table(range, &mut events);
            }
            event => {
                match event {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => depth -= 1,
                    _ => {}
                }
                renderer.range = range;
                renderer.event(event);
                renderer.anchor = false;
            }
        }
    }
    if let Some(Trailing::List(items)) = &trailing
        && opened == 1
    {
        observed = items.loose;
    }
    let resumed = match continuation {
        Continuation::Block => true,
        Continuation::Item { loose, .. } => {
            opened > 0 && observed.is_none_or(|observed| observed == loose)
        }
        Continuation::Text => opened > 0,
    };
    if !resumed {
        return skipped(false);
    }
    span.record("code.blocks", renderer.block);
    span.record("code.highlighted", renderer.highlighted);
    let inner = match &trailing {
        Some(Trailing::Text { .. }) => renderer.cut.map(|cut| Checkpoint {
            bytes: start + cut.bytes,
            lines: cut.line,
            blocks: renderer.block,
            continuation: Continuation::Text,
            group: Some((cut.group, start + cut.source)),
        }),
        Some(Trailing::List(items)) => items.loose.and_then(|loose| {
            (1..items.starts.len()).rev().find_map(|index| {
                let (checkpoint, next, _) = items.starts[index];
                let settled = !loose || items.starts.get(index + 1).is_some_and(|(_, _, gap)| *gap);
                checkpoint.filter(|_| settled).map(|checkpoint| Checkpoint {
                    continuation: Continuation::Item { next, loose },
                    ..checkpoint
                })
            })
        }),
        None => None,
    };
    let group = renderer.group.map(|(rows, end)| (rows, start + end));
    let mut layout = renderer.finish();
    for range in layout.sources.iter_mut().flatten() {
        range.start += start;
        range.end += start;
    }
    let block = boundary;
    let boundary = [inner, boundary]
        .into_iter()
        .flatten()
        .find(|boundary| boundary.lines > 0 && boundary.lines < layout.lines.len());
    Rendered {
        layout,
        boundary,
        block,
        resumed,
        group: matches!(continuation, Continuation::Text)
            .then_some(group)
            .flatten(),
    }
}

fn unsettled(text: &str, range: Range<usize>) -> bool {
    text[range.clone()]
        .char_indices()
        .any(|(index, character)| match character {
            '*' | '~' | '[' | ']' | '`' | '<' | '|' | '"' => true,
            '_' | '\'' => !text[..range.start + index]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric),
            _ => false,
        })
}

pub(crate) fn self_contained(markdown: &str) -> bool {
    memchr::memmem::find(markdown.as_bytes(), b"[^").is_none()
        && !markdown.split_inclusive('\n').any(|line| {
            let label = line.trim_start_matches(|character: char| {
                matches!(character, ' ' | '\t' | '>' | '-' | '*' | '+' | '.' | ')')
                    || character.is_ascii_digit()
            });
            label.starts_with('[')
                && (label.contains("]:") || line.ends_with('\n') && !label.contains(']'))
        })
}

pub fn wrap_plain(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    wrap_plain_with_whitespace(text, width, style, false)
}

pub fn wrap_plain_preserving_whitespace(
    text: &str,
    width: u16,
    style: Style,
) -> Vec<Line<'static>> {
    wrap_plain_with_whitespace(text, width, style, true)
}

fn wrap_plain_with_whitespace(
    text: &str,
    width: u16,
    style: Style,
    preserve_whitespace: bool,
) -> Vec<Line<'static>> {
    wrap_plain_rows(text, width, style, preserve_whitespace, |line, _| line)
}

pub(crate) fn wrap_plain_rows<T>(
    text: &str,
    width: u16,
    style: Style,
    preserve_whitespace: bool,
    mut row: impl FnMut(Line<'static>, usize) -> T,
) -> Vec<T> {
    if width == 0 {
        return vec![row(Line::default(), 0)];
    }
    let logical = sanitize_terminal_text(text);
    if let Some(rows) = wrap_ascii_spans(
        std::iter::once((logical.as_ref(), style, None)),
        width,
        true,
        preserve_whitespace,
    ) {
        if rows.is_empty() {
            return vec![row(Line::default(), 0)];
        }
        return rows
            .into_iter()
            .map(|(line, _, index, _)| row(line, index))
            .collect();
    }
    let mut lines = Vec::new();
    for (index, line) in logical.split('\n').enumerate() {
        let spans = vec![Span::styled(line, style)];
        lines.extend(
            wrap_spans_with_whitespace(&spans, width, true, preserve_whitespace)
                .into_iter()
                .map(|wrapped| row(wrapped, index)),
        );
    }
    if lines.is_empty() {
        lines.push(row(Line::default(), 0));
    }
    lines
}

pub fn sanitize(text: &str) -> String {
    sanitize_terminal_text(text).into_owned()
}

struct Renderer<'a> {
    markdown: &'a str,
    range: Range<usize>,
    width: u16,
    theme: &'a Theme,
    lines: Vec<Line<'static>>,
    sources: Vec<Option<Range<usize>>>,
    current: Vec<TaggedSpan>,
    styles: Vec<Style>,
    lists: Vec<ListState>,
    quote_depth: usize,
    links: Vec<LinkState>,
    rendered_links: Vec<(usize, LinkSpan)>,
    image: Option<LinkState>,
    highlights: &'a mut Highlights,
    block: usize,
    highlighted: usize,
    continued: bool,
    anchor: bool,
    cut: Option<Cut>,
    group: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct Cut {
    line: usize,
    bytes: usize,
    group: usize,
    source: usize,
}

struct ListState {
    next: Option<u64>,
}

struct LinkState {
    destination: Arc<str>,
    label: String,
}

struct TaggedSpan {
    span: Span<'static>,
    link: Option<Arc<str>>,
    source: Option<Range<usize>>,
    anchor: bool,
}

impl<'a> Renderer<'a> {
    fn new(
        markdown: &'a str,
        width: u16,
        theme: &'a Theme,
        highlights: &'a mut Highlights,
    ) -> Self {
        Self {
            markdown,
            range: 0..0,
            width,
            theme,
            lines: Vec::new(),
            sources: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default().fg(theme.text())],
            lists: Vec::new(),
            quote_depth: 0,
            links: Vec::new(),
            rendered_links: Vec::new(),
            image: None,
            highlights,
            block: 0,
            highlighted: 0,
            continued: false,
            anchor: false,
            cut: None,
            group: None,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.span(
                &code,
                Style::default()
                    .fg(self.theme.code_text())
                    .bg(self.theme.code_background()),
            ),
            Event::Html(html) | Event::InlineHtml(html) => {
                self.span(&html, Style::default().fg(self.theme.muted()));
            }
            Event::FootnoteReference(reference) => self.span(
                &format!("[{}]", sanitize(&reference)),
                Style::default().fg(self.theme.thinking_medium()),
            ),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.push_line(
                    Line::from(Span::styled(
                        "─".repeat(usize::from(self.width)),
                        Style::default().fg(self.theme.muted()),
                    )),
                    Some(self.range.clone()),
                );
                self.blank();
            }
            Event::TaskListMarker(checked) => self.span(
                if checked { "✓ " } else { "□ " },
                Style::default().fg(if checked {
                    self.theme.thinking_medium()
                } else {
                    self.theme.muted()
                }),
            ),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.ensure_prefix(),
            Tag::Heading { level, .. } => {
                self.flush();
                let style = heading_style(level, self.theme);
                self.push_unlinked(Span::styled("▍ ", style));
                self.push_style(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_add(1);
                self.push_style(Style::default().fg(self.theme.muted()));
            }
            Tag::List(start) => self.lists.push(ListState { next: start }),
            Tag::Item => {
                self.flush();
                self.ensure_quote_prefix();
                let depth = self.lists.len().saturating_sub(1);
                self.push_unlinked(Span::raw("  ".repeat(depth)));
                let marker = self.lists.last_mut().map_or_else(
                    || "• ".to_owned(),
                    |list| match &mut list.next {
                        Some(next) => {
                            let marker = format!("{next}. ");
                            *next = next.saturating_add(1);
                            marker
                        }
                        None => "• ".to_owned(),
                    },
                );
                self.push_unlinked(Span::styled(
                    marker,
                    Style::default().fg(self.theme.accent()),
                ));
            }
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(
                Style::default()
                    .fg(self.theme.muted())
                    .add_modifier(Modifier::CROSSED_OUT),
            ),

            Tag::Link { dest_url, .. } => {
                self.links.push(LinkState {
                    destination: Arc::from(sanitize(&dest_url)),
                    label: String::new(),
                });
                self.push_style(
                    Style::default()
                        .fg(self.theme.accent())
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some(LinkState {
                    destination: Arc::from(sanitize(&dest_url)),
                    label: String::new(),
                });
            }
            Tag::FootnoteDefinition(label) => {
                self.flush();
                self.push_unlinked(Span::styled(
                    format!("[{}] ", sanitize(&label)),
                    Style::default().fg(self.theme.thinking_medium()),
                ));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.push_style(Style::default().fg(self.theme.muted()));
            }
            Tag::CodeBlock(_) | Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank();
            }
            TagEnd::Heading(_) => {
                self.pop_style();
                self.flush();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.pop_style();
                self.blank();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),

            TagEnd::Link => {
                self.pop_style();
                if let Some(link) = self.links.pop()
                    && link.label.trim() != link.destination.as_ref()
                {
                    self.push_linked(
                        Span::styled(
                            format!(" ↗ {}", link.destination),
                            Style::default()
                                .fg(self.theme.accent())
                                .add_modifier(Modifier::DIM),
                        ),
                        link.destination,
                    );
                }
            }
            TagEnd::Image => {
                if let Some(image) = self.image.take() {
                    self.render_image_link(image);
                }
            }
            TagEnd::FootnoteDefinition | TagEnd::HtmlBlock => {
                self.pop_style();
                self.flush();
                self.blank();
            }
            TagEnd::CodeBlock
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell => {}
        }
    }

    fn text(&mut self, text: &str) {
        let text = sanitize(text);
        if let Some(image) = &mut self.image {
            image.label.push_str(&text);
            return;
        }
        if let Some(link) = self.links.last_mut() {
            link.label.push_str(&text);
        }
        self.ensure_prefix();
        self.push_current(Span::styled(text, self.style()));
    }

    fn span(&mut self, text: &str, style: Style) {
        let text = sanitize(text);
        if let Some(image) = &mut self.image {
            image.label.push_str(&text);
            return;
        }
        self.ensure_prefix();
        self.push_current(Span::styled(text, self.style().patch(style)));
    }

    fn code_block<'e>(
        &mut self,
        kind: CodeBlockKind<'_>,
        range: Range<usize>,
        events: &mut impl Iterator<Item = (Event<'e>, Range<usize>)>,
    ) {
        let fenced = matches!(kind, CodeBlockKind::Fenced(_));
        let language = match kind {
            CodeBlockKind::Indented => None,
            CodeBlockKind::Fenced(language) if language.is_empty() => None,
            CodeBlockKind::Fenced(language) => Some(sanitize(&language)),
        };
        let mut code = String::new();
        for (event, _) in events.by_ref() {
            match event {
                Event::End(TagEnd::CodeBlock) => break,
                Event::Text(text) | Event::Code(text) => code.push_str(&sanitize(&text)),
                Event::SoftBreak | Event::HardBreak => code.push('\n'),
                _ => {}
            }
        }
        let is_diff = language.as_deref().is_some_and(is_diff_language);
        if is_diff {
            for line in super::diff::render(&code, self.width, self.theme) {
                self.push_line(line, Some(range.clone()));
            }
            self.blank();
            return;
        }
        let rows = source_lines(self.markdown, range);
        let content = code.split_terminator('\n').count();
        let first = usize::from(fenced);
        let line_source = |line: usize| {
            (line < content)
                .then(|| rows.get(first + line).cloned())
                .flatten()
        };
        let fence = fenced.then(|| rows.first().cloned()).flatten();
        let closing = fenced.then(|| rows.get(first + content).cloned()).flatten();
        let block = self.highlight(code, language.as_deref());
        if self.width < 6 {
            self.narrow_code_block(block, &line_source);
            self.blank();
            return;
        }

        let border = Style::default().fg(self.theme.border());
        self.push_line(
            code_block_header(language.as_deref(), self.width, self.theme),
            fence,
        );
        let content_width = self.width.saturating_sub(4).max(1);
        let highlighted = &self.highlights.blocks[block];
        let mut source = Vec::new();
        for line in 0..highlighted.lines.len() {
            highlighted.spans(line, &mut source);
            for (index, mut spans) in
                super::highlight::wrap_spans(&source, content_width.saturating_sub(2).max(1))
                    .into_iter()
                    .enumerate()
            {
                if index > 0 {
                    spans.insert(
                        0,
                        Span::styled("↪ ", Style::default().fg(self.theme.muted())),
                    );
                }
                let used = spans.iter().map(Span::width).sum::<usize>();
                let padding = usize::from(content_width).saturating_sub(used);
                let mut body = Vec::with_capacity(spans.len() + 3);
                body.push(Span::styled("│ ", border));
                body.extend(spans);
                body.push(Span::raw(" ".repeat(padding)));
                body.push(Span::styled(" │", border));
                self.lines.push(Line::from(body));
                self.sources.push(line_source(line));
            }
        }
        self.push_line(
            Line::from(Span::styled(
                format!(
                    "╰{}╯",
                    "─".repeat(usize::from(self.width.saturating_sub(2)))
                ),
                border,
            )),
            closing,
        );
        self.blank();
    }

    fn highlight(&mut self, code: String, language: Option<&str>) -> usize {
        let block = self.block;
        self.block += 1;
        let cached = self
            .highlights
            .blocks
            .get_mut(block)
            .filter(|cached| cached.language.as_deref() == language);
        if let Some(cached) = &cached
            && cached.code == code
        {
            return block;
        }
        self.highlighted += 1;
        let assets = super::highlight::assets();
        let theme = super::highlight::theme();
        let source = code.trim_end_matches('\n');
        let resumed = cached.and_then(|cached| {
            let resume = cached.resume.take()?;
            (source.len() > resume.offset
                && source.as_bytes()[..resume.offset] == cached.code.as_bytes()[..resume.offset])
                .then(|| {
                    let mut lines = std::mem::take(&mut cached.lines);
                    lines.truncate(resume.lines);
                    let mut runs = std::mem::take(&mut cached.runs);
                    runs.truncate(lines.last().map_or(0, |&(_, end)| end));
                    (
                        runs,
                        lines,
                        resume.offset,
                        HighlightLines::from_state(theme, resume.highlight, resume.parse),
                    )
                })
        });
        let (mut runs, mut lines, offset, mut highlighter) = resumed.unwrap_or_else(|| {
            let syntax = language.map_or_else(
                || assets.syntaxes.find_syntax_plain_text(),
                |language| super::highlight::syntax_for_token(&assets.syntaxes, language),
            );
            (
                Vec::new(),
                Vec::new(),
                0,
                HighlightLines::new(syntax, theme),
            )
        });
        let (complete, last) = source[offset..]
            .rsplit_once('\n')
            .unwrap_or(("", &source[offset..]));
        if offset < source.len() - last.len() {
            let mut start = offset;
            for source_line in complete.split('\n') {
                let line = super::highlight::runs(&mut highlighter, source_line, &assets.syntaxes);
                push_runs(&mut runs, &mut lines, start, line);
                start += source_line.len() + 1;
            }
        }
        let (highlight, parse) = highlighter.state();
        let resume = Resume {
            offset: source.len() - last.len(),
            lines: lines.len(),
            highlight: highlight.clone(),
            parse: parse.clone(),
        };
        let mut highlighter = HighlightLines::from_state(theme, highlight, parse);
        let line = super::highlight::runs(&mut highlighter, last, &assets.syntaxes);
        push_runs(&mut runs, &mut lines, source.len() - last.len(), line);
        let highlighted = HighlightedBlock {
            language: language.map(str::to_owned),
            code,
            runs,
            lines,
            resume: Some(resume),
        };
        if let Some(slot) = self.highlights.blocks.get_mut(block) {
            *slot = highlighted;
        } else {
            self.highlights.blocks.push(highlighted);
        }
        block
    }

    fn narrow_code_block(
        &mut self,
        block: usize,
        line_source: &impl Fn(usize) -> Option<Range<usize>>,
    ) {
        let gutter = Style::default().fg(self.theme.border());
        let content_width = self.width.saturating_sub(2).max(1);
        let highlighted = &self.highlights.blocks[block];
        let mut source = Vec::new();
        for line in 0..highlighted.lines.len() {
            highlighted.spans(line, &mut source);
            for spans in super::highlight::wrap_spans(&source, content_width) {
                let mut row = vec![Span::styled("┃ ", gutter)];
                row.extend(spans);
                self.lines.push(Line::from(row));
                self.sources.push(line_source(line));
            }
        }
    }

    fn table<'e>(
        &mut self,
        range: Range<usize>,
        events: &mut impl Iterator<Item = (Event<'e>, Range<usize>)>,
    ) {
        let mut rows = Vec::<Vec<String>>::new();
        let mut row = Vec::<String>::new();
        let mut cell = String::new();
        let mut in_cell = false;
        let mut header_rows = 0_usize;
        let mut in_header = false;
        for (event, _) in events.by_ref() {
            match event {
                Event::Start(Tag::TableHead) => in_header = true,
                Event::End(TagEnd::TableHead) => {
                    if !row.is_empty() {
                        rows.push(std::mem::take(&mut row));
                        header_rows = header_rows.saturating_add(1);
                    }
                    in_header = false;
                }
                Event::Start(Tag::TableCell) => {
                    cell.clear();
                    in_cell = true;
                }
                Event::End(TagEnd::TableCell) => {
                    row.push(cell.trim().to_owned());
                    in_cell = false;
                }
                Event::End(TagEnd::TableRow) => {
                    if in_header {
                        header_rows = header_rows.saturating_add(1);
                    }
                    rows.push(std::mem::take(&mut row));
                }
                Event::End(TagEnd::Table) => break,
                Event::Text(text) | Event::Code(text) if in_cell => {
                    cell.push_str(&sanitize(&text));
                }
                Event::SoftBreak | Event::HardBreak if in_cell => cell.push(' '),
                _ => {}
            }
        }
        for line in render_table(&rows, header_rows, self.width, self.theme) {
            self.push_line(line, Some(range.clone()));
        }
        self.blank();
    }

    fn ensure_prefix(&mut self) {
        if !self.current.is_empty() {
            return;
        }
        self.ensure_quote_prefix();
    }

    fn ensure_quote_prefix(&mut self) {
        for _ in 0..self.quote_depth {
            self.push_unlinked(Span::styled("▌ ", Style::default().fg(self.theme.accent())));
        }
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_current(&mut self, span: Span<'static>) {
        let link = self.links.last().map(|link| Arc::clone(&link.destination));
        self.current.push(TaggedSpan {
            span,
            link,
            source: Some(self.range.clone()),
            anchor: self.anchor,
        });
    }

    fn push_unlinked(&mut self, span: Span<'static>) {
        self.current.push(TaggedSpan {
            span,
            link: None,
            source: None,
            anchor: false,
        });
    }

    fn push_linked(&mut self, span: Span<'static>, destination: Arc<str>) {
        self.current.push(TaggedSpan {
            span,
            link: Some(destination),
            source: Some(self.range.clone()),
            anchor: false,
        });
    }

    fn push_line(&mut self, line: Line<'static>, source: Option<Range<usize>>) {
        self.lines.push(line);
        self.sources.push(source);
    }

    fn render_image_link(&mut self, image: LinkState) {
        self.ensure_prefix();
        let label = if image.label.is_empty() {
            image.destination.to_string()
        } else {
            image.label
        };
        let include_destination = label.trim() != image.destination.as_ref();
        self.push_linked(
            Span::styled(
                label,
                Style::default()
                    .fg(self.theme.accent())
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Arc::clone(&image.destination),
        );
        if include_destination {
            self.push_linked(
                Span::styled(
                    format!(" ↗ {}", image.destination),
                    Style::default()
                        .fg(self.theme.accent())
                        .add_modifier(Modifier::DIM),
                ),
                image.destination,
            );
        }
    }

    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let first_line = self.lines.len();
        let source = self
            .current
            .iter()
            .filter_map(|tagged| tagged.source.clone())
            .reduce(|left, right| left.start.min(right.start)..left.end.max(right.end));
        let rows = wrap_tagged_spans(&self.current, self.width, true);
        if let Some(source) = &source
            && let Some((row, bytes)) = cut(&self.current, &rows)
        {
            self.cut = Some(Cut {
                line: first_line + row,
                bytes,
                group: first_line,
                source: source.start,
            });
        }
        self.group
            .get_or_insert((rows.len(), source.as_ref().map_or(0, |source| source.end)));
        for (offset, (line, links, _)) in rows.into_iter().enumerate() {
            self.push_line(line, source.clone());
            self.rendered_links.extend(
                links
                    .into_iter()
                    .map(|link| (first_line.saturating_add(offset), link)),
            );
        }
        self.current.clear();
    }

    fn blank(&mut self) {
        if self
            .lines
            .last()
            .map_or(self.continued, |line| line.width() == 0)
        {
            return;
        }
        self.push_line(Line::default(), None);
    }

    fn finish(mut self) -> Layout {
        self.flush();
        self.highlights.blocks.truncate(self.block);
        while self.lines.last().is_some_and(|line| line.width() == 0) {
            self.lines.pop();
            self.sources.pop();
        }
        let mut links = vec![Vec::new(); self.lines.len()];
        for (line, link) in self.rendered_links {
            if let Some(line_links) = links.get_mut(line) {
                line_links.push(link);
            }
        }
        Layout {
            lines: self.lines,
            links,
            sources: self.sources,
        }
    }
}

fn source_lines(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut start = text[..range.start].rfind('\n').map_or(0, |index| index + 1);
    let end = range.end.min(text.len());
    let mut lines = Vec::new();
    while start < end {
        let line_end = memchr::memchr(b'\n', &text.as_bytes()[start..end])
            .map_or(end, |offset| start + offset);
        lines.push(start..line_end);
        start = line_end + 1;
    }
    lines
}

fn is_diff_language(language: &str) -> bool {
    let language = language.split_ascii_whitespace().next().unwrap_or_default();
    language.eq_ignore_ascii_case("diff") || language.eq_ignore_ascii_case("patch")
}

fn heading_style(level: HeadingLevel, theme: &Theme) -> Style {
    let color = match level {
        HeadingLevel::H1 => theme.thinking_max(),
        HeadingLevel::H2 => theme.accent(),
        HeadingLevel::H3 => theme.thinking_medium(),
        HeadingLevel::H4 => theme.thinking_high(),
        HeadingLevel::H5 => theme.thinking_xhigh(),
        HeadingLevel::H6 => theme.muted(),
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub fn wrap_spans(spans: &[Span<'static>], width: u16, prefer_words: bool) -> Vec<Line<'static>> {
    wrap_spans_with_whitespace(spans, width, prefer_words, false)
}

fn wrap_spans_with_whitespace(
    spans: &[Span<'_>],
    width: u16,
    prefer_words: bool,
    preserve_whitespace: bool,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    if let Some(lines) = wrap_ascii_spans(
        spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style, None)),
        width,
        prefer_words,
        preserve_whitespace,
    ) {
        return lines.into_iter().map(|(line, ..)| line).collect();
    }
    let graphemes = styled_graphemes(
        spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style, None)),
    );
    wrap_graphemes(&graphemes, width, prefer_words, preserve_whitespace)
        .into_iter()
        .map(|(line, ..)| line)
        .collect()
}

type WrappedRow = (Line<'static>, Vec<LinkSpan>, Range<usize>);

fn wrap_tagged_spans(spans: &[TaggedSpan], width: u16, prefer_words: bool) -> Vec<WrappedRow> {
    if width == 0 {
        return Vec::new();
    }
    let pieces = || {
        spans.iter().map(|tagged| {
            (
                tagged.span.content.as_ref(),
                tagged.span.style,
                tagged.link.as_ref(),
            )
        })
    };
    if let Some(lines) = wrap_ascii_spans(pieces(), width, prefer_words, false) {
        return lines
            .into_iter()
            .map(|(line, links, _, range)| (line, links, range))
            .collect();
    }
    wrap_graphemes(&styled_graphemes(pieces()), width, prefer_words, false)
}

fn styled_graphemes<'a>(
    spans: impl Iterator<Item = (&'a str, Style, Option<&'a Arc<str>>)>,
) -> Vec<StyledGrapheme<'a>> {
    let mut offset = 0;
    spans
        .flat_map(|(content, style, link)| graphemes(content).map(move |text| (text, style, link)))
        .map(|(text, style, link)| {
            let grapheme = StyledGrapheme {
                text,
                style,
                link,
                width: if matches!(text.as_bytes(), [b' '..=b'~']) {
                    1
                } else {
                    u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
                },
                whitespace: text.chars().all(char::is_whitespace),
                offset,
            };
            offset += text.len();
            grapheme
        })
        .collect()
}

fn cut(spans: &[TaggedSpan], rows: &[WrappedRow]) -> Option<(usize, usize)> {
    if !spans.iter().any(|tagged| tagged.anchor) {
        return None;
    }
    let mut starts = Vec::with_capacity(spans.len());
    let mut joined = 0;
    for tagged in spans {
        starts.push(joined);
        joined += tagged.span.content.len();
    }
    let reach = rows
        .iter()
        .scan(0, |reach, row| {
            *reach = row.2.end.max(*reach);
            Some(*reach)
        })
        .collect::<Vec<_>>();
    (1..rows.len()).rev().find_map(|row| {
        let position = rows[row].2.start;
        let index = starts
            .partition_point(|&start| start <= position)
            .checked_sub(1)?;
        let tagged = &spans[index];
        let content = tagged.span.content.as_ref();
        let offset = position - starts[index];
        let settled = if index + 1 < spans.len() {
            content.len()
        } else {
            let trimmed = content.trim_end();
            trimmed.rfind(char::is_whitespace).map_or(0, |space| {
                space + trimmed[space..].chars().next().map_or(0, char::len_utf8)
            })
        };
        (tagged.anchor
            && content
                .get(offset..)
                .is_some_and(|rest| rest.starts_with(char::is_alphabetic))
            && reach[row - 1] <= starts[index] + settled)
            .then(|| {
                tagged
                    .source
                    .as_ref()
                    .map(|source| (row, source.start + offset))
            })
            .flatten()
    })
}

fn graphemes(text: &str) -> impl Iterator<Item = &str> {
    let mut offset = 0;
    let mut ascii_end = 0;
    let mut segmented = None::<unicode_segmentation::Graphemes<'_>>;
    std::iter::from_fn(move || {
        if segmented.is_none() {
            if offset >= ascii_end {
                ascii_end = offset + crate::text::ascii_prefix(&text[offset..], true);
                if ascii_end < text.len() {
                    ascii_end = ascii_end.saturating_sub(1).max(offset);
                }
            }
            if offset < ascii_end {
                offset += 1;
                return Some(&text[offset - 1..offset]);
            }
            segmented = Some(text[offset..].graphemes(true));
        }
        let grapheme = segmented.as_mut()?.next()?;
        offset += grapheme.len();
        if text
            .as_bytes()
            .get(offset)
            .is_some_and(|byte| matches!(byte, b' '..=b'~' | b'\n'))
        {
            segmented = None;
        }
        Some(grapheme)
    })
}

fn wrap_graphemes(
    graphemes: &[StyledGrapheme<'_>],
    width: u16,
    prefer_words: bool,
    preserve_whitespace: bool,
) -> Vec<WrappedRow> {
    if graphemes.is_empty() {
        return vec![(Line::default(), Vec::new(), 0..0)];
    }
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, grapheme) in graphemes.iter().enumerate() {
        if grapheme.text == "\n" {
            lines.extend(wrap_visual_line(
                &graphemes[start..index],
                width,
                prefer_words,
                preserve_whitespace,
            ));
            start = index + 1;
        }
    }
    if start < graphemes.len()
        || graphemes
            .last()
            .is_some_and(|grapheme| grapheme.text == "\n")
    {
        lines.extend(wrap_visual_line(
            &graphemes[start..],
            width,
            prefer_words,
            preserve_whitespace,
        ));
    }
    lines
}

fn wrap_visual_line(
    graphemes: &[StyledGrapheme<'_>],
    width: u16,
    prefer_words: bool,
    preserve_whitespace: bool,
) -> Vec<WrappedRow> {
    let Some(last) = graphemes.last() else {
        return vec![(Line::default(), Vec::new(), 0..0)];
    };

    let mut lines = Vec::new();
    let mut start = 0_usize;
    while start < graphemes.len() {
        if preserve_whitespace && start > 0 && graphemes[start].text == " " {
            start += 1;
        }
        if !preserve_whitespace {
            while start < graphemes.len() && graphemes[start].whitespace {
                start += 1;
            }
        }
        if start == graphemes.len() {
            break;
        }
        let mut end = start;
        let mut used = 0_u16;
        let mut word_break = None;
        let first_content = graphemes[start..]
            .iter()
            .position(|grapheme| !grapheme.whitespace)
            .map_or(start, |offset| start.saturating_add(offset));
        while end < graphemes.len() {
            let next = used.saturating_add(graphemes[end].width);
            if next > width && end > start {
                break;
            }
            if graphemes[end].whitespace && (!preserve_whitespace || end > first_content) {
                word_break = Some(end);
            }
            used = next;
            end += 1;
            if used >= width {
                break;
            }
        }
        let split = if prefer_words && end < graphemes.len() {
            word_break.filter(|&index| index > start).unwrap_or(end)
        } else {
            end
        };
        let (line, links) = graphemes_to_line(&graphemes[start..split], preserve_whitespace);
        let examined = graphemes.get(end).unwrap_or(last);
        lines.push((
            line,
            links,
            graphemes[start].offset..examined.offset + examined.text.len(),
        ));
        start = split.max(start + 1);
    }
    lines
}

struct StyledGrapheme<'a> {
    text: &'a str,
    style: Style,
    link: Option<&'a Arc<str>>,
    width: u16,
    whitespace: bool,
    offset: usize,
}

fn graphemes_to_line(
    graphemes: &[StyledGrapheme<'_>],
    preserve_whitespace: bool,
) -> (Line<'static>, Vec<LinkSpan>) {
    let mut spans = Vec::<Span<'static>>::new();
    let mut links = Vec::<LinkSpan>::new();
    let mut column = 0_u16;
    let rendered_len = if preserve_whitespace {
        graphemes.len()
    } else {
        graphemes
            .iter()
            .rposition(|grapheme| !grapheme.whitespace)
            .map_or(0, |index| index.saturating_add(1))
    };
    for run in graphemes[..rendered_len].chunk_by(|left, right| left.style == right.style) {
        let mut content =
            String::with_capacity(run.iter().map(|grapheme| grapheme.text.len()).sum());
        for grapheme in run {
            content.push_str(grapheme.text);
        }
        spans.push(Span::styled(content, run[0].style));
    }
    for grapheme in &graphemes[..rendered_len] {
        if let Some(destination) = grapheme.link
            && grapheme.width > 0
        {
            let end = column.saturating_add(grapheme.width);
            if let Some(last) = links.last_mut()
                && last.destination == *destination
                && last.end == column
            {
                last.end = end;
            } else {
                links.push(LinkSpan {
                    destination: Arc::clone(destination),
                    start: column,
                    end,
                });
            }
        }
        column = column.saturating_add(grapheme.width);
    }
    (Line::from(spans), links)
}

type AsciiRow = (Line<'static>, Vec<LinkSpan>, usize, Range<usize>);

fn wrap_ascii_spans<'a>(
    spans: impl Iterator<Item = (&'a str, Style, Option<&'a Arc<str>>)>,
    width: u16,
    prefer_words: bool,
    preserve_whitespace: bool,
) -> Option<Vec<AsciiRow>> {
    if width == 0 {
        return Some(Vec::new());
    }
    let spans = spans
        .filter(|(text, _, _)| !text.is_empty())
        .collect::<Vec<_>>();
    if spans
        .iter()
        .any(|(text, _, _)| crate::text::ascii_prefix(text, true) != text.len())
    {
        return None;
    }
    let text = if spans.len() == 1 {
        std::borrow::Cow::Borrowed(spans[0].0)
    } else {
        std::borrow::Cow::Owned(spans.iter().map(|(text, _, _)| *text).collect::<String>())
    };
    let mut index = 0;
    let mut offset = 0;
    let mut lines = Vec::new();
    for (range, line, window) in
        crate::text::ascii_rows(&text, usize::from(width), prefer_words, preserve_whitespace)
    {
        while index < spans.len() && offset + spans[index].0.len() <= range.start {
            offset += spans[index].0.len();
            index += 1;
        }
        let mut position = range.start;
        let mut rendered = Vec::<Span<'static>>::new();
        let mut links = Vec::<LinkSpan>::new();
        while position < range.end {
            let (content, style, link) = spans[index];
            let end = range.end.min(offset + content.len());
            let slice = &text[position..end];
            if let Some(last) = rendered.last_mut()
                && last.style == style
            {
                last.content.to_mut().push_str(slice);
            } else {
                rendered.push(Span::styled(slice.to_owned(), style));
            }
            if let Some(destination) = link {
                let start = (position - range.start) as u16;
                let end = (end - range.start) as u16;
                if let Some(last) = links.last_mut()
                    && last.destination == *destination
                    && last.end == start
                {
                    last.end = end;
                } else {
                    links.push(LinkSpan {
                        destination: Arc::clone(destination),
                        start,
                        end,
                    });
                }
            }
            position = end;
            if position == offset + content.len() {
                offset += content.len();
                index += 1;
            }
        }
        lines.push((
            Line::from(rendered),
            links,
            line,
            range.start..(window + 1).min(text.len()),
        ));
    }
    Some(lines)
}

fn hard_wrap(text: &str, width: u16) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if width > 0 && crate::text::ascii_prefix(text, false) == text.len() {
        return text
            .as_bytes()
            .chunks(usize::from(width))
            .scan(0, |offset, chunk| {
                let line = text[*offset..*offset + chunk.len()].to_owned();
                *offset += chunk.len();
                Some(line)
            })
            .collect();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0_u16;
    for grapheme in text.graphemes(true) {
        let grapheme_width = u16::try_from(UnicodeWidthStr::width(grapheme)).unwrap_or(u16::MAX);
        if used.saturating_add(grapheme_width) > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push_str(grapheme);
        used = used.saturating_add(grapheme_width);
    }
    lines.push(line);
    lines
}

fn code_block_header(language: Option<&str>, width: u16, theme: &Theme) -> Line<'static> {
    let border = Style::default().fg(theme.border());
    let Some(language) = language else {
        return Line::from(Span::styled(
            format!("╭{}╮", "─".repeat(usize::from(width.saturating_sub(2)))),
            border,
        ));
    };
    let available = width.saturating_sub(5);
    let language = truncate_graphemes(language, available);
    let language_width =
        u16::try_from(UnicodeWidthStr::width(language.as_str())).unwrap_or(u16::MAX);
    let fill = width.saturating_sub(language_width.saturating_add(5));
    Line::from(vec![
        Span::styled("╭─ ", border),
        Span::styled(
            language,
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}╮", "─".repeat(usize::from(fill))), border),
    ])
}

pub(crate) fn truncate_graphemes(text: &str, width: u16) -> String {
    if crate::text::ascii_prefix(text, false) == text.len() {
        return text[..text.len().min(usize::from(width))].to_owned();
    }
    let mut rendered = String::new();
    let mut used = 0_u16;
    for grapheme in text.graphemes(true) {
        let grapheme_width = u16::try_from(UnicodeWidthStr::width(grapheme)).unwrap_or(u16::MAX);
        if used.saturating_add(grapheme_width) > width {
            break;
        }
        rendered.push_str(grapheme);
        used = used.saturating_add(grapheme_width);
    }
    rendered
}

fn render_table(
    rows: &[Vec<String>],
    header_rows: usize,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 || width < 4 {
        return Vec::new();
    }
    let border_cells = u16::try_from(columns.saturating_add(1)).unwrap_or(u16::MAX);
    let available = width.saturating_sub(border_cells);
    if available < u16::try_from(columns.saturating_mul(3)).unwrap_or(u16::MAX) {
        return render_stacked_table(rows, header_rows, width, theme);
    }
    let mut widths = vec![3_u16; columns];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column]
                .max(u16::try_from(UnicodeWidthStr::width(cell.as_str())).unwrap_or(u16::MAX));
        }
    }
    while widths.iter().copied().sum::<u16>() > available {
        let Some((index, _)) = widths.iter().enumerate().max_by_key(|(_, width)| **width) else {
            break;
        };
        if widths[index] <= 3 {
            break;
        }
        widths[index] -= 1;
    }

    let mut lines = Vec::new();
    lines.push(table_rule('╭', '┬', '╮', &widths, theme));
    for (row_index, row) in rows.iter().enumerate() {
        let wrapped = widths
            .iter()
            .enumerate()
            .map(|(column, &cell_width)| {
                hard_wrap(row.get(column).map_or("", String::as_str), cell_width)
            })
            .collect::<Vec<_>>();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line_index in 0..height {
            let mut spans = vec![Span::styled("│", Style::default().fg(theme.border()))];
            for (column, &cell_width) in widths.iter().enumerate() {
                let text = wrapped[column].get(line_index).map_or("", String::as_str);
                let padding = usize::from(cell_width).saturating_sub(UnicodeWidthStr::width(text));
                let style = if row_index < header_rows {
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text())
                };
                spans.push(Span::styled(
                    format!("{text}{}", " ".repeat(padding)),
                    style,
                ));
                spans.push(Span::styled("│", Style::default().fg(theme.border())));
            }
            lines.push(Line::from(spans));
        }
        if row_index + 1 < rows.len() {
            lines.push(table_rule('├', '┼', '┤', &widths, theme));
        }
    }
    lines.push(table_rule('╰', '┴', '╯', &widths, theme));
    lines
}

fn table_rule(
    left: char,
    middle: char,
    right: char,
    widths: &[u16],
    theme: &Theme,
) -> Line<'static> {
    let mut rule = left.to_string();
    for (index, width) in widths.iter().enumerate() {
        rule.push_str(&"─".repeat(usize::from(*width)));
        rule.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    Line::from(Span::styled(rule, Style::default().fg(theme.border())))
}

fn render_stacked_table(
    rows: &[Vec<String>],
    header_rows: usize,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(Span::styled(
        format!("╭{}╮", "─".repeat(usize::from(width.saturating_sub(2)))),
        Style::default().fg(theme.border()),
    ))];
    for row in rows.iter().skip(header_rows.max(1)) {
        for (column, value) in row.iter().enumerate() {
            let header = headers.get(column).map_or("Value", String::as_str);
            let content = format!("{header}: {value}");
            for line in hard_wrap(&content, width.saturating_sub(4).max(1)) {
                let padding = usize::from(width.saturating_sub(4))
                    .saturating_sub(UnicodeWidthStr::width(line.as_str()));
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(theme.border())),
                    Span::styled(line, Style::default().fg(theme.text())),
                    Span::raw(" ".repeat(padding)),
                    Span::styled(" │", Style::default().fg(theme.border())),
                ]));
            }
        }
        if row.as_ptr() != rows.last().map_or(row.as_ptr(), Vec::as_ptr) {
            lines.push(Line::from(Span::styled(
                format!("├{}┤", "─".repeat(usize::from(width.saturating_sub(2)))),
                Style::default().fg(theme.border()),
            )));
        }
    }
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(usize::from(width.saturating_sub(2)))),
        Style::default().fg(theme.border()),
    )));
    lines
}
