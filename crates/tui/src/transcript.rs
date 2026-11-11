use std::{
    borrow::Cow,
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

use ratatui::{
    buffer::{Buffer, CellWidth},
    layout::{Alignment, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use syntect::easy::HighlightLines;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    beautify::beautify,
    diff,
    format::{
        format_duration, sanitize_terminal_text, sanitize_terminal_text_inline,
        strip_terminal_controls,
    },
    highlight, markdown,
    spinner::{SPINNER_SLOT, elapsed_label, elapsed_label_deadline},
    theme::Theme,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Yielded,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisplayMode {
    #[default]
    Collapsed,
    Beautified,
    Raw,
}

impl DisplayMode {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Collapsed => Self::Beautified,
            Self::Beautified => Self::Raw,
            Self::Raw => Self::Collapsed,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Collapsed => "collapsed",
            Self::Beautified => "beautified",
            Self::Raw => "raw",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolBody {
    Text,
    Code,
    Diff,
}

#[derive(Clone, Debug)]
pub struct ToolCell {
    pub call_id: String,
    pub name: String,
    pub title: String,
    pub input: String,
    pub input_body: ToolBody,
    pub output: String,
    pub output_body: ToolBody,
    pub status: ToolStatus,
    pub duration: Option<Duration>,
    pub started_at: Instant,
    pub depth: usize,
    pub raw_output: String,
    pub language: String,
    pub exit_code: Option<i64>,
    pub details: Vec<String>,
    pub children: Vec<Self>,
}

impl ToolCell {
    #[must_use]
    pub fn new(call_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            title: String::new(),
            input: String::new(),
            input_body: ToolBody::Text,
            output: String::new(),
            output_body: ToolBody::Text,
            status: ToolStatus::Running,
            duration: None,
            started_at: Instant::now(),
            depth: 0,
            raw_output: String::new(),
            language: String::new(),
            exit_code: None,
            details: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn find_mut(&mut self, call_id: &str) -> Option<&mut Self> {
        if self.call_id == call_id {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(call_id))
    }

    #[must_use]
    pub fn contains(&self, call_id: &str) -> bool {
        self.call_id == call_id || self.children.iter().any(|child| child.contains(call_id))
    }

    fn running(&self) -> bool {
        self.status == ToolStatus::Running || self.children.iter().any(Self::running)
    }

    fn finish_running(&mut self, status: ToolStatus) {
        if self.status == ToolStatus::Running {
            self.status = status;
        }
        for child in &mut self.children {
            child.finish_running(status);
        }
    }

    fn unsuccessful(&self) -> bool {
        matches!(self.status, ToolStatus::Failed) || self.exit_code.is_some_and(|code| code != 0)
    }
}

/// A message from someone, as chat apps show one: a header naming them, their
/// words as Markdown (where a line break stays one) and a bar beside both.
#[derive(Clone, Debug)]
pub struct Said {
    /// Who and when; none continues the message above as one group.
    pub header: Option<Line<'static>>,
    pub text: String,
    /// The bar's colour, or no bar.
    pub bar: Option<Color>,
    /// Its rows on the code background, which sets it apart.
    pub band: bool,
}

#[derive(Clone, Debug)]
pub enum CellKind {
    User(String),
    Said(Said),
    Assistant(String),
    Reasoning(String),
    Tool(ToolCell),
    Notice(NoticeLevel, String),
    Btw {
        question: String,
        answer: String,
        done: bool,
    },
    Summary(String),
}

#[derive(Clone, Debug)]
struct Cell {
    kind: CellKind,
    revision: u64,
    appended: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheKey {
    width: u16,
    revision: u64,
    mode: DisplayMode,
    reasoning: bool,
}

struct Layout {
    key: CacheKey,
    rows: usize,
    lines: Option<Rows>,
    highlights: markdown::Highlights,
    stable: Option<Stable>,
    formatted: Vec<Shown>,
}

struct Stable {
    rows: Rc<Output>,
    spinners: Vec<(usize, usize)>,
    body: usize,
    bytes: usize,
    blocks: usize,
    continuation: markdown::Continuation,
    open: Option<(usize, usize)>,
    fallback: (usize, usize, usize, usize),
}

#[derive(Clone)]
struct Rows {
    head: Rc<Output>,
    tail: Rc<Output>,
    spinners: Rc<[(usize, usize)]>,
    group: Option<(Range<usize>, Option<Source>)>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Point {
    cell: usize,
    row: usize,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: Point,
    head: Option<Point>,
    pointer: Option<u16>,
}

impl Selection {
    fn bounds(&self) -> Option<(Point, Point)> {
        self.head
            .map(|head| (head.min(self.anchor), head.max(self.anchor)))
    }
}

impl Rows {
    fn len(&self) -> usize {
        self.head.lines.len() + self.tail.lines.len()
    }

    fn part(&self, row: usize) -> (&Output, usize) {
        match row.checked_sub(self.head.lines.len()) {
            Some(row) => (&self.tail, row),
            None => (&self.head, row),
        }
    }

    fn line(&self, row: usize) -> &Line<'static> {
        let (part, row) = self.part(row);
        &part.lines[row]
    }

    fn link(&self, row: usize, column: u16) -> Option<&str> {
        let (part, row) = self.part(row);
        part.links
            .iter()
            .find(|(slot, link)| *slot == row && (link.start..link.end).contains(&column))
            .map(|(_, link)| link.destination.as_ref())
    }

    fn source(&self, row: usize) -> Option<&Option<Source>> {
        if let Some((rows, source)) = &self.group
            && rows.contains(&row)
        {
            return Some(source);
        }
        let (part, row) = self.part(row);
        part.sources.get(row)
    }

    fn clocks(&self, row: usize) -> impl Iterator<Item = Instant> + '_ {
        let (part, row) = self.part(row);
        part.clocks
            .iter()
            .filter(move |(slot, ..)| *slot == row)
            .map(|&(_, _, started_at)| started_at)
    }

    fn animated(&self, row: usize, spinner: &str) -> Line<'static> {
        let mut line = self.line(row).clone();
        for (_, span) in self.spinners.iter().filter(|(slot, _)| *slot == row) {
            line.spans[*span].content = Cow::Owned(spinner.to_owned());
        }
        let (part, row) = self.part(row);
        for &(_, span, started_at) in part.clocks.iter().filter(|(slot, ..)| *slot == row) {
            line.spans[span].content = Cow::Owned(format!(
                " · {}",
                elapsed_label(Instant::now().saturating_duration_since(started_at))
            ));
        }
        line
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TranscriptPolicy {
    pub collapsed_preview_lines: usize,
    pub collapsed_diff_lines: usize,
}

pub struct Transcript {
    cells: Vec<Cell>,
    cache: Vec<Option<Layout>>,
    resident: Vec<usize>,
    revision: u64,
    scroll: usize,
    mode: DisplayMode,
    reasoning: bool,
    policy: TranscriptPolicy,
    last_height: usize,
    last_total: Option<usize>,
    last_width: u16,
    theme: Option<Theme>,
    anchor: Option<(usize, usize)>,
    area: Rect,
    screen: Vec<Point>,
    selection: Option<Selection>,
    spinner_cells: Vec<Position>,
    clock_deadline: Option<Instant>,
}

impl Transcript {
    #[must_use]
    pub const fn new(policy: TranscriptPolicy) -> Self {
        Self {
            cells: Vec::new(),
            cache: Vec::new(),
            resident: Vec::new(),
            revision: 0,
            scroll: 0,
            mode: DisplayMode::Collapsed,
            reasoning: true,
            policy,
            last_height: 0,
            last_total: None,
            last_width: 0,
            theme: None,
            anchor: None,
            area: Rect::ZERO,
            screen: Vec::new(),
            selection: None,
            spinner_cells: Vec::new(),
            clock_deadline: None,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.cells.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn clear(&mut self) {
        self.cells.clear();
        self.cache.clear();
        self.resident.clear();
        self.scroll = 0;
        self.last_total = None;
        self.anchor = None;
        self.screen.clear();
        self.selection = None;
    }

    pub fn push(&mut self, kind: CellKind) -> usize {
        self.revision += 1;
        self.cells.push(Cell {
            kind,
            revision: self.revision,
            appended: false,
        });
        self.cache.push(None);
        self.last_total = None;
        self.cells.len() - 1
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&CellKind> {
        self.cells.get(index).map(|cell| &cell.kind)
    }

    pub fn update(&mut self, index: usize, change: impl FnOnce(&mut CellKind)) -> bool {
        let Some(cell) = self.cells.get_mut(index) else {
            return false;
        };
        change(&mut cell.kind);
        self.revision += 1;
        cell.revision = self.revision;
        cell.appended = false;
        self.last_total = None;
        true
    }

    pub fn append(&mut self, index: usize, text: &str) -> bool {
        let Some(cell) = self.cells.get_mut(index) else {
            return false;
        };
        let (CellKind::Assistant(existing)
        | CellKind::Reasoning(existing)
        | CellKind::Btw {
            answer: existing, ..
        }) = &mut cell.kind
        else {
            return false;
        };
        existing.push_str(text);
        self.revision += 1;
        cell.revision = self.revision;
        self.last_total = None;
        true
    }

    #[must_use]
    pub fn last(&self) -> Option<(usize, &CellKind)> {
        self.cells
            .last()
            .map(|cell| (self.cells.len() - 1, &cell.kind))
    }

    pub fn invalidate(&mut self) {
        self.cache.iter_mut().for_each(|entry| *entry = None);
        self.resident.clear();
    }

    #[must_use]
    pub fn has_notice_since(&self, index: usize, level: NoticeLevel, text: &str) -> bool {
        self.cells.iter().skip(index).any(|cell| {
            matches!(&cell.kind, CellKind::Notice(existing, message) if *existing == level && message == text)
        })
    }

    #[must_use]
    pub fn find_tool(&self, call_id: &str) -> Option<usize> {
        self.cells
            .iter()
            .rposition(|cell| matches!(&cell.kind, CellKind::Tool(tool) if tool.contains(call_id)))
    }

    pub fn finish_running_tools(&mut self, status: ToolStatus) {
        for index in 0..self.cells.len() {
            if matches!(&self.cells[index].kind, CellKind::Tool(tool) if tool.running()) {
                self.update(index, |kind| {
                    if let CellKind::Tool(tool) = kind {
                        tool.finish_running(status);
                    }
                });
            }
        }
    }

    #[must_use]
    pub const fn expanded(&self) -> bool {
        !matches!(self.mode, DisplayMode::Collapsed)
    }

    pub const fn toggle_expanded(&mut self) {
        self.mode = if self.expanded() {
            DisplayMode::Collapsed
        } else {
            DisplayMode::Beautified
        };
        self.last_total = None;
    }

    #[must_use]
    pub const fn mode(&self) -> DisplayMode {
        self.mode
    }

    pub const fn set_mode(&mut self, mode: DisplayMode) {
        self.mode = mode;
        self.last_total = None;
    }

    pub const fn cycle_mode(&mut self) -> DisplayMode {
        self.mode = self.mode.next();
        self.last_total = None;
        self.mode
    }

    #[must_use]
    pub const fn shows_reasoning(&self) -> bool {
        self.reasoning
    }

    pub const fn toggle_reasoning(&mut self) {
        self.reasoning = !self.reasoning;
        self.last_total = None;
    }

    #[must_use]
    pub const fn is_following(&self) -> bool {
        self.scroll == 0
    }

    fn limit(&mut self, needed: usize) -> Option<usize> {
        if self.last_total.is_none() {
            let _span = tracing::info_span!(
                "tui.transcript.extent",
                cells = self.cells.len(),
                needed,
                cached = false
            )
            .entered();
            let Some(theme) = self.theme.take() else {
                return Some(0);
            };
            let mut total = 0;
            let mut index = self.cells.len();
            let below = needed.saturating_sub(self.last_height);
            while index > 0 && total < needed {
                index -= 1;
                total += self.rows(index, self.last_width, &theme);
                if total <= below {
                    self.evict(index);
                }
            }
            self.theme = Some(theme);
            if index == 0 {
                self.last_total = Some(total);
            }
        }
        self.last_total
            .map(|total| total.saturating_sub(self.last_height))
    }

    #[must_use]
    pub const fn scroll_offset(&self) -> usize {
        self.scroll
    }

    pub fn scroll_up(&mut self, lines: usize) {
        let scroll = self.scroll.saturating_add(lines);
        let needed = scroll.saturating_add(self.last_height);
        self.scroll = self.limit(needed).map_or(scroll, |limit| scroll.min(limit));
    }

    pub fn scroll_down(&mut self, lines: usize) {
        if self.scroll == 0 {
            return;
        }
        let needed = self.scroll.saturating_add(self.last_height);
        let scroll = self
            .limit(needed)
            .map_or(self.scroll, |limit| self.scroll.min(limit));
        self.scroll = scroll.saturating_sub(lines);
    }

    pub const fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

    /// Scrolls so that cell `index` starts at the top, below the blank row
    /// above it, or as near as the end allows.
    pub fn scroll_to(&mut self, index: usize) {
        if index >= self.cells.len() {
            return;
        }
        let Some(theme) = self.theme.take() else {
            return;
        };
        let mut total = 0;
        for cell in index..self.cells.len() {
            total += self.rows(cell, self.last_width, &theme);
        }
        self.theme = Some(theme);
        let gap = usize::from(index > 0);
        self.scroll = total.saturating_sub(gap + self.last_height);
        self.anchor = Some((index, total));
    }

    pub fn scroll_to_top(&mut self) {
        if let (Some(limit), Some(total)) = (self.limit(usize::MAX), self.last_total) {
            self.scroll = limit;
            self.anchor = Some((0, total));
        }
    }

    #[must_use]
    pub const fn page(&self) -> usize {
        if self.last_height > 2 {
            self.last_height - 2
        } else {
            1
        }
    }

    pub fn select_start(&mut self, column: u16, row: u16) -> bool {
        if !self.area.contains(Position::new(column, row)) {
            return false;
        }
        let Some(anchor) = self.point_at(row) else {
            return false;
        };
        self.selection = Some(Selection {
            anchor,
            head: None,
            pointer: None,
        });
        true
    }

    pub fn select_to(&mut self, row: u16) -> bool {
        let Some(previous) = self.selection.map(|selection| selection.bounds()) else {
            return false;
        };
        self.point_to(row);
        self.selection.and_then(|selection| selection.bounds()) != previous
    }

    fn point_to(&mut self, row: u16) {
        let head = self.point_at(row);
        if let Some(selection) = &mut self.selection {
            selection.pointer = Some(row);
            selection.head = head;
        }
    }

    pub fn autoscroll(&mut self) -> bool {
        let Some(row) = self.selection.and_then(|selection| selection.pointer) else {
            return false;
        };
        let previous = self.scroll;
        let (top, bottom) = (self.area.top(), self.area.bottom().saturating_sub(1));
        if row <= top {
            self.scroll_up(usize::from(top - row) + 1);
        } else if row >= bottom {
            self.scroll_down(usize::from(row - bottom) + 1);
        }
        self.scroll != previous
    }

    #[must_use]
    pub fn autoscrolling(&self) -> bool {
        self.selection
            .and_then(|selection| selection.pointer)
            .is_some_and(|row| row <= self.area.top() || row + 1 >= self.area.bottom())
    }

    pub fn select_end(&mut self) -> Option<String> {
        let (start, end) = self.selection.take()?.bounds()?;
        let mut cells = Vec::new();
        for index in start.cell..=end.cell {
            let Some(rows) = self.cell_rows(index) else {
                continue;
            };
            let first = if index == start.cell { start.row } else { 0 };
            let last = if index == end.cell {
                end.row
            } else {
                rows.len().saturating_sub(1)
            };
            if last >= rows.len() {
                continue;
            }
            let text = selected_text(&rows, first, last, &self.cells[index].kind, self.mode);
            if !text.is_empty() {
                cells.push(text);
            }
        }
        let text = cells.join("\n\n");
        (!text.is_empty()).then_some(text)
    }

    pub const fn select_cancel(&mut self) {
        self.selection = None;
    }

    pub fn link_at(&mut self, column: u16, row: u16) -> Option<String> {
        if !self.area.contains(Position::new(column, row)) {
            return None;
        }
        let point = *self.screen.get(usize::from(row - self.area.top()))?;
        let rows = self.cell_rows(point.cell)?;
        let column = column - self.area.left();
        rows.link(point.row, column)
            .map(str::to_owned)
            .or_else(|| url_at(&rows, point.row, column, self.last_width))
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    }

    fn point_at(&self, row: u16) -> Option<Point> {
        let offset = usize::from(row.saturating_sub(self.area.top()));
        self.screen.get(offset).or(self.screen.last()).copied()
    }

    fn cell_rows(&mut self, index: usize) -> Option<Rows> {
        if index >= self.cells.len() {
            return None;
        }
        let theme = self.theme.take()?;
        let rows = self.layout(index, self.last_width, &theme);
        self.theme = Some(theme);
        Some(rows)
    }

    fn key(&self, index: usize, width: u16) -> CacheKey {
        CacheKey {
            width,
            revision: self.cells[index].revision,
            mode: self.mode,
            reasoning: self.reasoning,
        }
    }

    fn rows(&mut self, index: usize, width: u16, theme: &Theme) -> usize {
        match &self.cache[index] {
            Some(layout) if layout.key == self.key(index, width) => layout.rows,
            _ => self.layout(index, width, theme).len(),
        }
    }

    fn evict(&mut self, index: usize) {
        let live = self.cells[index].revision == self.revision;
        if let Some(Some(layout)) = self.cache.get_mut(index)
            && layout.release(live)
        {
            self.resident.retain(|&resident| resident != index);
        }
    }

    fn layout(&mut self, index: usize, width: u16, theme: &Theme) -> Rows {
        let key = self.key(index, width);
        let cell = &self.cells[index];
        if let Some(Layout {
            key: cached,
            lines: Some(rows),
            ..
        }) = &self.cache[index]
            && *cached == key
        {
            return rows.clone();
        }
        let span = tracing::info_span!(
            "tui.transcript.layout",
            cell.index = index,
            cell.revision = cell.revision,
            width,
            mode = self.mode.label(),
            layout.lines = tracing::field::Empty
        );
        let _entered = span.enter();
        let previous = self.cache[index].take();
        if !previous
            .as_ref()
            .is_some_and(|layout| layout.lines.is_some())
        {
            self.resident.push(index);
        }
        let (mut highlights, stable, cached) = previous.map_or_else(Default::default, |layout| {
            let reusable = cell.appended
                && layout.key.width == width
                && layout.key.mode == key.mode
                && layout.key.reasoning == key.reasoning;
            (
                layout.highlights,
                layout.stable.filter(|_| reusable),
                layout.formatted,
            )
        });
        let mut formatted = Formatted {
            cached,
            used: Vec::new(),
        };
        let context = RenderContext {
            width,
            theme,
            mode: self.mode,
            reasoning: self.reasoning || self.mode != DisplayMode::Collapsed,
            preview: self.policy.collapsed_preview_lines,
            diff_preview: self.policy.collapsed_diff_lines,
        };
        let (rows, stable) = context
            .stream(&cell.kind, index, stable, &mut highlights)
            .unwrap_or_else(|| {
                let mut out = Output::default();
                if index > 0 {
                    out.push(Line::default(), None);
                }
                context.render(&cell.kind, &mut out, &mut formatted);
                let rows = Rows {
                    head: Rc::default(),
                    spinners: spinners(&out.lines, 0).collect(),
                    tail: Rc::new(out),
                    group: None,
                };
                (rows, None)
            });
        span.record("layout.lines", rows.len());
        self.cells[index].appended = true;
        self.cache[index] = Some(Layout {
            key,
            rows: rows.len(),
            lines: Some(rows.clone()),
            highlights,
            stable,
            formatted: formatted.used,
        });
        rows
    }

    fn extend(
        &mut self,
        layouts: &mut Vec<usize>,
        total: &mut usize,
        needed: usize,
        floor: usize,
        width: u16,
        theme: &Theme,
    ) {
        let mut index = self.cells.len() - layouts.len();
        while index > 0 && (*total < needed || index > floor) {
            index -= 1;
            let rows = self.rows(index, width, theme);
            *total += rows;
            layouts.push(rows);
            if *total <= self.scroll {
                self.evict(index);
            }
        }
    }

    #[must_use]
    pub fn spinner_cells(&self) -> &[Position] {
        &self.spinner_cells
    }

    #[must_use]
    pub const fn clock_deadline(&self) -> Option<Instant> {
        self.clock_deadline
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, spinner: &str) {
        let span = tracing::info_span!(
            "tui.transcript.render",
            width = area.width,
            height = area.height,
            cells = self.cells.len(),
            mode = self.mode.label(),
            scroll.before = self.scroll,
            scroll.after = tracing::field::Empty,
            layout.visited_cells = tracing::field::Empty
        );
        let _entered = span.enter();
        self.spinner_cells.clear();
        self.clock_deadline = None;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width;
        let height = usize::from(area.height);
        if width != self.last_width {
            self.selection = None;
        }
        self.last_width = width;
        if self.theme.as_ref() != Some(theme) {
            self.invalidate();
            self.theme = Some(theme.clone());
        }
        let count = self.cells.len();
        let pinned = self.scroll > 0 || self.selection.is_some();
        let floor = match self.anchor {
            Some((first, _)) if pinned => first.min(count),
            _ => count,
        };
        let mut layouts = Vec::new();
        let mut total = 0;
        self.extend(
            &mut layouts,
            &mut total,
            self.scroll.saturating_add(height),
            floor,
            width,
            theme,
        );
        if pinned
            && let Some((first, lines)) = self.anchor
            && first <= count
            && count - first <= layouts.len()
        {
            let since = layouts[..count - first].iter().sum::<usize>();
            self.scroll += since.saturating_sub(lines);
        }
        self.extend(
            &mut layouts,
            &mut total,
            self.scroll.saturating_add(height),
            count,
            width,
            theme,
        );
        let first = count - layouts.len();
        span.record("layout.visited_cells", layouts.len());
        self.anchor = Some((first, total));
        self.last_total = (first == 0).then_some(total);
        self.last_height = height;
        self.scroll = self.scroll.min(total.saturating_sub(height));
        span.record("scroll.after", self.scroll);
        let end = total - self.scroll;
        let start = end.saturating_sub(height);
        let mut rows = area
            .intersection(buffer.area)
            .rows()
            .filter(|row| !row.is_empty());
        let mut offset = 0;
        let mut visible = Vec::new();
        for (index, &lines) in (first..count).zip(layouts.iter().rev()) {
            let next = offset + lines;
            if next > start && offset < end {
                visible.push((index, offset));
            }
            offset = next;
            if offset >= end {
                break;
            }
        }
        let (cells, cache, revision) = (&self.cells, &mut self.cache, self.revision);
        let shown = match (visible.first(), visible.last()) {
            (Some(&(top, _)), Some(&(bottom, _))) => top..bottom + 1,
            _ => 0..0,
        };
        self.resident.retain(|index| {
            let keep = shown.contains(index);
            if !keep && let Some(Some(layout)) = cache.get_mut(*index) {
                layout.release(cells[*index].revision == revision);
            }
            keep
        });
        self.area = area.intersection(buffer.area);
        self.screen.clear();
        'layouts: for (index, offset) in visible {
            let layout = self.layout(index, width, theme);
            let count = layout.len();
            let from = start.saturating_sub(offset);
            let to = (end - offset).min(count);
            for row in from..to {
                let Some(row_area) = rows.next() else {
                    break 'layouts;
                };
                if layout.spinners.iter().any(|(slot, _)| *slot == row) {
                    let now = Instant::now();
                    for started_at in layout.clocks(row) {
                        let deadline = elapsed_label_deadline(started_at, now);
                        self.clock_deadline = Some(
                            self.clock_deadline
                                .map_or(deadline, |current| current.min(deadline)),
                        );
                    }
                    let cells = &mut self.spinner_cells;
                    render_line(
                        &layout.animated(row, spinner),
                        row_area,
                        buffer,
                        |span, x| {
                            if x < row_area.right() && layout.spinners.contains(&(row, span)) {
                                cells.push(Position::new(x, row_area.y));
                            }
                        },
                    );
                } else {
                    render_line(layout.line(row), row_area, buffer, |_, _| {});
                }
                self.screen.push(Point { cell: index, row });
            }
        }
        if let Some(pointer) = self.selection.and_then(|selection| selection.pointer) {
            self.point_to(pointer);
        }
        if let Some((first, last)) = self.selection.and_then(|selection| selection.bounds()) {
            let area = self.area;
            for (y, point) in (area.y..).zip(&self.screen) {
                if (first..=last).contains(point) {
                    buffer.set_style(
                        Rect::new(area.x, y, area.width, 1),
                        Style::default().add_modifier(Modifier::REVERSED),
                    );
                }
            }
        }
        if self.scroll > 0 {
            let label = format!(" ↓ {} more lines · end to follow ", self.scroll);
            let label_width = u16::try_from(label.chars().count())
                .unwrap_or(u16::MAX)
                .min(area.width);
            let x = area.x + area.width - label_width;
            let visible =
                Rect::new(x, area.y + area.height - 1, label_width, 1).intersection(buffer.area);
            if !visible.is_empty() {
                buffer.set_stringn(
                    visible.x,
                    visible.y,
                    label
                        .chars()
                        .skip(usize::from(visible.x - x))
                        .collect::<String>(),
                    usize::from(visible.width),
                    Style::default()
                        .fg(theme.code_background())
                        .bg(theme.accent()),
                );
            }
        }
    }
}

impl Stable {
    fn rewind(&mut self) {
        let (lines, body, bytes, blocks) = self.fallback;
        let head = Rc::make_mut(&mut self.rows);
        head.lines.truncate(lines);
        head.sources.truncate(lines);
        head.links.retain(|&(row, _)| row < lines);
        self.spinners.retain(|&(row, _)| row < lines);
        self.body = body;
        self.bytes = bytes;
        self.blocks = blocks;
        self.continuation = markdown::Continuation::Block;
        self.open = None;
    }
}

impl Layout {
    fn release(&mut self, live: bool) -> bool {
        if !live {
            self.stable = None;
        }
        let released = self.lines.take().is_some();
        if released {
            self.highlights.shrink_to_fit();
            self.formatted = Vec::new();
        }
        released
    }
}

type Form = (ToolBody, u16, DisplayMode);

struct Shown {
    part: Part,
    form: Form,
    language: String,
    text: String,
    rows: Vec<Row>,
}

#[derive(Default)]
struct Formatted {
    cached: Vec<Shown>,
    used: Vec<Shown>,
}

impl Formatted {
    fn get(
        &mut self,
        part: &Part,
        form: Form,
        language: &str,
        text: &str,
        render: impl FnOnce() -> Vec<Row>,
    ) -> Vec<Row> {
        let found = self.cached.iter().position(|shown| {
            shown.part == *part
                && shown.form == form
                && shown.language == language
                && shown.text == text
        });
        let shown = match found {
            Some(index) => self.cached.swap_remove(index),
            None => Shown {
                part: part.clone(),
                form,
                language: language.to_owned(),
                text: text.to_owned(),
                rows: render(),
            },
        };
        let rows = shown.rows.clone();
        self.used.push(shown);
        rows
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Part {
    Text,
    Question,
    Header(usize),
    Input(usize),
    Output(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Extent {
    Whole,
    Bytes(Range<usize>),
    Line(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Source {
    part: Part,
    extent: Extent,
}

type Row = (Line<'static>, Option<Source>);

const fn whole(part: Part) -> Option<Source> {
    Some(Source {
        part,
        extent: Extent::Whole,
    })
}

#[derive(Clone, Default)]
struct Output {
    lines: Vec<Line<'static>>,
    sources: Vec<Option<Source>>,
    clocks: Vec<(usize, usize, Instant)>,
    links: Vec<(usize, markdown::LinkSpan)>,
}

impl Output {
    fn push(&mut self, line: Line<'static>, source: Option<Source>) {
        self.lines.push(line);
        self.sources.push(source);
    }

    fn push_linked(
        &mut self,
        line: Line<'static>,
        source: Option<Source>,
        links: Vec<markdown::LinkSpan>,
    ) {
        let row = self.lines.len();
        self.links.extend(links.into_iter().map(|link| (row, link)));
        self.push(line, source);
    }

    fn prefixed(&mut self, body: Vec<Row>, first: Span<'static>, rest: Span<'static>) {
        for (index, (line, source)) in body.into_iter().enumerate() {
            let prefix = if index == 0 {
                first.clone()
            } else {
                rest.clone()
            };
            self.push(prefixed(line, prefix), source);
        }
    }
}

fn prefixed(line: Line<'static>, prefix: Span<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(prefix);
    spans.extend(line.spans);
    Line::from(spans).style(line.style)
}

fn spinners(lines: &[Line<'_>], offset: usize) -> impl Iterator<Item = (usize, usize)> {
    lines.iter().enumerate().flat_map(move |(row, line)| {
        line.spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.content == SPINNER_SLOT)
            .map(move |(span, _)| (offset + row, span))
    })
}

const fn text_source(bytes: Range<usize>) -> Option<Source> {
    Some(Source {
        part: Part::Text,
        extent: Extent::Bytes(bytes),
    })
}

fn plain(text: &str, width: u16, style: Style, preserve_whitespace: bool, part: &Part) -> Vec<Row> {
    plain_from(text, width, style, preserve_whitespace, part, 0)
}

fn plain_from(
    text: &str,
    width: u16,
    style: Style,
    preserve_whitespace: bool,
    part: &Part,
    first: usize,
) -> Vec<Row> {
    markdown::wrap_plain_rows(text, width, style, preserve_whitespace, |line, index| {
        (
            line,
            Some(Source {
                part: part.clone(),
                extent: Extent::Line(first + index),
            }),
        )
    })
}

fn rendered(layout: markdown::Layout, part: &Part) -> Vec<Row> {
    layout
        .lines
        .into_iter()
        .zip(layout.sources)
        .map(|(line, range)| {
            (
                line,
                range.map(|range| Source {
                    part: part.clone(),
                    extent: Extent::Bytes(range),
                }),
            )
        })
        .collect()
}

struct RenderContext<'a> {
    width: u16,
    theme: &'a Theme,
    mode: DisplayMode,
    reasoning: bool,
    preview: usize,
    diff_preview: usize,
}

impl RenderContext<'_> {
    fn stream(
        &self,
        kind: &CellKind,
        index: usize,
        stable: Option<Stable>,
        highlights: &mut markdown::Highlights,
    ) -> Option<(Rows, Option<Stable>)> {
        let theme = self.theme;
        let thinking = Style::default()
            .fg(theme.muted())
            .add_modifier(Modifier::ITALIC);
        let bar = Style::default().fg(theme.thinking_medium());
        let (text, plain, first, rest) = match kind {
            CellKind::Assistant(text) => (
                text,
                None,
                Span::styled("● ", Style::default().fg(theme.text())),
                Span::raw("  "),
            ),
            CellKind::Reasoning(text) if self.reasoning => (
                text,
                Some(thinking),
                Span::styled("∴ ", thinking),
                Span::raw("  "),
            ),
            CellKind::Btw { answer, .. } => (
                answer,
                None,
                Span::styled("┃ ", bar),
                Span::styled("┃ ", bar),
            ),
            _ => return None,
        };
        let width = self.width.saturating_sub(2);
        let contained =
            |stable: &Stable| plain.is_some() || markdown::self_contained(&text[stable.bytes..]);
        let mut stable = stable
            .filter(contained)
            .unwrap_or_else(|| self.frame(kind, index, text));
        let commit = width > 0 && contained(&stable);
        let mut group = None;
        let mut block = None;
        let mut links = Vec::new();
        let (body, boundary) = match plain {
            None => {
                let render = |stable: &Stable, highlights: &mut markdown::Highlights| {
                    markdown::render_from(
                        text,
                        stable.bytes,
                        stable.blocks,
                        stable.continuation,
                        width,
                        theme,
                        highlights,
                    )
                };
                let mut rendered_text = render(&stable, highlights);
                if !rendered_text.resumed {
                    stable.rewind();
                    rendered_text = render(&stable, highlights);
                }
                group = rendered_text.group;
                block = rendered_text.block;
                links = std::mem::take(&mut rendered_text.layout.links);
                (
                    rendered(rendered_text.layout, &Part::Text),
                    rendered_text.boundary.filter(|_| commit),
                )
            }
            Some(style) => {
                let tail = text[stable.bytes..].trim_end();
                let complete = memchr::memrchr(b'\n', tail.as_bytes()).filter(|&end| {
                    commit && memchr::memchr2(0x1b, b'\r', &tail.as_bytes()[..end]).is_none()
                });
                match complete {
                    Some(end) => {
                        let lines = memchr::memchr_iter(b'\n', &tail.as_bytes()[..end]).count() + 1;
                        let mut rows = plain_from(
                            &tail[..end],
                            width,
                            style,
                            false,
                            &Part::Text,
                            stable.blocks,
                        );
                        let boundary = markdown::Checkpoint {
                            bytes: stable.bytes + end + 1,
                            lines: rows.len(),
                            blocks: stable.blocks + lines,
                            continuation: markdown::Continuation::Block,
                            group: None,
                        };
                        rows.extend(plain_from(
                            &tail[end + 1..],
                            width,
                            style,
                            false,
                            &Part::Text,
                            boundary.blocks,
                        ));
                        (rows, Some(boundary))
                    }
                    None => (
                        plain_from(tail, width, style, false, &Part::Text, stable.blocks),
                        None,
                    ),
                }
            }
        };
        let opening = stable.body == 0;
        let mut links = links.into_iter();
        let mut body = body.into_iter().enumerate().map(|(row, (line, source))| {
            let prefix = if opening && row == 0 {
                first.clone()
            } else {
                rest.clone()
            };
            let shift = u16::try_from(prefix.width()).unwrap_or(u16::MAX);
            let mut links = links.next().unwrap_or_default();
            for link in &mut links {
                link.start = link.start.saturating_add(shift);
                link.end = link.end.saturating_add(shift);
            }
            (prefixed(line, prefix), source, links)
        });
        let open = stable.open;
        let mut committed = 0;
        if let Some(boundary) = boundary {
            let head = Rc::make_mut(&mut stable.rows);
            let before = head.lines.len();
            for (line, source, links) in body.by_ref().take(boundary.lines) {
                head.push_linked(line, source, links);
            }
            if let (Some((row, start)), Some((rows, end))) = (stable.open, group)
                && boundary.lines >= rows
            {
                head.sources[row..before + rows].fill(text_source(start..end));
                stable.open = None;
            }
            stable.open = match boundary.group {
                Some((0, _)) if stable.open.is_some() => stable.open,
                Some((line, source)) => Some((before + line, source)),
                None => None,
            };
            stable
                .spinners
                .extend(spinners(&head.lines[before..], before));
            stable.body += boundary.lines;
            stable.bytes = boundary.bytes;
            stable.blocks = boundary.blocks;
            stable.continuation = boundary.continuation;
            committed = boundary.lines;
            let rewind = match boundary.continuation {
                markdown::Continuation::Block => Some(boundary),
                _ => block.filter(|block| block.lines <= boundary.lines),
            };
            if let Some(rewind) = rewind {
                stable.fallback = (
                    before + rewind.lines,
                    stable.body - boundary.lines + rewind.lines,
                    rewind.bytes,
                    rewind.blocks,
                );
            }
        }
        let mut tail = Output::default();
        for (line, source, links) in body {
            tail.push_linked(line, source, links);
        }
        let offset = stable.rows.lines.len();
        let rows = Rows {
            head: Rc::clone(&stable.rows),
            spinners: stable
                .spinners
                .iter()
                .copied()
                .chain(spinners(&tail.lines, offset))
                .collect(),
            tail: Rc::new(tail),
            group: stable.open.filter(|_| stable.open == open).zip(group).map(
                |((row, start), (rows, end))| {
                    (row..offset + rows - committed, text_source(start..end))
                },
            ),
        };
        Some((rows, (stable.body > 0).then_some(stable)))
    }

    fn frame(&self, kind: &CellKind, index: usize, text: &str) -> Stable {
        let mut rows = Output::default();
        if index > 0 {
            rows.push(Line::default(), None);
        }
        if let CellKind::Btw { question, done, .. } = kind {
            rows.push(self.btw_header(question, *done), whole(Part::Question));
        }
        let bytes = match kind {
            CellKind::Reasoning(_) => text.len() - text.trim_start().len(),
            _ => 0,
        };
        let lines = rows.lines.len();
        Stable {
            spinners: spinners(&rows.lines, 0).collect(),
            rows: Rc::new(rows),
            body: 0,
            bytes,
            blocks: 0,
            continuation: markdown::Continuation::Block,
            open: None,
            fallback: (lines, 0, bytes, 0),
        }
    }

    fn btw_header(&self, question: &str, done: bool) -> Line<'static> {
        let bar = Style::default().fg(self.theme.thinking_medium());
        let mut header = vec![
            Span::styled("┃ ", bar),
            Span::styled("btw", bar.add_modifier(Modifier::BOLD)),
        ];
        if !done {
            header.push(Span::raw(" "));
            header.push(Span::styled(SPINNER_SLOT, bar.add_modifier(Modifier::BOLD)));
        }
        header.push(Span::styled(
            format!("  {}", sanitize_terminal_text_inline(question)),
            Style::default().fg(self.theme.muted()),
        ));
        Line::from(header)
    }

    fn render(&self, kind: &CellKind, out: &mut Output, formatted: &mut Formatted) {
        let theme = self.theme;
        match kind {
            CellKind::User(text) => {
                let body = plain(
                    text,
                    self.width.saturating_sub(2),
                    Style::default()
                        .fg(theme.text())
                        .add_modifier(Modifier::BOLD),
                    true,
                    &Part::Text,
                );
                out.prefixed(
                    body,
                    Span::styled(
                        "› ",
                        Style::default()
                            .fg(theme.accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                );
            }
            CellKind::Said(said) => self.said(said, out),
            CellKind::Assistant(_) | CellKind::Btw { .. } => {}
            CellKind::Reasoning(text) => {
                let style = Style::default()
                    .fg(theme.muted())
                    .add_modifier(Modifier::ITALIC);
                let summary =
                    sanitize_terminal_text_inline(text.trim().lines().next().unwrap_or_default())
                        .into_owned();
                let summary = summary.trim_matches('*').trim().to_owned();
                let mut collapsed = vec![Span::styled("∴ thinking", style)];
                if !summary.is_empty() {
                    collapsed.push(Span::styled(format!(" · {summary}"), style));
                }
                collapsed.push(Span::styled(
                    "  (ctrl+o)",
                    Style::default().fg(theme.border()),
                ));
                let line = Line::from(collapsed);
                for line in markdown::wrap_spans(&line.spans, self.width, true)
                    .into_iter()
                    .take(1)
                {
                    out.push(line, whole(Part::Text));
                }
            }
            CellKind::Tool(tool) => self.tool(tool, tool.depth, false, out, &mut 0, formatted),
            CellKind::Notice(level, text) => {
                let (icon, color) = match level {
                    NoticeLevel::Info => ("ℹ ", theme.muted()),
                    NoticeLevel::Warning => ("⚠ ", theme.thinking_high()),
                    NoticeLevel::Error => ("✗ ", theme.thinking_max()),
                };
                let body = plain(
                    text,
                    self.width.saturating_sub(2),
                    Style::default().fg(color),
                    false,
                    &Part::Text,
                );
                out.prefixed(
                    body,
                    Span::styled(icon, Style::default().fg(color)),
                    Span::raw("  "),
                );
            }
            CellKind::Summary(text) => {
                let label = format!(" {} ", sanitize_terminal_text_inline(text));
                let fill = usize::from(self.width).saturating_sub(label.chars().count() + 2);
                out.push(
                    Line::from(vec![
                        Span::styled("──", Style::default().fg(theme.border())),
                        Span::styled(label, Style::default().fg(theme.muted())),
                        Span::styled("─".repeat(fill), Style::default().fg(theme.border())),
                    ]),
                    whole(Part::Text),
                );
            }
        }
    }

    fn said(&self, said: &Said, out: &mut Output) {
        let bar = said.bar.map_or_else(
            || Span::raw("  "),
            |color| Span::styled("▎ ", Style::default().fg(color)),
        );
        let band = said
            .band
            .then(|| Style::default().bg(self.theme.code_background()));
        let width = usize::from(self.width);
        let row = |line: Line<'static>| {
            let mut line = prefixed(line, bar.clone());
            if let Some(band) = band {
                let fill = width.saturating_sub(line.width());
                line.spans.push(Span::raw(" ".repeat(fill)));
                line.style = line.style.patch(band);
            }
            line
        };
        // A message that continues a group joins it across the blank row between them.
        if said.header.is_none()
            && let Some(gap) = out.lines.last_mut()
        {
            *gap = row(Line::default());
        }
        let inner = self.width.saturating_sub(2);
        if let Some(header) = &said.header {
            for line in markdown::wrap_spans(&header.spans, inner, true)
                .into_iter()
                .take(1)
            {
                out.push(row(line), whole(Part::Question));
            }
        }
        let mut layout = markdown::render_chat(&said.text, inner, self.theme);
        let links = std::mem::take(&mut layout.links);
        for ((line, source), mut links) in rendered(layout, &Part::Text)
            .into_iter()
            .zip(links.into_iter().chain(std::iter::repeat_with(Vec::new)))
        {
            for link in &mut links {
                link.start = link.start.saturating_add(2);
                link.end = link.end.saturating_add(2);
            }
            out.push_linked(row(line), source, links);
        }
    }

    fn tool(
        &self,
        tool: &ToolCell,
        depth: usize,
        nested: bool,
        out: &mut Output,
        ordinal: &mut usize,
        formatted: &mut Formatted,
    ) {
        let id = *ordinal;
        *ordinal += 1;
        let theme = self.theme;
        let indent = "  ".repeat(depth);
        let (icon, color) = match tool.status {
            ToolStatus::Running => (SPINNER_SLOT, theme.accent()),
            ToolStatus::Completed if tool.unsuccessful() => ("✗", theme.thinking_max()),
            ToolStatus::Completed => ("✓", theme.thinking_low()),
            ToolStatus::Failed => ("✗", theme.thinking_max()),
            ToolStatus::Cancelled => ("⊘", theme.muted()),
            ToolStatus::Yielded => ("↻", theme.accent()),
        };
        let muted = Style::default().fg(theme.muted());
        let mut header = vec![
            Span::raw(indent.clone()),
            Span::styled(icon, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(
                sanitize_terminal_text_inline(&tool.name).into_owned(),
                Style::default()
                    .fg(theme.text())
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if !tool.title.is_empty() {
            header.push(Span::styled(
                format!(" {}", sanitize_terminal_text_inline(&tool.title)),
                muted,
            ));
        }
        if let Some(duration) = tool.duration {
            header.push(Span::styled(
                format!(
                    " · {}",
                    format_duration(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
                ),
                muted,
            ));
        } else if tool.status == ToolStatus::Running {
            out.clocks
                .push((out.lines.len(), header.len(), tool.started_at));
            header.push(Span::styled("", muted));
        }
        if let Some(code) = tool.exit_code.filter(|code| *code != 0) {
            header.push(Span::styled(
                format!(" · exit {code}"),
                Style::default().fg(theme.thinking_max()),
            ));
        }
        for detail in &tool.details {
            header.push(Span::styled(
                format!(" · {}", sanitize_terminal_text_inline(detail)),
                muted,
            ));
        }
        out.push(Line::from(header), whole(Part::Header(id)));
        let collapsed = self.mode == DisplayMode::Collapsed;
        let inner = self
            .width
            .saturating_sub(u16::try_from(indent.len() + 4).unwrap_or(u16::MAX));
        let gutter = Style::default().fg(theme.border());
        let show_input = !nested || !collapsed || tool.input_body == ToolBody::Diff;
        if show_input && !tool.input.trim().is_empty() {
            let part = Part::Input(id);
            let form = (tool.input_body, inner, self.mode);
            let shown = formatted.get(&part, form, &tool.language, &tool.input, || {
                let input = match (tool.input_body, self.mode) {
                    (ToolBody::Code, DisplayMode::Collapsed | DisplayMode::Beautified)
                        if matches!(tool.language.as_str(), "js" | "javascript") =>
                    {
                        Cow::Owned(beautify(&tool.input, "  "))
                    }
                    _ => Cow::Borrowed(tool.input.as_str()),
                };
                let limit = self.limit(tool.input_body);
                let rows = if collapsed { limit } else { usize::MAX };
                let (mut body, total) =
                    self.body(&input, tool.input_body, &tool.language, inner, &part, rows);
                if matches!(input, Cow::Owned(_)) {
                    for (_, source) in &mut body {
                        *source = whole(part.clone());
                    }
                }
                self.preview(body, total, &part, limit, false)
            });
            out.prefixed(
                shown,
                Span::styled(format!("{indent}  │ "), gutter),
                Span::styled(format!("{indent}  │ "), gutter),
            );
        }
        for child in &tool.children {
            self.tool(child, depth + 1, true, out, ordinal, formatted);
        }
        let output = match self.mode {
            DisplayMode::Raw if !tool.raw_output.is_empty() => tool.raw_output.as_str(),
            _ => tool.output.as_str(),
        };
        let show_output = !nested || !collapsed || tool.unsuccessful();
        if show_output && !output.trim().is_empty() {
            let (body, language) = if self.mode == DisplayMode::Raw {
                (ToolBody::Text, "")
            } else {
                (tool.output_body, tool.language.as_str())
            };
            let part = Part::Output(id);
            let form = (body, inner, self.mode);
            let shown = formatted.get(&part, form, language, output, || {
                let (rows, total) = self.body(output, body, language, inner, &part, usize::MAX);
                self.preview(rows, total, &part, self.limit(tool.output_body), true)
            });
            out.prefixed(
                shown,
                Span::styled(format!("{indent}  └ "), gutter),
                Span::styled(format!("{indent}    "), gutter),
            );
        }
    }

    fn body(
        &self,
        text: &str,
        body: ToolBody,
        language: &str,
        width: u16,
        part: &Part,
        rows: usize,
    ) -> (Vec<Row>, usize) {
        let _span = tracing::info_span!(
            "tui.tool.format",
            text.bytes = text.len(),
            ?body,
            width,
            mode = self.mode.label()
        )
        .entered();
        let text = sanitize_terminal_text(text.trim_end());
        match body {
            ToolBody::Diff => {
                let (lines, total) = diff::render_rows(&text, width, self.theme, rows);
                let lines = lines
                    .into_iter()
                    .map(|line| (line, whole(part.clone())))
                    .collect();
                (lines, total)
            }
            ToolBody::Code => {
                let (lines, total) = code_lines(&text, language, width, rows);
                let lines = lines
                    .into_iter()
                    .map(|(line, index)| {
                        (
                            line,
                            Some(Source {
                                part: part.clone(),
                                extent: Extent::Line(index),
                            }),
                        )
                    })
                    .collect();
                (lines, total)
            }
            ToolBody::Text => {
                let lines = plain(
                    &text,
                    width,
                    Style::default().fg(self.theme.muted()),
                    true,
                    part,
                );
                let total = lines.len();
                (lines, total)
            }
        }
    }

    const fn limit(&self, body: ToolBody) -> usize {
        match body {
            ToolBody::Diff => self.diff_preview,
            ToolBody::Text | ToolBody::Code => self.preview,
        }
    }

    fn preview(
        &self,
        body: Vec<Row>,
        total: usize,
        part: &Part,
        preview: usize,
        tail: bool,
    ) -> Vec<Row> {
        let limit = if self.mode == DisplayMode::Collapsed {
            preview.min(total)
        } else {
            total
        };
        let hidden = total - limit;
        let mut shown: Vec<Row> = if tail {
            let skipped = body.len() - limit;
            body.into_iter().skip(skipped).collect()
        } else {
            body.into_iter().take(limit).collect()
        };
        if hidden > 0 {
            let hint = (
                Line::styled(
                    format!("… {hidden} more lines (ctrl+o to expand)"),
                    Style::default()
                        .fg(self.theme.muted())
                        .add_modifier(Modifier::ITALIC),
                ),
                whole(part.clone()),
            );
            if tail {
                shown.insert(0, hint);
            } else {
                shown.push(hint);
            }
        }
        shown
    }
}

fn code_lines(
    text: &str,
    language: &str,
    width: u16,
    rows: usize,
) -> (Vec<(Line<'static>, usize)>, usize) {
    let _span = tracing::info_span!(
        "tui.code.highlight",
        text.bytes = text.len(),
        width,
        language
    )
    .entered();
    let assets = highlight::assets();
    let syntax = highlight::syntax_for_token(&assets.syntaxes, language);
    let mut highlighter = HighlightLines::new(syntax, highlight::theme());
    let mut lines = Vec::new();
    let mut total = 0;
    for (index, line) in text.lines().enumerate() {
        if lines.len() >= rows {
            total += highlight::wrap_spans(&[Span::raw(line)], width).len();
            continue;
        }
        for spans in highlight::wrap(
            highlight::line(&mut highlighter, line, &assets.syntaxes),
            width,
        ) {
            total += 1;
            if lines.len() < rows {
                lines.push((Line::from(spans), index));
            }
        }
    }
    (lines, total)
}

fn render_line(line: &Line<'_>, area: Rect, buffer: &mut Buffer, mut at: impl FnMut(usize, u16)) {
    if line
        .alignment
        .is_some_and(|alignment| alignment != Alignment::Left)
    {
        Paragraph::new(line.clone()).render(area, buffer);
        return;
    }
    let start = buffer.index_of(area.x, area.y);
    let cells = &mut buffer.content[start..start + usize::from(area.width)];
    let mut used = 0;
    for (index, span) in line.spans.iter().enumerate() {
        if let Ok(column) = u16::try_from(used) {
            at(index, area.x.saturating_add(column));
        }
        let style = line.style.patch(span.style);
        let content = span.content.as_ref();
        if content.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
            for index in 0..content.len() {
                let Some(cell) = cells.get_mut(used) else {
                    return;
                };
                cell.set_symbol(&content[index..=index]).set_style(style);
                used += 1;
            }
            continue;
        }
        for grapheme in content
            .graphemes(true)
            .filter(|grapheme| !grapheme.contains(char::is_control))
        {
            let width = usize::from(grapheme.cell_width());
            if width > cells.len() {
                continue;
            }
            if used + width > cells.len() {
                return;
            }
            if width > 0 {
                cells[used].set_symbol(grapheme).set_style(style);
            }
            used += width;
        }
    }
}

struct Selected<'a> {
    part: &'a Part,
    whole: bool,
    bytes: Option<Range<usize>>,
    lines: Option<(usize, usize)>,
}

fn raw_rows<'a>(
    kind: &CellKind,
    mode: DisplayMode,
    sources: impl Iterator<Item = &'a Option<Source>>,
) -> String {
    let mut selected = Vec::<Selected<'_>>::new();
    for source in sources.flatten() {
        if selected
            .last()
            .is_none_or(|selected| *selected.part != source.part)
        {
            selected.push(Selected {
                part: &source.part,
                whole: false,
                bytes: None,
                lines: None,
            });
        }
        let Some(current) = selected.last_mut() else {
            continue;
        };
        match &source.extent {
            Extent::Whole => current.whole = true,
            Extent::Bytes(range) => {
                current.bytes = Some(current.bytes.as_ref().map_or(range.clone(), |bytes| {
                    bytes.start.min(range.start)..bytes.end.max(range.end)
                }));
            }
            Extent::Line(line) => {
                current.lines = Some(current.lines.map_or((*line, *line), |(first, last)| {
                    (first.min(*line), last.max(*line))
                }));
            }
        }
    }
    selected
        .into_iter()
        .filter_map(|selected| {
            let text = part_text(kind, selected.part, mode)?;
            if selected.whole {
                return Some(strip_terminal_controls(text.trim_end()).into_owned());
            }
            let range = [
                selected.bytes.map(|bytes| lines_around(&text, bytes)),
                selected.lines.and_then(|(first, last)| {
                    Some(line_range(&text, first)?.start..line_range(&text, last)?.end)
                }),
            ]
            .into_iter()
            .flatten()
            .reduce(|left, right| left.start.min(right.start)..left.end.max(right.end))?;
            Some(strip_terminal_controls(&text[range]).into_owned())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn selected_text(
    rows: &Rows,
    first: usize,
    last: usize,
    kind: &CellKind,
    mode: DisplayMode,
) -> String {
    let source = |row: usize| rows.source(row).and_then(Option::as_ref);
    let top = source(first).filter(|&top| first.checked_sub(1).and_then(source) == Some(top));
    let bottom = source(last).filter(|&bottom| source(last + 1) == Some(bottom));
    let head = top.map_or(0, |top| {
        (first..=last)
            .take_while(|&row| source(row) == Some(top))
            .count()
    });
    let tail = bottom.map_or(0, |bottom| {
        (first + head..=last)
            .rev()
            .take_while(|&row| source(row) == Some(bottom))
            .count()
    });
    let middle = first + head..last + 1 - tail;
    let blank = |row: &usize| source(*row).is_none();
    let leading = middle.clone().take_while(blank).count();
    let trailing = middle.clone().skip(leading).rev().take_while(blank).count();
    let raw = raw_rows(kind, mode, middle.filter_map(|row| rows.source(row)));
    let mut lines = Vec::new();
    if head > 0 {
        lines.push(shown_rows(rows, first..first + head));
    }
    lines.extend(std::iter::repeat_n(String::new(), leading));
    if !raw.is_empty() {
        lines.push(raw);
    }
    lines.extend(std::iter::repeat_n(String::new(), trailing));
    if tail > 0 {
        lines.push(shown_rows(rows, last + 1 - tail..last + 1));
    }
    lines.join("\n").trim_matches('\n').to_owned()
}

fn shown_rows(rows: &Rows, range: Range<usize>) -> String {
    range
        .map(|row| {
            let mut text = content(rows.line(row));
            text.truncate(text.trim_end().len());
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn content(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .skip(1)
        .map(|span| span.content.as_ref())
        .collect()
}

fn url_at(rows: &Rows, row: usize, column: u16, width: u16) -> Option<String> {
    let line = rows.line(row);
    let mut at = 0;
    let cells = line
        .spans
        .iter()
        .flat_map(|span| span.content.graphemes(true))
        .map(|grapheme| {
            let cell = (at, grapheme);
            at += grapheme.cell_width();
            cell
        })
        .collect::<Vec<(u16, &str)>>();
    let url = |grapheme: &str| grapheme.bytes().all(url_byte);
    let index = cells.iter().position(|&(start, grapheme)| {
        url(grapheme) && (start..start + grapheme.cell_width()).contains(&column)
    })?;
    let start = cells[..index]
        .iter()
        .rposition(|&(_, grapheme)| !url(grapheme))
        .map_or(0, |index| index + 1);
    let end = cells[index..]
        .iter()
        .position(|&(_, grapheme)| !url(grapheme))
        .map_or(cells.len(), |end| index + end);
    let mut token = cells[start..end]
        .iter()
        .map(|&(_, grapheme)| grapheme)
        .collect::<String>();
    let mut offset = cells[start..index]
        .iter()
        .map(|&(_, grapheme)| grapheme.len())
        .sum::<usize>();
    let full = |row: usize| rows.line(row).width() >= usize::from(width);
    let wrapped = |above: usize, below: usize| {
        full(above)
            && rows
                .source(above)
                .and_then(Option::as_ref)
                .is_some_and(|source| rows.source(below).and_then(Option::as_ref) == Some(source))
    };
    let prefix = line.spans.first().map_or(0, Span::width);
    let (mut above, mut open) = (row, usize::from(cells[start].0) == prefix);
    while open && above > 0 && wrapped(above - 1, above) {
        above -= 1;
        let text = content(rows.line(above));
        let length = text
            .bytes()
            .rev()
            .position(|byte| !url_byte(byte))
            .unwrap_or(text.len());
        token.insert_str(0, &text[text.len() - length..]);
        offset += length;
        open = length == text.len();
    }
    let (mut below, mut open) = (row, end == cells.len());
    while open && wrapped(below, below + 1) {
        below += 1;
        let text = content(rows.line(below));
        let length = text
            .bytes()
            .position(|byte| !url_byte(byte))
            .unwrap_or(text.len());
        token.push_str(&text[..length]);
        open = length == text.len();
    }
    let start = ["https://", "http://"]
        .into_iter()
        .filter_map(|scheme| token[..token.len().min(offset + scheme.len())].rfind(scheme))
        .max()?;
    let url = trim_url(&token[start..]);
    let host = url.find("://")? + 3;
    (host < url.len() && offset < start + url.len()).then(|| url.to_owned())
}

const fn url_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
        && !matches!(
            byte,
            b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
        )
}

fn trim_url(mut url: &str) -> &str {
    loop {
        let count = |byte| url.bytes().filter(|&other| other == byte).count();
        let trailing = match url.as_bytes().last() {
            Some(b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'*') => true,
            Some(b')') => count(b'(') < count(b')'),
            Some(b']') => count(b'[') < count(b']'),
            _ => false,
        };
        if !trailing {
            return url;
        }
        url = &url[..url.len() - 1];
    }
}

fn part_text<'a>(kind: &'a CellKind, part: &Part, mode: DisplayMode) -> Option<Cow<'a, str>> {
    match (kind, part) {
        (
            CellKind::User(text)
            | CellKind::Assistant(text)
            | CellKind::Notice(_, text)
            | CellKind::Summary(text)
            | CellKind::Btw { answer: text, .. },
            Part::Text,
        ) => Some(Cow::Borrowed(text)),
        (CellKind::Reasoning(text), Part::Text) => Some(Cow::Borrowed(text.trim())),
        (CellKind::Said(said), Part::Text) => Some(Cow::Borrowed(&said.text)),
        (CellKind::Said(said), Part::Question) => said.header.as_ref().map(|header| {
            Cow::Owned(
                header
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect(),
            )
        }),
        (CellKind::Btw { question, .. }, Part::Question) => Some(Cow::Borrowed(question)),
        (CellKind::Tool(tool), Part::Header(index)) => nth_tool(tool, *index).map(|tool| {
            if tool.title.is_empty() {
                Cow::Borrowed(tool.name.as_str())
            } else {
                Cow::Owned(format!("{} {}", tool.name, tool.title))
            }
        }),
        (CellKind::Tool(tool), Part::Input(index)) => {
            nth_tool(tool, *index).map(|tool| Cow::Borrowed(tool.input.as_str()))
        }
        (CellKind::Tool(tool), Part::Output(index)) => nth_tool(tool, *index).map(|tool| {
            Cow::Borrowed(if mode == DisplayMode::Raw && !tool.raw_output.is_empty() {
                tool.raw_output.as_str()
            } else {
                tool.output.as_str()
            })
        }),
        _ => None,
    }
}

fn nth_tool(tool: &ToolCell, index: usize) -> Option<&ToolCell> {
    fn walk<'a>(tool: &'a ToolCell, remaining: &mut usize) -> Option<&'a ToolCell> {
        if *remaining == 0 {
            return Some(tool);
        }
        *remaining -= 1;
        tool.children
            .iter()
            .find_map(|child| walk(child, remaining))
    }
    walk(tool, &mut { index })
}

fn lines_around(text: &str, range: Range<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = range.end.min(text.len());
    let start = range.start.min(end);
    while end > start && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let first = memchr::memrchr2(b'\n', b'\r', &bytes[..start]).map_or(0, |index| index + 1);
    let last = memchr::memchr2(b'\n', b'\r', &bytes[end..]).map_or(text.len(), |index| end + index);
    first..last
}

fn line_range(text: &str, line: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = 0;
    for _ in 0..line {
        start += memchr::memchr2(b'\n', b'\r', &bytes[start..])? + 1;
        if bytes[start - 1] == b'\r' && bytes.get(start) == Some(&b'\n') {
            start += 1;
        }
    }
    let end =
        memchr::memchr2(b'\n', b'\r', &bytes[start..]).map_or(text.len(), |index| start + index);
    Some(start..end)
}
