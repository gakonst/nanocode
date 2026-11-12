use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use arc_swap::ArcSwapOption;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{format::normalize_line_endings, history::History, theme::Theme};

const PROMPT: &str = "› ";
const CONTINUATION: &str = "  ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Ignored,
    Edited,
    Moved,
    Submit(String),
}

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
    history: History,
    placeholder: String,
    preferred_column: Option<usize>,
    layout: ArcSwapOption<(usize, Arc<[Row]>)>,
}

#[derive(Clone, Debug)]
struct Row {
    start: usize,
    end: usize,
}

impl Clone for Composer {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            cursor: self.cursor,
            history: self.history.clone(),
            placeholder: self.placeholder.clone(),
            preferred_column: self.preferred_column,
            layout: ArcSwapOption::new(self.layout.load_full()),
        }
    }
}

impl Composer {
    #[must_use]
    pub fn new(placeholder: impl Into<String>, history: History) -> Self {
        Self {
            placeholder: placeholder.into(),
            history,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[must_use]
    pub const fn history(&self) -> &History {
        &self.history
    }

    pub const fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<String>) {
        self.placeholder = placeholder.into();
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.layout.store(None);
        self.text = text.into();
        self.cursor = self.text.len();
        self.preferred_column = None;
    }

    pub fn take(&mut self) -> String {
        self.layout.store(None);
        self.cursor = 0;
        self.preferred_column = None;
        self.history.reset();
        std::mem::take(&mut self.text)
    }

    pub fn insert_str(&mut self, text: &str) {
        let text = normalize_line_endings(text);
        self.layout.store(None);
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.preferred_column = None;
    }

    #[must_use]
    pub fn mention(&self, sigil: char) -> Option<(usize, &str)> {
        let before = &self.text[..self.cursor];
        let start = before.rfind(char::is_whitespace).map_or(0, |index| {
            index + before[index..].chars().next().map_or(1, char::len_utf8)
        });
        let token = &before[start..];
        token.strip_prefix(sigil).map(|query| (start, query))
    }

    pub fn replace_before_cursor(&mut self, start: usize, replacement: &str) {
        if start <= self.cursor && self.text.is_char_boundary(start) {
            self.layout.store(None);
            self.text.replace_range(start..self.cursor, replacement);
            self.cursor = start + replacement.len();
            self.preferred_column = None;
        }
    }

    pub fn paste(&mut self, text: &str) -> ComposerAction {
        self.insert_str(text);
        ComposerAction::Edited
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ComposerAction {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Enter if shift || alt => self.edit(|composer| composer.insert_str("\n")),
            KeyCode::Enter => {
                if self.text[..self.cursor].ends_with('\\') {
                    return self.edit(|composer| {
                        composer.cursor -= 1;
                        composer.text.remove(composer.cursor);
                        composer.insert_str("\n");
                    });
                }
                if self.text.trim().is_empty() {
                    return ComposerAction::Ignored;
                }
                let text = self.take();
                self.history.push(text.clone());
                ComposerAction::Submit(text)
            }
            KeyCode::Char('j') if control => self.edit(|composer| composer.insert_str("\n")),
            KeyCode::Char('a') if control => self.moved(Self::line_start),
            KeyCode::Char('e') if control => self.moved(Self::line_end),
            KeyCode::Char('b') if control => self.moved(Self::left),
            KeyCode::Char('f') if control => self.moved(Self::right),
            KeyCode::Char('b') if alt => self.moved(Self::word_left),
            KeyCode::Char('f') if alt => self.moved(Self::word_right),
            KeyCode::Char('u') if control => self.edit(|composer| {
                let start = composer.line_start_offset();
                composer.delete_range(start, composer.cursor);
            }),
            KeyCode::Char('k') if control => self.edit(|composer| {
                let end = composer.line_end_offset();
                let end = if end == composer.cursor && end < composer.text.len() {
                    end + 1
                } else {
                    end
                };
                composer.delete_range(composer.cursor, end);
            }),
            KeyCode::Char('w') if control => self.edit(Self::delete_word_back),
            KeyCode::Backspace if alt || control => self.edit(Self::delete_word_back),
            KeyCode::Char('d') if alt => self.edit(|composer| {
                let end = composer.word_right_offset();
                composer.delete_range(composer.cursor, end);
            }),
            KeyCode::Char(character) if !control && !alt => {
                self.edit(|composer| composer.insert_str(character.encode_utf8(&mut [0; 4])))
            }
            KeyCode::Char(character) if alt && !control => {
                self.edit(|composer| composer.insert_str(character.encode_utf8(&mut [0; 4])))
            }
            KeyCode::Tab if !control && !alt => self.edit(|composer| composer.insert_str("    ")),
            KeyCode::Backspace => {
                if self.cursor == 0 {
                    return ComposerAction::Ignored;
                }
                self.edit(|composer| {
                    let start = composer.previous_boundary(composer.cursor);
                    composer.delete_range(start, composer.cursor);
                })
            }
            KeyCode::Delete => {
                if self.cursor == self.text.len() {
                    return ComposerAction::Ignored;
                }
                self.edit(|composer| {
                    let end = composer.next_boundary(composer.cursor);
                    composer.delete_range(composer.cursor, end);
                })
            }
            KeyCode::Left if control || alt => self.moved(Self::word_left),
            KeyCode::Right if control || alt => self.moved(Self::word_right),
            KeyCode::Left => self.moved(Self::left),
            KeyCode::Right => self.moved(Self::right),
            KeyCode::Home => self.moved(Self::line_start),
            KeyCode::End => self.moved(Self::line_end),
            KeyCode::Up => self.vertical(true),
            KeyCode::Down => self.vertical(false),
            _ => ComposerAction::Ignored,
        }
    }

    fn edit(&mut self, change: impl FnOnce(&mut Self)) -> ComposerAction {
        change(self);
        self.preferred_column = None;
        ComposerAction::Edited
    }

    fn moved(&mut self, movement: impl FnOnce(&mut Self)) -> ComposerAction {
        movement(self);
        self.preferred_column = None;
        ComposerAction::Moved
    }

    fn vertical(&mut self, up: bool) -> ComposerAction {
        let line_start = self.line_start_offset();
        let line_end = self.line_end_offset();
        if up && line_start == 0 || !up && line_end == self.text.len() {
            return self.history_step(up);
        }
        let column = self
            .preferred_column
            .unwrap_or_else(|| self.text[line_start..self.cursor].graphemes(true).count());
        let (target_start, target_end) = if up {
            let end = line_start - 1;
            (
                self.text[..end].rfind('\n').map_or(0, |index| index + 1),
                end,
            )
        } else {
            let start = line_end + 1;
            (
                start,
                self.text[start..]
                    .find('\n')
                    .map_or(self.text.len(), |index| start + index),
            )
        };
        self.cursor = self.text[target_start..target_end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(target_end, |(index, _)| target_start + index);
        self.preferred_column = Some(column);
        ComposerAction::Moved
    }

    fn history_step(&mut self, up: bool) -> ComposerAction {
        if !up && !self.history.is_browsing() {
            if self.cursor == self.text.len() {
                return ComposerAction::Ignored;
            }
            self.cursor = self.text.len();
            return ComposerAction::Moved;
        }
        if up
            && !self.history.is_browsing()
            && self.cursor != 0
            && !self.text.contains('\n')
            && !self.text.is_empty()
        {
            self.cursor = 0;
            return ComposerAction::Moved;
        }
        let entry = if up {
            self.history.older(&self.text).map(str::to_owned)
        } else {
            self.history.newer().map(str::to_owned)
        };
        match entry {
            Some(entry) => {
                self.layout.store(None);
                self.text = entry;
                self.cursor = self.text.len();
                self.preferred_column = None;
                ComposerAction::Edited
            }
            None => ComposerAction::Ignored,
        }
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        if start < end {
            self.layout.store(None);
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
    }

    fn delete_word_back(&mut self) {
        let start = self.word_left_offset();
        self.delete_range(start, self.cursor);
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .graphemes(true)
            .next()
            .map_or(self.text.len(), |grapheme| offset + grapheme.len())
    }

    fn left(&mut self) {
        self.cursor = self.previous_boundary(self.cursor);
    }

    fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    fn line_start_offset(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end_offset(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index)
    }

    fn line_start(&mut self) {
        self.cursor = self.line_start_offset();
    }

    fn line_end(&mut self) {
        self.cursor = self.line_end_offset();
    }

    fn word_left_offset(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|character: char| !is_word(character));
        trimmed
            .rfind(|character: char| !is_word(character))
            .map_or(0, |index| {
                index + trimmed[index..].chars().next().map_or(1, char::len_utf8)
            })
    }

    fn word_right_offset(&self) -> usize {
        let after = &self.text[self.cursor..];
        let skipped = after.len()
            - after
                .trim_start_matches(|character: char| !is_word(character))
                .len();
        let rest = &after[skipped..];
        let word = rest
            .find(|character: char| !is_word(character))
            .unwrap_or(rest.len());
        self.cursor + skipped + word
    }

    fn word_left(&mut self) {
        self.cursor = self.word_left_offset();
    }

    fn word_right(&mut self) {
        self.cursor = self.word_right_offset();
    }

    fn rows(&self, width: usize) -> Arc<[Row]> {
        if let Some(cached) = self.layout.load_full()
            && cached.0 == width
        {
            return Arc::clone(&cached.1);
        }
        let content = width.saturating_sub(PROMPT.width()).max(1);
        let mut rows = Vec::new();
        let mut line_start = 0;
        for line in self.text.split('\n') {
            if crate::text::ascii_prefix(line, false) == line.len() {
                for start in (0..line.len()).step_by(content) {
                    rows.push(Row {
                        start: line_start + start,
                        end: line_start + (start + content).min(line.len()),
                    });
                }
                if line.is_empty() {
                    rows.push(Row {
                        start: line_start,
                        end: line_start,
                    });
                }
                line_start += line.len() + 1;
                continue;
            }
            let mut row_start = line_start;
            let mut row_width = 0;
            for (index, grapheme) in line.grapheme_indices(true) {
                let grapheme_width = grapheme.width();
                if row_width + grapheme_width > content && row_width > 0 {
                    rows.push(Row {
                        start: row_start,
                        end: line_start + index,
                    });
                    row_start = line_start + index;
                    row_width = 0;
                }
                row_width += grapheme_width;
            }
            rows.push(Row {
                start: row_start,
                end: line_start + line.len(),
            });
            line_start += line.len() + 1;
        }
        let rows: Arc<[Row]> = rows.into();
        self.layout
            .store(Some(Arc::new((width, Arc::clone(&rows)))));
        rows
    }

    fn cursor_cell(&self, rows: &[Row]) -> (usize, usize) {
        let cursor = self.cursor;
        rows.iter()
            .enumerate()
            .find(|(index, row)| {
                row.start <= cursor
                    && (cursor < row.end
                        || cursor == row.end
                            && rows.get(index + 1).is_none_or(|next| next.start != cursor))
            })
            .map_or((0, PROMPT.width()), |(index, row)| {
                (index, PROMPT.width() + self.text[row.start..cursor].width())
            })
    }

    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        let _span = tracing::info_span!("tui.composer.layout", width, text.bytes = self.text.len())
            .entered();
        u16::try_from(self.rows(usize::from(width)).len()).unwrap_or(u16::MAX)
    }

    pub fn render(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        active: bool,
    ) -> Option<Position> {
        let _span = tracing::info_span!(
            "tui.composer.render",
            width = area.width,
            height = area.height,
            text.bytes = self.text.len(),
            active
        )
        .entered();
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let prompt_style = Style::default()
            .fg(if active {
                theme.accent()
            } else {
                theme.muted()
            })
            .add_modifier(Modifier::BOLD);
        if self.text.is_empty() {
            Paragraph::new(Line::from(vec![
                Span::styled(PROMPT, prompt_style),
                Span::styled(self.placeholder.clone(), Style::default().fg(theme.muted())),
            ]))
            .render(area, buffer);
            return active.then(|| Position::new(area.x + PROMPT.width() as u16, area.y));
        }
        let rows = self.rows(usize::from(area.width));
        let (cursor_row, cursor_column) = self.cursor_cell(&rows);
        let visible = usize::from(area.height);
        let offset = (cursor_row + 1).saturating_sub(visible);
        let text_style = Style::default().fg(theme.text());
        let lines = rows
            .iter()
            .skip(offset)
            .take(visible)
            .enumerate()
            .map(|(index, row)| {
                let prefix = if index + offset == 0 {
                    Span::styled(PROMPT, prompt_style)
                } else {
                    Span::raw(CONTINUATION)
                };
                Line::from(vec![
                    prefix,
                    Span::styled(self.text[row.start..row.end].to_owned(), text_style),
                ])
            })
            .collect::<Vec<_>>();
        Paragraph::new(lines).render(area, buffer);
        let column = u16::try_from(cursor_column)
            .unwrap_or(u16::MAX)
            .min(area.width - 1);
        let row = u16::try_from(cursor_row - offset).unwrap_or(0);
        active.then(|| Position::new(area.x + column, area.y + row))
    }
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}
