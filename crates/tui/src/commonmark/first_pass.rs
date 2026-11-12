use std::cmp::max;

use super::label::{LinkLabel, scan_link_label_rest};
use super::parse::{
    Allocations, FootnoteUse, Item, ItemBody, LINK_DESTINATION_PARENTHESES_MAX, LinkDefinition,
    scan_containers,
};
use super::scan::*;
use super::text::CowStr;
use super::tree::{Tree, TreeIndex};
use super::{HeadingLevel, Options};

const TREE_CAPACITY_MIN: usize = 128;
const TEXT_BYTES_PER_TREE_NODE: usize = 32;
const TABLE_FILLED_CELLS_MAX: usize = 1 << 18;
const HARD_BREAK_SPACES_MIN: usize = 2;
const STRIKETHROUGH_TILDES_MAX: usize = 2;
const EN_DASH_HYPHENS: usize = 2;
const EM_DASH_HYPHENS: usize = 3;

pub(crate) fn run_first_pass(text: &str, options: Options) -> (Tree<Item>, Allocations<'_>) {
    let start_capacity = max(TREE_CAPACITY_MIN, text.len() / TEXT_BYTES_PER_TREE_NODE);
    let lookup_table = &create_lookup_table(&options);
    let first_pass = FirstPass {
        text,
        tree: Tree::with_capacity(start_capacity),
        begin_list_item: None,
        last_line_blank: false,
        allocations: Allocations::new(),
        options,
        lookup_table,
    };
    first_pass.run()
}

struct FirstPass<'a, 'b> {
    text: &'a str,
    tree: Tree<Item>,
    begin_list_item: Option<usize>,
    last_line_blank: bool,
    allocations: Allocations<'a>,
    options: Options,
    lookup_table: &'b LookupTable,
}

impl<'a, 'b> FirstPass<'a, 'b> {
    fn run(mut self) -> (Tree<Item>, Allocations<'a>) {
        let mut index = 0;
        while index < self.text.len() {
            index = self.parse_block(index);
        }
        while self.tree.spine_length() > 0 {
            self.pop(index);
        }
        (self.tree, self.allocations)
    }

    fn parse_block(&mut self, mut start_index: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut line_start = LineStart::new(&bytes[start_index..]);

        let cursor = scan_containers(&self.tree, &mut line_start, self.options);
        for _ in cursor..self.tree.spine_length() {
            self.pop(start_index);
        }

        loop {
            let save = line_start.clone();
            let outer_indent = line_start.scan_space_up_to(CODE_INDENT_COLUMNS);
            if outer_indent >= CODE_INDENT_COLUMNS {
                line_start = save;
                break;
            }
            if self.options.contains(Options::ENABLE_FOOTNOTES) {
                let container_start = start_index + line_start.bytes_scanned();
                if let Some(byte_count) = self.parse_footnote(container_start) {
                    start_index = container_start + byte_count;
                    line_start = LineStart::new(&bytes[start_index..]);
                    continue;
                }
            }
            let container_start = start_index + line_start.bytes_scanned();
            if let Some((marker, start_number, indent)) =
                line_start.scan_list_marker_with_indent(outer_indent)
            {
                let after_marker_index = start_index + line_start.bytes_scanned();
                self.continue_list(container_start - outer_indent, marker, start_number);
                self.tree.append(Item {
                    start: container_start - outer_indent,
                    end: after_marker_index,
                    body: ItemBody::ListItem(indent),
                });
                self.tree.push();
                if let Some(count) = scan_blank_line(&bytes[after_marker_index..]) {
                    self.begin_list_item = Some(after_marker_index + count);
                    return after_marker_index + count;
                }
                if self.options.contains(Options::ENABLE_TASKLISTS) {
                    let task_list_marker =
                        line_start.scan_task_list_marker().map(|is_checked| Item {
                            start: after_marker_index,
                            end: start_index + line_start.bytes_scanned(),
                            body: ItemBody::TaskListMarker(is_checked),
                        });
                    if let Some(task_list_marker) = task_list_marker {
                        if let Some(count) = scan_blank_line(&bytes[task_list_marker.end..]) {
                            self.tree.append(task_list_marker);
                            self.begin_list_item = Some(task_list_marker.end + count);
                            return task_list_marker.end + count;
                        } else {
                            line_start.scan_all_space();
                            let index = start_index + line_start.bytes_scanned();
                            return self.parse_paragraph(index, Some(task_list_marker));
                        }
                    }
                }
            } else if line_start.scan_blockquote_marker() {
                let kind = if self.options.contains(Options::ENABLE_GFM) {
                    line_start.scan_blockquote_tag()
                } else {
                    None
                };
                self.finish_list(start_index);
                self.tree.append(Item {
                    start: container_start,
                    end: 0,
                    body: ItemBody::BlockQuote(kind),
                });
                self.tree.push();
                if kind.is_some() {
                    let index = start_index + line_start.bytes_scanned();
                    let mut lazy_line_start = LineStart::new(&bytes[index..]);
                    let tree_position =
                        scan_containers(&self.tree, &mut lazy_line_start, self.options);
                    let current_container = tree_position == self.tree.spine_length();
                    if !lazy_line_start.scan_space(CODE_INDENT_COLUMNS)
                        && self.scan_paragraph_interrupt(
                            &bytes[index + lazy_line_start.bytes_scanned()..],
                            current_container,
                        )
                    {
                        return index;
                    } else {
                        line_start = lazy_line_start;
                        line_start.scan_all_space();
                        start_index = index;
                        break;
                    }
                }
            } else {
                line_start = save;
                break;
            }
        }

        let index = start_index + line_start.bytes_scanned();

        if let Some(count) = scan_blank_line(&bytes[index..]) {
            if let Some(node_index) = self.tree.peek_up() {
                match &mut self.tree[node_index].item.body {
                    ItemBody::BlockQuote(..) => (),
                    ItemBody::ListItem(indent) if self.begin_list_item.is_some() => {
                        self.last_line_blank = true;

                        *indent = 0;
                    }
                    _ => {
                        self.last_line_blank = true;
                    }
                }
            } else {
                self.last_line_blank = true;
            }
            return index + count;
        }

        let remaining_space = line_start.remaining_space();

        let indent = line_start.scan_space_up_to(CODE_INDENT_COLUMNS);
        if indent == CODE_INDENT_COLUMNS {
            self.finish_list(start_index);
            let index = start_index + line_start.bytes_scanned();
            let remaining_space = line_start.remaining_space();
            return self.parse_indented_code_block(index, remaining_space);
        }

        let index = start_index + line_start.bytes_scanned();

        if bytes[index] == b'<' {
            if let Some(html_end_tag) = get_html_end_tag(&bytes[(index + 1)..]) {
                self.finish_list(start_index);
                return self.parse_html_block_type_1_to_5(
                    index,
                    html_end_tag,
                    remaining_space,
                    indent,
                );
            }

            if starts_html_block_type_6(&bytes[(index + 1)..]) {
                self.finish_list(start_index);
                return self.parse_html_block_type_6_or_7(index, remaining_space, indent);
            }

            if let Some(_html_bytes) = scan_html_type_7(&bytes[index..]) {
                self.finish_list(start_index);
                return self.parse_html_block_type_6_or_7(index, remaining_space, indent);
            }
        }

        if let Ok(count) = scan_thematic_break(&bytes[index..]) {
            self.finish_list(start_index);
            return self.parse_thematic_break(count, index);
        }

        if let Some(atx_level) = scan_atx_heading(&bytes[index..]) {
            self.finish_list(start_index);
            return self.parse_atx_heading(index, atx_level);
        }

        if let Some((count, fence_character)) = scan_code_fence(&bytes[index..]) {
            self.finish_list(start_index);
            return self.parse_fenced_code_block(index, indent, fence_character, count);
        }

        while let Some((byte_count, label, link_definition)) =
            self.parse_reference_definition_total(start_index + line_start.bytes_scanned())
        {
            self.allocations
                .reference_definitions
                .0
                .entry(label)
                .or_insert(link_definition);
            let container_start = start_index + line_start.bytes_scanned();
            let mut index = container_start + byte_count;

            if let Some(newline) = scan_blank_line(&bytes[index..]) {
                index += newline;
            } else {
                self.finish_list(start_index);
                return index;
            }
            if let Some(lazy_line_start) = self.scan_next_line_or_lazy_continuation(&bytes[index..])
            {
                line_start = lazy_line_start;
                start_index = index;
            } else {
                self.finish_list(start_index);
                return index;
            }
        }

        let index = start_index + line_start.bytes_scanned();

        self.parse_paragraph(index, None)
    }

    fn scan_next_line_or_lazy_continuation<'input>(
        &mut self,
        bytes: &'input [u8],
    ) -> Option<LineStart<'input>> {
        let mut line_start = LineStart::new(bytes);
        let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
        let current_container = tree_position == self.tree.spine_length();
        if !line_start.scan_space(CODE_INDENT_COLUMNS)
            && self
                .scan_paragraph_interrupt(&bytes[line_start.bytes_scanned()..], current_container)
        {
            None
        } else {
            line_start.scan_all_space();
            Some(line_start)
        }
    }

    fn parse_table(
        &mut self,
        table_columns: usize,
        head_start: usize,
        body_start: usize,
    ) -> Option<usize> {
        let mut missing_empty_cells = 0;

        let (_separator_start, head_row_index) =
            self.parse_table_row_inner(head_start, table_columns, &mut missing_empty_cells)?;
        self.tree[head_row_index].item.body = ItemBody::TableHead;

        let mut index = body_start;
        while let Some((next_index, _row_index)) =
            self.parse_table_row(index, table_columns, &mut missing_empty_cells)
        {
            index = next_index;
        }

        self.pop(index);
        Some(index)
    }

    fn parse_table_row_inner(
        &mut self,
        mut index: usize,
        row_cells: usize,
        missing_empty_cells: &mut usize,
    ) -> Option<(usize, TreeIndex)> {
        let bytes = self.text.as_bytes();
        let mut cells = 0;
        let mut final_cell_index = None;

        let old_current = self.tree.current();
        let row_index = self.tree.append(Item {
            start: index,
            end: 0,
            body: ItemBody::TableRow,
        });
        self.tree.push();

        loop {
            index += scan_character(&bytes[index..], b'|');
            let start_index = index;
            index += scan_whitespace_no_newline(&bytes[index..]);

            if let Some(end_of_line_bytes) = scan_end_of_line(&bytes[index..]) {
                index += end_of_line_bytes;
                break;
            }

            let cell_index = self.tree.append(Item {
                start: start_index,
                end: index,
                body: ItemBody::TableCell,
            });
            self.tree.push();
            let (next_index, _line_break) = self.parse_line(index, None, TableParseMode::Active);

            self.tree[cell_index].item.end = next_index;
            self.tree.pop();

            index = next_index;
            cells += 1;

            if cells == row_cells {
                final_cell_index = Some(cell_index);
            }
        }

        if let (Some(current), 0) = (old_current, cells) {
            self.pop(index);
            self.tree[current].next = None;
            return None;
        }

        for _ in cells..row_cells {
            if *missing_empty_cells >= TABLE_FILLED_CELLS_MAX {
                return None;
            }
            *missing_empty_cells += 1;
            self.tree.append(Item {
                start: index,
                end: index,
                body: ItemBody::TableCell,
            });
        }

        if let Some(cell_index) = final_cell_index {
            self.tree[cell_index].next = None;
        }

        self.pop(index);

        Some((index, row_index))
    }

    fn parse_table_row(
        &mut self,
        mut index: usize,
        row_cells: usize,
        missing_empty_cells: &mut usize,
    ) -> Option<(usize, TreeIndex)> {
        let bytes = self.text.as_bytes();
        let mut line_start = LineStart::new(&bytes[index..]);
        let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
        let current_container = tree_position == self.tree.spine_length();
        if !current_container {
            return None;
        }
        line_start.scan_all_space();
        index += line_start.bytes_scanned();
        if scan_paragraph_interrupt_no_table(
            &bytes[index..],
            current_container,
            self.options.contains(Options::ENABLE_FOOTNOTES),
            &self.tree,
        ) {
            return None;
        }

        let (index, row_index) =
            self.parse_table_row_inner(index, row_cells, missing_empty_cells)?;
        Some((index, row_index))
    }

    fn parse_paragraph(&mut self, start_index: usize, task_list_item: Option<Item>) -> usize {
        self.finish_list(start_index);
        let node_index = self.tree.append(Item {
            start: start_index,
            end: 0,
            body: ItemBody::Paragraph,
        });
        self.tree.push();

        if let Some(item) = task_list_item {
            self.tree.append(item);
        }

        let bytes = self.text.as_bytes();
        let mut index = start_index;
        loop {
            let scan_mode = if self.options.contains(Options::ENABLE_TABLES) && index == start_index
            {
                TableParseMode::Scan
            } else {
                TableParseMode::Disabled
            };
            let (next_index, line_break) = self.parse_line(index, None, scan_mode);

            if let Some(Item {
                body: ItemBody::Table(alignment_index),
                ..
            }) = line_break
            {
                let table_columns = self.allocations[alignment_index].len();
                self.tree[node_index].item.body = ItemBody::Table(alignment_index);

                self.tree[node_index].child = None;
                self.tree.pop();
                self.tree.push();
                if let Some(index) = self.parse_table(table_columns, index, next_index) {
                    return index;
                }
            }

            index = next_index;
            let mut line_start = LineStart::new(&bytes[index..]);
            let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
            let current_container = tree_position == self.tree.spine_length();
            let trailing_backslash_position = match line_break {
                Some(Item {
                    start,
                    body: ItemBody::HardBreak(true),
                    ..
                }) if bytes[start] == b'\\' => Some(start),
                _ => None,
            };
            if !line_start.scan_space(CODE_INDENT_COLUMNS) {
                let content_index = index + line_start.bytes_scanned();
                if current_container
                    && let Some(setext_end) = self.parse_setext_heading(
                        content_index,
                        node_index,
                        trailing_backslash_position.is_some(),
                    )
                {
                    if let Some(position) = trailing_backslash_position {
                        self.tree.append_text(position, position + 1, false);
                    }
                    self.pop(setext_end);
                    return setext_end;
                }

                let suffix = &bytes[content_index..];
                if self.scan_paragraph_interrupt(suffix, current_container) {
                    if let Some(position) = trailing_backslash_position {
                        self.tree.append_text(position, position + 1, false);
                    }
                    break;
                }
            }
            line_start.scan_all_space();
            if line_start.is_at_end_of_line() {
                if let Some(position) = trailing_backslash_position {
                    self.tree.append_text(position, position + 1, false);
                }
                break;
            }
            index = next_index + line_start.bytes_scanned();
            if let Some(item) = line_break {
                self.tree.append(item);
            }
        }

        self.pop(index);
        index
    }

    fn parse_setext_heading(
        &mut self,
        index: usize,
        node_index: TreeIndex,
        has_trailing_content: bool,
    ) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let (count, level) = scan_setext_heading(&bytes[index..])?;

        if let Some(current_index) = self.tree.current() {
            let parent_index = self.tree.peek_up().unwrap();
            let header_start = self.tree[parent_index].item.start;
            let content_end = self.tree[current_index].item.end;
            let new_end = if has_trailing_content {
                content_end
            } else {
                let trailing_spaces = scan_rev_while(
                    &bytes[header_start..content_end],
                    is_ascii_whitespace_no_newline,
                );
                content_end - trailing_spaces
            };
            self.tree[current_index].item.end = new_end;
        }

        self.tree[node_index].item.body = ItemBody::Heading(level);

        Some(index + count)
    }

    fn parse_line(
        &mut self,
        start: usize,
        end: Option<usize>,
        mode: TableParseMode,
    ) -> (usize, Option<Item>) {
        let bytes = self.text.as_bytes();
        let bytes = match end {
            Some(end) => &bytes[..end],
            None => bytes,
        };
        let bytes_length = bytes.len();
        let mut pipes = 0;
        let mut last_pipe_index = start;
        let mut begin_text = start;
        let mut backslash_escaped = false;

        let (final_index, line_break) =
            iterate_special_bytes(self.lookup_table, bytes, start, |index, byte| match byte {
                b'\n' | b'\r' => {
                    if let TableParseMode::Active = mode {
                        return LoopInstruction::BreakAtWith(index, None);
                    }

                    let mut cursor = index;
                    let end_of_line_bytes = scan_end_of_line(&bytes[index..]).unwrap();

                    let end_index = index + end_of_line_bytes;
                    let trailing_backslashes =
                        scan_rev_while(&bytes[..index], |other_byte| other_byte == b'\\');
                    if trailing_backslashes % 2 == 1 && end_index < bytes_length {
                        cursor -= 1;
                        self.tree.append_text(begin_text, cursor, backslash_escaped);
                        backslash_escaped = false;
                        return LoopInstruction::BreakAtWith(
                            end_index,
                            Some(Item {
                                start: cursor,
                                end: end_index,
                                body: ItemBody::HardBreak(true),
                            }),
                        );
                    }

                    if mode == TableParseMode::Scan && pipes > 0 {
                        let next_line_index = index + end_of_line_bytes;
                        let mut line_start = LineStart::new(&bytes[next_line_index..]);
                        if scan_containers(&self.tree, &mut line_start, self.options)
                            == self.tree.spine_length()
                        {
                            let table_head_index = next_line_index + line_start.bytes_scanned();
                            let (table_head_bytes, alignment) =
                                scan_table_head(&bytes[table_head_index..]);

                            if table_head_bytes > 0 {
                                let header_count =
                                    count_header_columns(bytes, pipes, start, last_pipe_index);

                                if alignment.len() == header_count {
                                    let alignment_index =
                                        self.allocations.allocate_alignment(alignment);
                                    let end_index = table_head_index + table_head_bytes;
                                    return LoopInstruction::BreakAtWith(
                                        end_index,
                                        Some(Item {
                                            start: cursor,
                                            end: end_index,
                                            body: ItemBody::Table(alignment_index),
                                        }),
                                    );
                                }
                            }
                        }
                    }

                    let trailing_whitespace =
                        scan_rev_while(&bytes[..index], is_ascii_whitespace_no_newline);
                    if trailing_whitespace >= HARD_BREAK_SPACES_MIN {
                        cursor -= trailing_whitespace;
                        self.tree.append_text(begin_text, cursor, backslash_escaped);
                        backslash_escaped = false;
                        return LoopInstruction::BreakAtWith(
                            end_index,
                            Some(Item {
                                start: cursor,
                                end: end_index,
                                body: ItemBody::HardBreak(false),
                            }),
                        );
                    }

                    self.tree.append_text(
                        begin_text,
                        index - trailing_whitespace,
                        backslash_escaped,
                    );
                    backslash_escaped = false;

                    LoopInstruction::BreakAtWith(
                        end_index,
                        Some(Item {
                            start: cursor,
                            end: end_index,
                            body: ItemBody::SoftBreak,
                        }),
                    )
                }
                b'\\' => {
                    if index + 1 < bytes_length && is_ascii_punctuation(bytes[index + 1]) {
                        self.tree.append_text(begin_text, index, backslash_escaped);
                        if bytes[index + 1] == b'`' {
                            let count = 1 + scan_character_repeat(&bytes[(index + 2)..], b'`');
                            self.tree.append(Item {
                                start: index + 1,
                                end: index + count + 1,
                                body: ItemBody::MaybeCode(count, true),
                            });
                            begin_text = index + 1 + count;
                            backslash_escaped = false;
                            LoopInstruction::ContinueAndSkip(count)
                        } else if bytes[index + 1] == b'|' && TableParseMode::Active == mode {
                            begin_text = index + 1;
                            backslash_escaped = false;
                            LoopInstruction::ContinueAndSkip(1)
                        } else if index + 2 < bytes_length
                            && bytes[index + 1] == b'\\'
                            && bytes[index + 2] == b'|'
                            && TableParseMode::Active == mode
                        {
                            begin_text = index + 2;
                            backslash_escaped = true;
                            LoopInstruction::ContinueAndSkip(2)
                        } else {
                            begin_text = index + 1;
                            backslash_escaped = true;
                            LoopInstruction::ContinueAndSkip(1)
                        }
                    } else {
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                delimiter @ b'*' | delimiter @ b'_' | delimiter @ b'~' => {
                    let string_suffix = &self.text[index..];
                    let count =
                        1 + scan_character_repeat(&string_suffix.as_bytes()[1..], delimiter);
                    let can_open = delimiter_run_can_open(
                        &self.text[start..],
                        string_suffix,
                        count,
                        index - start,
                        mode,
                    );
                    let can_close = delimiter_run_can_close(
                        &self.text[start..],
                        string_suffix,
                        count,
                        index - start,
                        mode,
                    );
                    let is_valid_sequence = delimiter != b'~' || count <= STRIKETHROUGH_TILDES_MAX;

                    if (can_open || can_close) && is_valid_sequence {
                        self.tree.append_text(begin_text, index, backslash_escaped);
                        backslash_escaped = false;
                        for cursor in 0..count {
                            self.tree.append(Item {
                                start: index + cursor,
                                end: index + cursor + 1,
                                body: ItemBody::MaybeEmphasis(count - cursor, can_open, can_close),
                            });
                        }
                        begin_text = index + count;
                    }
                    LoopInstruction::ContinueAndSkip(count - 1)
                }
                b'`' => {
                    self.tree.append_text(begin_text, index, backslash_escaped);
                    backslash_escaped = false;
                    let count = 1 + scan_character_repeat(&bytes[(index + 1)..], b'`');
                    self.tree.append(Item {
                        start: index,
                        end: index + count,
                        body: ItemBody::MaybeCode(count, false),
                    });
                    begin_text = index + count;
                    LoopInstruction::ContinueAndSkip(count - 1)
                }
                b'<' if bytes.get(index + 1) != Some(&b'\\') => {
                    self.tree.append_text(begin_text, index, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: index,
                        end: index + 1,
                        body: ItemBody::MaybeHtml,
                    });
                    begin_text = index + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b'!' => {
                    if index + 1 < bytes_length && bytes[index + 1] == b'[' {
                        self.tree.append_text(begin_text, index, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: index,
                            end: index + 2,
                            body: ItemBody::MaybeImage,
                        });
                        begin_text = index + 2;
                        LoopInstruction::ContinueAndSkip(1)
                    } else {
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                b'[' => {
                    self.tree.append_text(begin_text, index, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: index,
                        end: index + 1,
                        body: ItemBody::MaybeLinkOpen,
                    });
                    begin_text = index + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b']' => {
                    self.tree.append_text(begin_text, index, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: index,
                        end: index + 1,
                        body: ItemBody::MaybeLinkClose(true),
                    });
                    begin_text = index + 1;
                    LoopInstruction::ContinueAndSkip(0)
                }
                b'&' => match scan_entity(&bytes[index..]) {
                    (length, Some(value)) => {
                        self.tree.append_text(begin_text, index, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: index,
                            end: index + length,
                            body: ItemBody::SynthesizeText(self.allocations.allocate_string(value)),
                        });
                        begin_text = index + length;
                        LoopInstruction::ContinueAndSkip(length - 1)
                    }
                    _ => LoopInstruction::ContinueAndSkip(0),
                },
                b'|' => {
                    if index != 0 && bytes[index - 1] == b'\\' {
                        LoopInstruction::ContinueAndSkip(0)
                    } else if let TableParseMode::Active = mode {
                        LoopInstruction::BreakAtWith(index, None)
                    } else {
                        last_pipe_index = index;
                        pipes += 1;
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                b'.' => {
                    if index + 2 < bytes.len()
                        && bytes[index + 1] == b'.'
                        && bytes[index + 2] == b'.'
                    {
                        self.tree.append_text(begin_text, index, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: index,
                            end: index + 3,
                            body: ItemBody::SynthesizeChar('…'),
                        });
                        begin_text = index + 3;
                        LoopInstruction::ContinueAndSkip(2)
                    } else {
                        LoopInstruction::ContinueAndSkip(0)
                    }
                }
                b'-' => {
                    let count = 1 + scan_character_repeat(&bytes[(index + 1)..], b'-');
                    if count == 1 {
                        LoopInstruction::ContinueAndSkip(0)
                    } else {
                        let item_body = if count == EN_DASH_HYPHENS {
                            ItemBody::SynthesizeChar('–')
                        } else if count == EM_DASH_HYPHENS {
                            ItemBody::SynthesizeChar('—')
                        } else {
                            let (em_dashes, en_dashes) =
                                match count % (EN_DASH_HYPHENS * EM_DASH_HYPHENS) {
                                    0 | 3 => (count / EM_DASH_HYPHENS, 0),
                                    2 | 4 => (0, count / EN_DASH_HYPHENS),
                                    1 => (count / EM_DASH_HYPHENS - 1, 2),
                                    _ => (count / EM_DASH_HYPHENS, 1),
                                };

                            let mut buffer = String::with_capacity(3 * (em_dashes + en_dashes));
                            for _ in 0..em_dashes {
                                buffer.push('—');
                            }
                            for _ in 0..en_dashes {
                                buffer.push('–');
                            }
                            ItemBody::SynthesizeText(
                                self.allocations.allocate_string(buffer.into()),
                            )
                        };

                        self.tree.append_text(begin_text, index, backslash_escaped);
                        backslash_escaped = false;
                        self.tree.append(Item {
                            start: index,
                            end: index + count,
                            body: item_body,
                        });
                        begin_text = index + count;
                        LoopInstruction::ContinueAndSkip(count - 1)
                    }
                }
                quote @ b'\'' | quote @ b'"' => {
                    let string_suffix = &self.text[index..];
                    let can_open = delimiter_run_can_open(
                        &self.text[start..],
                        string_suffix,
                        1,
                        index - start,
                        mode,
                    );
                    let can_close = delimiter_run_can_close(
                        &self.text[start..],
                        string_suffix,
                        1,
                        index - start,
                        mode,
                    );

                    self.tree.append_text(begin_text, index, backslash_escaped);
                    backslash_escaped = false;
                    self.tree.append(Item {
                        start: index,
                        end: index + 1,
                        body: ItemBody::MaybeSmartQuote(quote, can_open, can_close),
                    });
                    begin_text = index + 1;

                    LoopInstruction::ContinueAndSkip(0)
                }
                _ => LoopInstruction::ContinueAndSkip(0),
            });

        if line_break.is_none() {
            let trailing_whitespace = scan_rev_while(
                &bytes[begin_text..final_index],
                is_ascii_whitespace_no_newline,
            );

            self.tree.append_text(
                begin_text,
                final_index - trailing_whitespace,
                backslash_escaped,
            );
        }
        (final_index, line_break)
    }

    fn parse_html_block_type_1_to_5(
        &mut self,
        start_index: usize,
        html_end_tag: &str,
        mut remaining_space: usize,
        mut indent: usize,
    ) -> usize {
        self.tree.append(Item {
            start: start_index,
            end: 0,
            body: ItemBody::HtmlBlock,
        });
        self.tree.push();

        let bytes = self.text.as_bytes();
        let mut index = start_index;
        let end_index;
        loop {
            let line_start_index = index;
            index += scan_nextline(&bytes[index..]);
            self.append_html_line(remaining_space.max(indent), line_start_index, index);

            let mut line_start = LineStart::new(&bytes[index..]);
            let container_count = scan_containers(&self.tree, &mut line_start, self.options);
            if container_count < self.tree.spine_length() {
                end_index = index;
                break;
            }

            if self.text[line_start_index..index].contains(html_end_tag) {
                end_index = index;
                break;
            }

            let next_line_index = index + line_start.bytes_scanned();
            if next_line_index == self.text.len() {
                end_index = next_line_index;
                break;
            }
            index = next_line_index;
            remaining_space = line_start.remaining_space();
            indent = 0;
        }
        self.pop(end_index);
        index
    }

    fn parse_html_block_type_6_or_7(
        &mut self,
        start_index: usize,
        mut remaining_space: usize,
        mut indent: usize,
    ) -> usize {
        self.tree.append(Item {
            start: start_index,
            end: 0,
            body: ItemBody::HtmlBlock,
        });
        self.tree.push();

        let bytes = self.text.as_bytes();
        let mut index = start_index;
        let end_index;
        loop {
            let line_start_index = index;
            index += scan_nextline(&bytes[index..]);
            self.append_html_line(remaining_space.max(indent), line_start_index, index);

            let mut line_start = LineStart::new(&bytes[index..]);
            let container_count = scan_containers(&self.tree, &mut line_start, self.options);
            if container_count < self.tree.spine_length() || line_start.is_at_end_of_line() {
                end_index = index;
                break;
            }

            let next_line_index = index + line_start.bytes_scanned();
            if next_line_index == self.text.len()
                || scan_blank_line(&bytes[next_line_index..]).is_some()
            {
                end_index = next_line_index;
                break;
            }
            index = next_line_index;
            remaining_space = line_start.remaining_space();
            indent = 0;
        }
        self.pop(end_index);
        index
    }

    fn parse_indented_code_block(
        &mut self,
        start_index: usize,
        mut remaining_space: usize,
    ) -> usize {
        self.tree.append(Item {
            start: start_index,
            end: 0,
            body: ItemBody::IndentCodeBlock,
        });
        self.tree.push();
        let bytes = self.text.as_bytes();
        let mut last_nonblank_child = None;
        let mut last_nonblank_index = 0;
        let mut end_index = 0;
        self.last_line_blank = false;

        let mut index = start_index;
        loop {
            let line_start_index = index;
            index += scan_nextline(&bytes[index..]);
            self.append_code_text(remaining_space, line_start_index, index);

            if !self.last_line_blank {
                last_nonblank_child = self.tree.current();
                last_nonblank_index = index;
                end_index = index;
            }

            let mut line_start = LineStart::new(&bytes[index..]);
            let container_count = scan_containers(&self.tree, &mut line_start, self.options);
            if container_count < self.tree.spine_length()
                || !(line_start.scan_space(CODE_INDENT_COLUMNS) || line_start.is_at_end_of_line())
            {
                break;
            }
            let next_line_index = index + line_start.bytes_scanned();
            if next_line_index == self.text.len() {
                break;
            }
            index = next_line_index;
            remaining_space = line_start.remaining_space();
            self.last_line_blank = scan_blank_line(&bytes[index..]).is_some();
        }

        if let Some(child) = last_nonblank_child {
            self.tree[child].next = None;
            self.tree[child].item.end = last_nonblank_index;
        }
        self.pop(end_index);
        index
    }

    fn parse_fenced_code_block(
        &mut self,
        start_index: usize,
        indent: usize,
        fence_character: u8,
        fence_length: usize,
    ) -> usize {
        let bytes = self.text.as_bytes();
        let mut info_start = start_index + fence_length;
        info_start += scan_whitespace_no_newline(&bytes[info_start..]);

        let mut index = info_start + scan_nextline(&bytes[info_start..]);
        let info_end = index - scan_rev_while(&bytes[info_start..index], is_ascii_whitespace);
        let info_string = unescape(&self.text[info_start..info_end], self.tree.is_in_table());
        self.tree.append(Item {
            start: start_index,
            end: 0,
            body: ItemBody::FencedCodeBlock(self.allocations.allocate_string(info_string)),
        });
        self.tree.push();
        loop {
            let mut line_start = LineStart::new(&bytes[index..]);
            let container_count = scan_containers(&self.tree, &mut line_start, self.options);
            if container_count < self.tree.spine_length() {
                self.pop(index);
                return index;
            }
            line_start.scan_space(indent);
            let mut close_line_start = line_start.clone();
            if !close_line_start.scan_space(CODE_INDENT_COLUMNS - indent) {
                let close_index = index + close_line_start.bytes_scanned();
                if let Some(count) =
                    scan_closing_code_fence(&bytes[close_index..], fence_character, fence_length)
                {
                    index = close_index + count;
                    self.pop(index);

                    return index + scan_blank_line(&bytes[index..]).unwrap_or(0);
                }
            }
            let remaining_space = line_start.remaining_space();
            index += line_start.bytes_scanned();
            let next_index = index + scan_nextline(&bytes[index..]);
            self.append_code_text(remaining_space, index, next_index);
            index = next_index;
        }
    }

    fn append_code_text(&mut self, remaining_space: usize, start: usize, end: usize) {
        if remaining_space > 0 {
            let string_index = self
                .allocations
                .allocate_string("   "[..remaining_space].into());
            self.tree.append(Item {
                start,
                end: start,
                body: ItemBody::SynthesizeText(string_index),
            });
        }
        if self.text.as_bytes()[end - 2] == b'\r' {
            self.tree.append_text(start, end - 2, false);
            self.tree.append_text(end - 1, end, false);
        } else {
            self.tree.append_text(start, end, false);
        }
    }

    fn append_html_line(&mut self, remaining_space: usize, start: usize, end: usize) {
        if remaining_space > 0 {
            let string_index = self
                .allocations
                .allocate_string("   "[..remaining_space].into());
            self.tree.append(Item {
                start,
                end: start,
                body: ItemBody::SynthesizeText(string_index),
            });
        }
        if self.text.as_bytes()[end - 2] == b'\r' {
            self.tree.append(Item {
                start,
                end: end - 2,
                body: ItemBody::Html,
            });
            self.tree.append(Item {
                start: end - 1,
                end,
                body: ItemBody::Html,
            });
        } else {
            self.tree.append(Item {
                start,
                end,
                body: ItemBody::Html,
            });
        }
    }

    fn pop(&mut self, index: usize) {
        let current_index = self.tree.pop().unwrap();
        self.tree[current_index].item.end = index;
        if let ItemBody::List(true, _, _) = self.tree[current_index].item.body {
            surgerize_tight_list(&mut self.tree, current_index);
            self.begin_list_item = None;
        }
    }

    fn finish_list(&mut self, index: usize) {
        self.finish_empty_list_item();
        if let Some(node_index) = self.tree.peek_up()
            && let ItemBody::List(_, _, _) = self.tree[node_index].item.body
        {
            self.pop(index);
        }
        if self.last_line_blank {
            if let Some(node_index) = self.tree.peek_grandparent()
                && let ItemBody::List(ref mut is_tight, _, _) = self.tree[node_index].item.body
            {
                *is_tight = false;
            }
            self.last_line_blank = false;
        }
    }

    fn finish_empty_list_item(&mut self) {
        if let Some(begin_list_item) = self.begin_list_item
            && self.last_line_blank
            && let Some(node_index) = self.tree.peek_up()
            && let ItemBody::ListItem(_) = self.tree[node_index].item.body
        {
            self.pop(begin_list_item);
        }
        self.begin_list_item = None;
    }

    fn continue_list(&mut self, start: usize, marker: u8, start_number: u64) {
        self.finish_empty_list_item();
        if let Some(node_index) = self.tree.peek_up() {
            if let ItemBody::List(ref mut is_tight, existing_character, _) =
                self.tree[node_index].item.body
                && existing_character == marker
            {
                if self.last_line_blank {
                    *is_tight = false;
                    self.last_line_blank = false;
                }
                return;
            }

            self.finish_list(start);
        }
        self.tree.append(Item {
            start,
            end: 0,
            body: ItemBody::List(true, marker, start_number),
        });
        self.tree.push();
        self.last_line_blank = false;
    }

    fn parse_thematic_break(&mut self, thematic_break_size: usize, index: usize) -> usize {
        self.tree.append(Item {
            start: index,
            end: index + thematic_break_size,
            body: ItemBody::Rule,
        });
        index + thematic_break_size
    }

    fn parse_atx_heading(&mut self, start: usize, atx_level: HeadingLevel) -> usize {
        let mut index = start;
        let heading_index = self.tree.append(Item {
            start,
            end: 0,
            body: ItemBody::default(),
        });
        index += atx_level as usize;

        let bytes = self.text.as_bytes();
        if let Some(end_of_line_bytes) = scan_end_of_line(&bytes[index..]) {
            self.tree[heading_index].item.end = index + end_of_line_bytes;
            self.tree[heading_index].item.body = ItemBody::Heading(atx_level);
            return index + end_of_line_bytes;
        }

        let skip_spaces = scan_whitespace_no_newline(&bytes[index..]);
        index += skip_spaces;

        let header_start = index;
        let header_node_index = self.tree.push();

        let (line_index, line_line_break) = self.parse_line(index, None, TableParseMode::Disabled);
        index = line_index;

        if let Some(Item {
            start,
            end,
            body: ItemBody::HardBreak(true),
        }) = line_line_break
        {
            self.tree.append_text(start, end, false);
        }
        let (end, content_end) = (index, index);
        self.tree[header_node_index].item.end = end;

        let mut empty_text_node = false;
        if let Some(current_index) = self.tree.current() {
            let header_text = &bytes[header_start..content_end];
            let mut limit = header_text
                .iter()
                .rposition(|&byte| !(byte == b'\n' || byte == b'\r' || byte == b' '))
                .map_or(0, |cursor| cursor + 1);
            let closer = header_text[..limit]
                .iter()
                .rposition(|&byte| byte != b'#')
                .map_or(0, |cursor| cursor + 1);
            if closer == 0 {
                limit = closer;
            } else {
                let spaces = scan_rev_while(&header_text[..closer], |byte| byte == b' ');
                if spaces > 0 {
                    limit = closer - spaces;
                }
            }

            self.tree[current_index].item.end = limit + header_start;

            if limit == 0 {
                empty_text_node = true;
            }
        }

        if empty_text_node {
            self.tree.remove_node();
        } else {
            self.tree.pop();
        }
        self.tree[heading_index].item.body = ItemBody::Heading(atx_level);

        end
    }

    fn parse_footnote(&mut self, start: usize) -> Option<usize> {
        let bytes = &self.text.as_bytes()[start..];
        if !bytes.starts_with(b"[^") {
            return None;
        }
        let (mut cursor, label) =
            scan_link_label_rest(&self.text[start + 2..], &|_| None, self.tree.is_in_table())?;
        if label.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
            return None;
        }
        cursor += 2;
        if bytes.get(cursor) != Some(&b':') {
            return None;
        }
        cursor += 1;
        self.finish_list(start);
        if let Some(node_index) = self.tree.peek_up()
            && let ItemBody::FootnoteDefinition(..) = self.tree[node_index].item.body
        {
            self.pop(start);
        }
        cursor += scan_whitespace_no_newline(&bytes[cursor..]);
        self.allocations
            .footnote_definitions
            .0
            .insert(LinkLabel::new(label.clone()), FootnoteUse { use_count: 0 });
        self.tree.append(Item {
            start,
            end: 0,
            body: ItemBody::FootnoteDefinition(self.allocations.allocate_string(label)),
        });
        self.tree.push();
        Some(cursor)
    }

    fn parse_reference_definition_label(&self, start: usize) -> Option<(usize, CowStr<'a>)> {
        scan_link_label_rest(
            &self.text[start..],
            &|bytes| {
                let mut line_start = LineStart::new(bytes);
                let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
                let current_container = tree_position == self.tree.spine_length();
                if line_start.scan_space(CODE_INDENT_COLUMNS) {
                    return Some(line_start.bytes_scanned());
                }
                let bytes_scanned = line_start.bytes_scanned();
                let suffix = &bytes[bytes_scanned..];
                if self.scan_paragraph_interrupt(suffix, current_container)
                    || (current_container && scan_setext_heading(suffix).is_some())
                {
                    None
                } else {
                    Some(bytes_scanned)
                }
            },
            self.tree.is_in_table(),
        )
    }

    fn parse_reference_definition_total(
        &mut self,
        start: usize,
    ) -> Option<(usize, LinkLabel<'a>, LinkDefinition<'a>)> {
        let bytes = &self.text.as_bytes()[start..];
        if bytes.first() != Some(&b'[') {
            return None;
        }
        let (mut cursor, label) = self.parse_reference_definition_label(start + 1)?;
        cursor += 1;
        if bytes.get(cursor) != Some(&b':') {
            return None;
        }
        cursor += 1;
        let (byte_count, link_definition) =
            self.scan_reference_definition(start, start + cursor)?;
        Some((byte_count + cursor, LinkLabel::new(label), link_definition))
    }

    fn scan_reference_definition_space(
        &self,
        bytes: &[u8],
        mut cursor: usize,
    ) -> Option<(usize, usize)> {
        let mut newlines = 0;
        loop {
            let whitespaces = scan_whitespace_no_newline(&bytes[cursor..]);
            cursor += whitespaces;
            if let Some(end_of_line_bytes) = scan_end_of_line(&bytes[cursor..]) {
                cursor += end_of_line_bytes;
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
            } else {
                break;
            }
            let mut line_start = LineStart::new(&bytes[cursor..]);
            let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
            let current_container = tree_position == self.tree.spine_length();
            if !line_start.scan_space(CODE_INDENT_COLUMNS) {
                let suffix = &bytes[cursor + line_start.bytes_scanned()..];
                if self.scan_paragraph_interrupt(suffix, current_container)
                    || scan_setext_heading(suffix).is_some()
                {
                    return None;
                }
            }
            cursor += line_start.bytes_scanned();
        }
        Some((cursor, newlines))
    }

    fn scan_reference_definition_title<'t>(&self, text: &'t str) -> Option<(usize, CowStr<'t>)> {
        let bytes = text.as_bytes();
        let closing_delimiter = match bytes.first()? {
            b'\'' => b'\'',
            b'"' => b'"',
            b'(' => b')',
            _ => return None,
        };
        let mut byte_count = 1;
        let mut title_line_start = 1;

        let mut line_buffer = None;

        while let Some(&byte) = bytes.get(byte_count) {
            match byte {
                b'(' if closing_delimiter == b')' => {
                    return None;
                }
                b'\n' | b'\r' => {
                    let line_buffer = if let Some(line_buffer) = &mut line_buffer {
                        line_buffer
                    } else {
                        line_buffer = Some(String::new());
                        line_buffer.as_mut().unwrap()
                    };
                    line_buffer.push_str(&text[title_line_start..byte_count]);
                    line_buffer.push('\n');

                    byte_count += 1;
                    if byte == b'\r' && bytes.get(byte_count) == Some(&b'\n') {
                        byte_count += 1;
                    }
                    let mut line_start = LineStart::new(&bytes[byte_count..]);
                    let tree_position = scan_containers(&self.tree, &mut line_start, self.options);
                    let current_container = tree_position == self.tree.spine_length();
                    if !line_start.scan_space(CODE_INDENT_COLUMNS) {
                        let suffix = &bytes[byte_count + line_start.bytes_scanned()..];
                        if self.scan_paragraph_interrupt(suffix, current_container)
                            || scan_setext_heading(suffix).is_some()
                        {
                            return None;
                        }
                    }
                    line_start.scan_all_space();
                    byte_count += line_start.bytes_scanned();
                    title_line_start = byte_count;
                    if scan_blank_line(&bytes[byte_count..]).is_some() {
                        return None;
                    }
                }
                b'\\' => {
                    byte_count += 1;
                    if let Some(byte) = bytes.get(byte_count)
                        && byte != &b'\r'
                        && byte != &b'\n'
                    {
                        byte_count += 1;
                    }
                }
                byte if byte == closing_delimiter => {
                    let string = if let Some(mut line_buffer) = line_buffer {
                        line_buffer.push_str(&text[title_line_start..byte_count]);
                        CowStr::from(line_buffer)
                    } else {
                        CowStr::from(&text[title_line_start..byte_count])
                    };
                    return Some((byte_count + 1, string));
                }
                _ => {
                    byte_count += 1;
                }
            }
        }
        None
    }

    fn scan_reference_definition(
        &self,
        span_start: usize,
        start: usize,
    ) -> Option<(usize, LinkDefinition<'a>)> {
        let bytes = self.text.as_bytes();

        let (mut cursor, _newlines) = self.scan_reference_definition_space(bytes, start)?;

        let (destination_length, destination) =
            scan_link_destination(self.text, cursor, LINK_DESTINATION_PARENTHESES_MAX)?;
        if destination_length == 0 {
            return None;
        }
        let destination = unescape(destination, self.tree.is_in_table());
        cursor += destination_length;

        let mut backup = (
            cursor - start,
            LinkDefinition {
                destination,
                title: None,
                span: span_start..cursor,
            },
        );

        let (mut cursor, newlines) = if let Some((new_i, mut newlines)) =
            self.scan_reference_definition_space(bytes, cursor)
        {
            if cursor == self.text.len() {
                newlines += 1;
            }
            if new_i == cursor && newlines == 0 {
                return None;
            }
            if newlines > 1 {
                return Some(backup);
            };
            (new_i, newlines)
        } else {
            return Some(backup);
        };

        if let Some((title_length, title)) =
            self.scan_reference_definition_title(&self.text[cursor..])
        {
            cursor += title_length;
            if scan_blank_line(&bytes[cursor..]).is_some() {
                backup.0 = cursor - start;
                backup.1.span = span_start..cursor;
                backup.1.title = Some(unescape(title, self.tree.is_in_table()));
                return Some(backup);
            }
        }
        if newlines > 0 { Some(backup) } else { None }
    }

    fn scan_paragraph_interrupt(&self, bytes: &[u8], current_container: bool) -> bool {
        if scan_paragraph_interrupt_no_table(
            bytes,
            current_container,
            self.options.contains(Options::ENABLE_FOOTNOTES),
            &self.tree,
        ) {
            return true;
        }

        if !self.options.contains(Options::ENABLE_TABLES) || !bytes.starts_with(b"|") {
            return false;
        }

        let mut pipes = 0;
        let mut next_line_index = 0;
        let mut escaped = false;
        let mut last_pipe_index = 0;
        for (cursor, &byte) in bytes.iter().enumerate() {
            match byte {
                b'\\' => {
                    escaped = true;
                    continue;
                }
                b'|' if !escaped => {
                    pipes += 1;
                    last_pipe_index = cursor;
                }
                b'\r' | b'\n' => {
                    next_line_index = cursor + scan_end_of_line(&bytes[cursor..]).unwrap();
                    break;
                }
                _ => {}
            }
            escaped = false;
        }

        if next_line_index == 0 {
            return false;
        }

        let mut line_start = LineStart::new(&bytes[next_line_index..]);
        if scan_containers(&self.tree, &mut line_start, self.options) != self.tree.spine_length() {
            return false;
        }
        let table_head_index = next_line_index + line_start.bytes_scanned();
        let (table_head_bytes, alignment) = scan_table_head(&bytes[table_head_index..]);

        if table_head_bytes == 0 {
            return false;
        }

        let header_count = count_header_columns(bytes, pipes, 0, last_pipe_index);

        alignment.len() == header_count
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum TableParseMode {
    Scan,
    Active,
    Disabled,
}

fn count_header_columns(
    bytes: &[u8],
    mut pipes: usize,
    mut start: usize,
    last_pipe_index: usize,
) -> usize {
    start += scan_whitespace_no_newline(&bytes[start..]);
    if bytes[start] == b'|' {
        pipes -= 1;
    }

    if scan_blank_line(&bytes[(last_pipe_index + 1)..]).is_some() {
        pipes
    } else {
        pipes + 1
    }
}

fn scan_paragraph_interrupt_no_table(
    bytes: &[u8],
    current_container: bool,
    has_footnote: bool,
    tree: &Tree<Item>,
) -> bool {
    scan_end_of_line(bytes).is_some()
        || scan_thematic_break(bytes).is_ok()
        || scan_atx_heading(bytes).is_some()
        || scan_code_fence(bytes).is_some()
        || scan_blockquote_start(bytes).is_some()
        || scan_list_item(bytes).is_some_and(|(index, delimiter, start_number, _)| {
            !current_container
                || tree.is_in_table()
                || (delimiter == b'*'
                    || delimiter == b'-'
                    || delimiter == b'+'
                    || start_number == 1)
                    && (scan_blank_line(&bytes[index..]).is_none())
        })
        || bytes.starts_with(b"<")
            && (get_html_end_tag(&bytes[1..]).is_some() || starts_html_block_type_6(&bytes[1..]))
        || (has_footnote
            && bytes.starts_with(b"[^")
            && scan_link_label_rest(
                std::str::from_utf8(&bytes[2..]).unwrap(),
                &|_| None,
                tree.is_in_table(),
            )
            .is_some_and(|(len, _)| bytes.get(2 + len) == Some(&b':')))
}

fn get_html_end_tag(text_bytes: &[u8]) -> Option<&'static str> {
    static BEGIN_TAGS: &[&[u8]; 4] = &[b"pre", b"style", b"script", b"textarea"];
    static ST_BEGIN_TAGS: &[&[u8]; 3] = &[b"!--", b"?", b"![CDATA["];

    for (beg_tag, end_tag) in BEGIN_TAGS
        .iter()
        .zip(["</pre>", "</style>", "</script>", "</textarea>"].iter())
    {
        let tag_length = beg_tag.len();

        if text_bytes.len() < tag_length {
            break;
        }

        if !text_bytes[..tag_length].eq_ignore_ascii_case(beg_tag) {
            continue;
        }

        if text_bytes.len() == tag_length {
            return Some(end_tag);
        }

        let text = text_bytes[tag_length];
        if is_ascii_whitespace(text) || text == b'>' {
            return Some(end_tag);
        }
    }

    for (beg_tag, end_tag) in ST_BEGIN_TAGS.iter().zip(["-->", "?>", "]]>"].iter()) {
        if text_bytes.starts_with(beg_tag) {
            return Some(end_tag);
        }
    }

    if text_bytes.len() > 1 && text_bytes[0] == b'!' && text_bytes[1].is_ascii_alphabetic() {
        Some(">")
    } else {
        None
    }
}

fn surgerize_tight_list(tree: &mut Tree<Item>, list_index: TreeIndex) {
    let mut list_item = tree[list_index].child;
    while let Some(list_item_index) = list_item {
        let mut node_index = tree[list_item_index].child;
        while let Some(node) = node_index {
            if let ItemBody::Paragraph = tree[node].item.body {
                tree[node].item.body = ItemBody::TightParagraph;
            }
            node_index = tree[node].next;
        }

        list_item = tree[list_item_index].next;
    }
}

fn delimiter_run_can_open(
    text: &str,
    suffix: &str,
    run_length: usize,
    index: usize,
    mode: TableParseMode,
) -> bool {
    let next_char = if let Some(character) = suffix[run_length..].chars().next() {
        character
    } else {
        return false;
    };
    if next_char.is_whitespace() {
        return false;
    }
    if index == 0 {
        return true;
    }
    if mode == TableParseMode::Active {
        if text.as_bytes()[..index].ends_with(b"|") && !text.as_bytes()[..index].ends_with(br"\|") {
            return true;
        }
        if next_char == '|' {
            return false;
        }
    }
    let delimiter = suffix.bytes().next().unwrap();

    if delimiter == b'*' && !is_punctuation(next_char) {
        return true;
    }
    if delimiter == b'~' && run_length > 1 {
        return true;
    }
    let previous_char = text[..index].chars().last().unwrap();
    if delimiter == b'~' && previous_char == '~' && !is_punctuation(next_char) {
        return true;
    }

    previous_char.is_whitespace()
        || is_punctuation(previous_char)
            && (delimiter != b'\'' || ![']', ')'].contains(&previous_char))
}

fn delimiter_run_can_close(
    text: &str,
    suffix: &str,
    run_length: usize,
    index: usize,
    mode: TableParseMode,
) -> bool {
    if index == 0 {
        return false;
    }
    let previous_char = text[..index].chars().last().unwrap();
    if previous_char.is_whitespace() {
        return false;
    }
    let next_char = if let Some(character) = suffix[run_length..].chars().next() {
        character
    } else {
        return true;
    };
    if mode == TableParseMode::Active {
        if text.as_bytes()[..index].ends_with(b"|") && !text.as_bytes()[..index].ends_with(br"\|") {
            return false;
        }
        if next_char == '|' {
            return true;
        }
    }
    let delimiter = suffix.bytes().next().unwrap();

    if (delimiter == b'*' || (delimiter == b'~' && run_length > 1))
        && !is_punctuation(previous_char)
    {
        return true;
    }
    if delimiter == b'~' && previous_char == '~' {
        return true;
    }

    next_char.is_whitespace() || is_punctuation(next_char)
}

fn create_lookup_table(options: &Options) -> LookupTable {
    special_bytes(options)
}

fn special_bytes(options: &Options) -> [bool; 256] {
    let mut bytes = [false; 256];
    let standard_bytes = *b"\n\r*_&\\[]<!`";

    for &byte in &standard_bytes {
        bytes[byte as usize] = true;
    }
    if options.contains(Options::ENABLE_TABLES) {
        bytes[b'|' as usize] = true;
    }
    if options.contains(Options::ENABLE_STRIKETHROUGH) {
        bytes[b'~' as usize] = true;
    }
    if options.contains(Options::ENABLE_SMART_PUNCTUATION) {
        for &byte in b".-\"'" {
            bytes[byte as usize] = true;
        }
    }

    bytes
}

enum LoopInstruction<T> {
    ContinueAndSkip(usize),
    BreakAtWith(usize, T),
}

type LookupTable = [bool; 256];

fn iterate_special_bytes<F, T>(
    lookup_table: &[bool; 256],
    bytes: &[u8],
    mut index: usize,
    mut callback: F,
) -> (usize, Option<T>)
where
    F: FnMut(usize, u8) -> LoopInstruction<Option<T>>,
{
    while index < bytes.len() {
        let b = bytes[index];
        if lookup_table[b as usize] {
            match callback(index, b) {
                LoopInstruction::ContinueAndSkip(skip) => {
                    index += skip;
                }
                LoopInstruction::BreakAtWith(index, item) => {
                    return (index, item);
                }
            }
        }
        index += 1;
    }

    (index, None)
}
