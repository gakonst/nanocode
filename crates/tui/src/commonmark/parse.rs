use std::cmp::{max, min};
use std::collections::{HashMap, VecDeque};
use std::iter::FusedIterator;
use std::ops::{Index, Range};

use super::first_pass::run_first_pass;
use super::label::{LinkLabel, ReferenceLabel, scan_link_label_rest};
use super::scan::*;
use super::text::CowStr;
use super::tree::{Tree, TreeIndex};
use super::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Tag, TagEnd,
};

pub(crate) const LINK_DESTINATION_PARENTHESES_MAX: usize = 32;
const LINK_REFERENCE_EXPANSION_BYTES_MIN: usize = 100_000;
const LINK_CAPACITY: usize = 128;
const DELIMITER_RUN_MULTIPLE: usize = 3;
const LOWER_BOUND_SLOTS: usize = 9;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Item {
    pub start: usize,
    pub end: usize,
    pub body: ItemBody,
}

#[derive(Debug, PartialEq, Clone, Copy, Default)]
pub(crate) enum ItemBody {
    MaybeEmphasis(usize, bool, bool),
    MaybeSmartQuote(u8, bool, bool),
    MaybeCode(usize, bool),
    MaybeHtml,
    MaybeLinkOpen,
    MaybeLinkClose(bool),
    MaybeImage,
    Emphasis,
    Strong,
    Strikethrough,
    Code(StringIndex),
    Link(LinkIndex),
    Image(LinkIndex),
    FootnoteReference(StringIndex),
    TaskListMarker(bool),
    InlineHtml,
    OwnedInlineHtml(StringIndex),
    SynthesizeText(StringIndex),
    SynthesizeChar(char),
    Html,
    Text {
        backslash_escaped: bool,
    },
    SoftBreak,
    HardBreak(bool),

    #[default]
    Root,
    Paragraph,
    TightParagraph,
    Rule,
    Heading(HeadingLevel),
    FencedCodeBlock(StringIndex),
    IndentCodeBlock,
    HtmlBlock,
    BlockQuote(Option<BlockQuoteKind>),
    List(bool, u8, u64),
    ListItem(usize),
    FootnoteDefinition(StringIndex),
    Table(AlignmentIndex),
    TableHead,
    TableRow,
    TableCell,
}

impl ItemBody {
    const fn is_maybe_inline(&self) -> bool {
        use ItemBody::*;
        matches!(
            *self,
            MaybeEmphasis(..)
                | MaybeSmartQuote(..)
                | MaybeCode(..)
                | MaybeHtml
                | MaybeLinkOpen
                | MaybeLinkClose(..)
                | MaybeImage
        )
    }
    const fn is_inline(&self) -> bool {
        use ItemBody::*;
        matches!(
            *self,
            MaybeEmphasis(..)
                | MaybeSmartQuote(..)
                | MaybeCode(..)
                | MaybeHtml
                | MaybeLinkOpen
                | MaybeLinkClose(..)
                | MaybeImage
                | Emphasis
                | Strong
                | Strikethrough
                | Code(..)
                | Link(..)
                | Image(..)
                | FootnoteReference(..)
                | TaskListMarker(..)
                | InlineHtml
                | OwnedInlineHtml(..)
                | SynthesizeText(..)
                | SynthesizeChar(..)
                | Html
                | Text { .. }
                | SoftBreak
                | HardBreak(..)
        )
    }
}

pub struct Parser<'input> {
    text: &'input str,
    options: Options,
    tree: Tree<Item>,
    allocations: Allocations<'input>,
    html_scan_guard: HtmlScanGuard,
    link_reference_expansion_limit: usize,
    inline_stack: InlineStack,
    link_stack: LinkStack,
    code_delimiters: CodeDelimiters,
}

impl<'input> Parser<'input> {
    pub fn new_ext(text: &'input str, options: Options) -> Self {
        let (mut tree, allocations) = run_first_pass(text, options);
        tree.reset();
        Parser {
            text,
            options,
            tree,
            allocations,
            inline_stack: InlineStack::default(),
            link_stack: LinkStack::default(),
            html_scan_guard: HtmlScanGuard::default(),
            link_reference_expansion_limit: text.len().max(LINK_REFERENCE_EXPANSION_BYTES_MIN),
            code_delimiters: CodeDelimiters::new(),
        }
    }

    fn fetch_link_type_url_title(
        &mut self,
        link_label: CowStr<'input>,
        link_type: LinkType,
    ) -> Option<(LinkType, CowStr<'input>, CowStr<'input>)> {
        if self.link_reference_expansion_limit == 0 {
            return None;
        }
        let (link_type, url, title) =
            self.allocations
                .reference_definitions
                .get(link_label)
                .map(|matching_definition| {
                    let title = matching_definition
                        .title
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| "".into());
                    let url = matching_definition.destination.clone();
                    (link_type, url, title)
                })?;

        self.link_reference_expansion_limit = self
            .link_reference_expansion_limit
            .saturating_sub(url.len() + title.len());

        Some((link_type, url, title))
    }

    fn handle_inline(&mut self) {
        self.handle_inline_links();
        self.handle_emphasis_and_hard_break();
    }

    fn handle_inline_links(&mut self) {
        let mut current = self.tree.current();
        let mut previous = None;

        let block_end = self.tree[self.tree.peek_up().unwrap()].item.end;
        let block_text = &self.text[..block_end];

        while let Some(mut current_index) = current {
            match self.tree[current_index].item.body {
                ItemBody::MaybeHtml => {
                    let next = self.tree[current_index].next;
                    let autolink = if let Some(next_index) = next {
                        scan_autolink(block_text, self.tree[next_index].item.start)
                    } else {
                        None
                    };

                    if let Some((index, uri, link_type)) = autolink {
                        let node = scan_nodes_to_index(&self.tree, next, index);
                        let text_node = self.tree.create_node(Item {
                            start: self.tree[current_index].item.start + 1,
                            end: index - 1,
                            body: ItemBody::Text {
                                backslash_escaped: false,
                            },
                        });
                        let link_index =
                            self.allocations
                                .allocate_link(link_type, uri, "".into(), "".into());
                        self.tree[current_index].item.body = ItemBody::Link(link_index);
                        self.tree[current_index].item.end = index;
                        self.tree[current_index].next = node;
                        self.tree[current_index].child = Some(text_node);
                        previous = current;
                        current = node;
                        if let Some(node_index) = current {
                            self.tree[node_index].item.start =
                                max(self.tree[node_index].item.start, index);
                        }
                        continue;
                    } else {
                        let inline_html = next.and_then(|next_index| {
                            self.scan_inline_html(
                                block_text.as_bytes(),
                                self.tree[next_index].item.start,
                            )
                        });
                        if let Some((span, index)) = inline_html {
                            let node = scan_nodes_to_index(&self.tree, next, index);
                            self.tree[current_index].item.body = if !span.is_empty() {
                                let converted_string =
                                    String::from_utf8(span).expect("invalid utf8");
                                ItemBody::OwnedInlineHtml(
                                    self.allocations.allocate_string(converted_string.into()),
                                )
                            } else {
                                ItemBody::InlineHtml
                            };
                            self.tree[current_index].item.end = index;
                            self.tree[current_index].next = node;
                            previous = current;
                            current = node;
                            if let Some(node_index) = current {
                                self.tree[node_index].item.start =
                                    max(self.tree[node_index].item.start, index);
                            }
                            continue;
                        }
                    }
                    self.tree[current_index].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                }
                ItemBody::MaybeCode(mut search_count, preceded_by_backslash) => {
                    if preceded_by_backslash {
                        search_count -= 1;
                        if search_count == 0 {
                            self.tree[current_index].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                            previous = current;
                            current = self.tree[current_index].next;
                            continue;
                        }
                    }

                    if self.code_delimiters.is_populated() {
                        if let Some(scan_index) =
                            self.code_delimiters.find(current_index, search_count)
                        {
                            self.make_code_span(current_index, scan_index, preceded_by_backslash);
                        } else {
                            self.tree[current_index].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    } else {
                        let mut scan = if search_count > 0 {
                            self.tree[current_index].next
                        } else {
                            None
                        };
                        while let Some(scan_index) = scan {
                            if let ItemBody::MaybeCode(delimiter_count, _) =
                                self.tree[scan_index].item.body
                            {
                                if search_count == delimiter_count {
                                    self.make_code_span(
                                        current_index,
                                        scan_index,
                                        preceded_by_backslash,
                                    );
                                    self.code_delimiters.clear();
                                    break;
                                } else {
                                    self.code_delimiters.insert(delimiter_count, scan_index);
                                }
                            }
                            scan = self.tree[scan_index].next;
                        }
                        if scan.is_none() {
                            self.tree[current_index].item.body = ItemBody::Text {
                                backslash_escaped: false,
                            };
                        }
                    }
                }
                ItemBody::MaybeLinkOpen => {
                    self.tree[current_index].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    self.link_stack.push(LinkStackElement {
                        node: current_index,
                        kind: LinkStackKind::Link,
                    });
                }
                ItemBody::MaybeImage => {
                    self.tree[current_index].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    self.link_stack.push(LinkStackElement {
                        node: current_index,
                        kind: LinkStackKind::Image,
                    });
                }
                ItemBody::MaybeLinkClose(could_be_reference) => {
                    self.tree[current_index].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                    let top_link = self.link_stack.pop();

                    if let Some(top) = top_link {
                        if top.kind != LinkStackKind::Image
                            && matches!(
                                self.tree[self.tree.peek_up().unwrap()].item.body,
                                ItemBody::Link(..)
                            )
                        {
                            continue;
                        }
                        if top.kind == LinkStackKind::Disabled {
                            continue;
                        }
                        let next = self.tree[current_index].next;
                        if let Some((next_index, url, title)) = self.scan_inline_link(
                            block_text,
                            self.tree[current_index].item.end,
                            next,
                        ) {
                            let next_node = scan_nodes_to_index(&self.tree, next, next_index);
                            if let Some(previous_index) = previous {
                                self.tree[previous_index].next = None;
                            }
                            current = Some(top.node);
                            current_index = top.node;
                            let link_index = self.allocations.allocate_link(
                                LinkType::Inline,
                                url,
                                title,
                                "".into(),
                            );
                            self.tree[current_index].item.body = if top.kind == LinkStackKind::Image
                            {
                                ItemBody::Image(link_index)
                            } else {
                                ItemBody::Link(link_index)
                            };
                            self.tree[current_index].child = self.tree[current_index].next;
                            self.tree[current_index].next = next_node;
                            self.tree[current_index].item.end = next_index;
                            if let Some(next_node_index) = next_node {
                                self.tree[next_node_index].item.start =
                                    max(self.tree[next_node_index].item.start, next_index);
                            }

                            if top.kind == LinkStackKind::Link {
                                self.disable_all_links();
                            }
                        } else {
                            let scan_result =
                                scan_reference(&self.tree, block_text, next, self.options);
                            let (node_after_link, link_type) = match scan_result {
                                ReferenceScan::LinkLabel(_, end_index) => {
                                    let reference_close_node = if let Some(node) =
                                        scan_nodes_to_index(&self.tree, next, end_index - 1)
                                    {
                                        node
                                    } else {
                                        continue;
                                    };
                                    self.tree[reference_close_node].item.body =
                                        ItemBody::MaybeLinkClose(false);
                                    let next_node = self.tree[reference_close_node].next;

                                    (next_node, LinkType::Reference)
                                }

                                ReferenceScan::Collapsed(next_node) => {
                                    if !could_be_reference {
                                        continue;
                                    }
                                    (next_node, LinkType::Collapsed)
                                }

                                ReferenceScan::Failed | ReferenceScan::UnexpectedFootnote => {
                                    if !could_be_reference {
                                        continue;
                                    }
                                    (next, LinkType::Shortcut)
                                }
                            };

                            let label: Option<(ReferenceLabel<'input>, usize)> = match scan_result {
                                ReferenceScan::LinkLabel(length, end_index) => {
                                    Some((ReferenceLabel::Link(length), end_index))
                                }
                                ReferenceScan::Collapsed(..)
                                | ReferenceScan::Failed
                                | ReferenceScan::UnexpectedFootnote => {
                                    let label_start = self.tree[top.node].item.end - 1;
                                    let label_end = self.tree[current_index].item.end;
                                    scan_link_label(
                                        &self.tree,
                                        &self.text[label_start..label_end],
                                        self.options,
                                    )
                                    .map(|(index, label)| (label, label_start + index))
                                    .filter(|(_, end)| *end == label_end)
                                }
                            };

                            let id = match &label {
                                Some(
                                    (ReferenceLabel::Link(length), _)
                                    | (ReferenceLabel::Footnote(length), _),
                                ) => length.clone(),
                                None => "".into(),
                            };

                            if let Some((ReferenceLabel::Footnote(length), end)) = label {
                                let footnote_reference = self.allocations.allocate_string(length);
                                if let Some(definition) =
                                    self.allocations.footnote_definitions.get_mut(
                                        self.allocations.strings[footnote_reference.0].to_owned(),
                                    )
                                {
                                    definition.use_count += 1;
                                }
                                if self.allocations.footnote_definitions.contains(
                                    self.allocations.strings[footnote_reference.0].clone(),
                                ) {
                                    let footnote_index = if top.kind == LinkStackKind::Image {
                                        self.tree[top.node].next = Some(current_index);
                                        self.tree[top.node].child = None;
                                        self.tree[top.node].item.body =
                                            ItemBody::SynthesizeChar('!');
                                        self.tree[current_index].item.start =
                                            self.tree[top.node].item.start + 1;
                                        self.tree[top.node].item.end =
                                            self.tree[top.node].item.start + 1;
                                        current_index
                                    } else {
                                        top.node
                                    };

                                    self.tree[footnote_index].next = next;
                                    self.tree[footnote_index].child = None;
                                    self.tree[footnote_index].item.body =
                                        ItemBody::FootnoteReference(footnote_reference);
                                    self.tree[footnote_index].item.end = end;
                                    previous = Some(footnote_index);
                                    current = next;
                                    self.link_stack.clear();
                                    continue;
                                }
                            } else if let Some((ReferenceLabel::Link(link_label), end)) = label
                                && let Some((definition_link_type, url, title)) =
                                    self.fetch_link_type_url_title(link_label, link_type)
                            {
                                let link_index = self.allocations.allocate_link(
                                    definition_link_type,
                                    url,
                                    title,
                                    id,
                                );
                                self.tree[top.node].item.body = if top.kind == LinkStackKind::Image
                                {
                                    ItemBody::Image(link_index)
                                } else {
                                    ItemBody::Link(link_index)
                                };
                                let label_node = self.tree[top.node].next;

                                self.tree[top.node].next = node_after_link;

                                if label_node != current {
                                    self.tree[top.node].child = label_node;

                                    if let Some(previous_index) = previous {
                                        self.tree[previous_index].next = None;
                                    }
                                }

                                self.tree[top.node].item.end = end;

                                current = Some(top.node);
                                current_index = top.node;

                                if top.kind == LinkStackKind::Link {
                                    self.disable_all_links();
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            previous = current;
            current = self.tree[current_index].next;
        }
        self.link_stack.clear();
        self.code_delimiters.clear();
    }

    fn handle_emphasis_and_hard_break(&mut self) {
        let mut previous = None;
        let mut previous_index: TreeIndex;
        let mut current = self.tree.current();

        let mut single_quote_open: Option<TreeIndex> = None;
        let mut double_quote_open: bool = false;

        while let Some(mut current_index) = current {
            match self.tree[current_index].item.body {
                ItemBody::MaybeEmphasis(mut count, can_open, can_close) => {
                    let run_length = count;
                    let byte = self.text.as_bytes()[self.tree[current_index].item.start];
                    let both = can_open && can_close;
                    if can_close {
                        while let Some(element) =
                            self.inline_stack
                                .find_match(&mut self.tree, byte, run_length, both)
                        {
                            if let Some(previous_index) = previous {
                                self.tree[previous_index].next = None;
                            }
                            let match_count = min(count, element.count);

                            let mut end = current_index - 1;
                            let mut start = element.start + element.count;

                            while start > element.start + element.count - match_count {
                                let increment =
                                    if start > element.start + element.count - match_count + 1 {
                                        2
                                    } else {
                                        1
                                    };
                                let kind = if byte == b'~' {
                                    if self.options.contains(Options::ENABLE_STRIKETHROUGH) {
                                        ItemBody::Strikethrough
                                    } else {
                                        ItemBody::Text {
                                            backslash_escaped: false,
                                        }
                                    }
                                } else if increment == 2 {
                                    ItemBody::Strong
                                } else {
                                    ItemBody::Emphasis
                                };

                                let root = start - increment;
                                end = end + increment;
                                self.tree[root].item.body = kind;
                                self.tree[root].item.end = self.tree[end].item.end;
                                self.tree[root].child = Some(start);
                                self.tree[root].next = None;
                                start = root;
                            }

                            previous_index = element.start + element.count - match_count;
                            previous = Some(previous_index);
                            current = self.tree[current_index + match_count - 1].next;
                            self.tree[previous_index].next = current;

                            if element.count > match_count {
                                self.inline_stack.push(InlineElement {
                                    start: element.start,
                                    count: element.count - match_count,
                                    run_length: element.run_length,
                                    delimiter: element.delimiter,
                                    both: element.both,
                                })
                            }
                            count -= match_count;
                            if count > 0 {
                                current_index = current.unwrap();
                            } else {
                                break;
                            }
                        }
                    }
                    if count > 0 {
                        if can_open {
                            self.inline_stack.push(InlineElement {
                                start: current_index,
                                run_length,
                                count,
                                delimiter: byte,
                                both,
                            });
                        } else {
                            for cursor in 0..count {
                                self.tree[current_index + cursor].item.body = ItemBody::Text {
                                    backslash_escaped: false,
                                };
                            }
                        }
                        previous_index = current_index + count - 1;
                        previous = Some(previous_index);
                        current = self.tree[previous_index].next;
                    }
                }
                ItemBody::MaybeSmartQuote(byte, can_open, can_close) => {
                    self.tree[current_index].item.body = match byte {
                        b'\'' => {
                            if let (Some(open_index), true) = (single_quote_open, can_close) {
                                self.tree[open_index].item.body = ItemBody::SynthesizeChar('‘');
                                single_quote_open = None;
                            } else if can_open {
                                single_quote_open = Some(current_index);
                            }
                            ItemBody::SynthesizeChar('’')
                        }
                        _ => {
                            if can_close && double_quote_open {
                                double_quote_open = false;
                                ItemBody::SynthesizeChar('”')
                            } else {
                                if can_open && !double_quote_open {
                                    double_quote_open = true;
                                }
                                ItemBody::SynthesizeChar('“')
                            }
                        }
                    };
                    previous = current;
                    current = self.tree[current_index].next;
                }
                ItemBody::HardBreak(true) => {
                    if self.tree[current_index].next.is_none() {
                        self.tree[current_index].item.body = ItemBody::SynthesizeChar('\\');
                    }
                    previous = current;
                    current = self.tree[current_index].next;
                }
                _ => {
                    previous = current;
                    current = self.tree[current_index].next;
                }
            }
        }
        self.inline_stack.pop_all(&mut self.tree);
    }

    fn disable_all_links(&mut self) {
        self.link_stack.disable_all_links();
    }

    fn scan_inline_link(
        &self,
        underlying: &'input str,
        mut index: usize,
        node: Option<TreeIndex>,
    ) -> Option<(usize, CowStr<'input>, CowStr<'input>)> {
        if underlying.as_bytes().get(index) != Some(&b'(') {
            return None;
        }
        index += 1;

        let scan_separator = |index: &mut usize| {
            *index += scan_while(
                &underlying.as_bytes()[*index..],
                is_ascii_whitespace_no_newline,
            );
            if let Some(line_break_length) = scan_end_of_line(&underlying.as_bytes()[*index..]) {
                *index += line_break_length;
                *index += skip_container_prefixes(
                    &self.tree,
                    &underlying.as_bytes()[*index..],
                    self.options,
                );
            }
            *index += scan_while(
                &underlying.as_bytes()[*index..],
                is_ascii_whitespace_no_newline,
            );
        };

        scan_separator(&mut index);

        let (destination_length, destination) =
            scan_link_destination(underlying, index, LINK_DESTINATION_PARENTHESES_MAX)?;
        let destination = unescape(destination, self.tree.is_in_table());
        index += destination_length;

        scan_separator(&mut index);

        let title = if let Some((bytes_scanned, parsed_title)) =
            self.scan_link_title(underlying, index, node)
        {
            index += bytes_scanned;
            scan_separator(&mut index);
            parsed_title
        } else {
            "".into()
        };
        if underlying.as_bytes().get(index) != Some(&b')') {
            return None;
        }
        index += 1;

        Some((index, destination, title))
    }

    fn scan_link_title(
        &self,
        text: &'input str,
        start_index: usize,
        node: Option<TreeIndex>,
    ) -> Option<(usize, CowStr<'input>)> {
        let bytes = text.as_bytes();
        let open = match bytes.get(start_index) {
            Some(other_byte @ b'\'') | Some(other_byte @ b'\"') | Some(other_byte @ b'(') => {
                *other_byte
            }
            _ => return None,
        };
        let close = if open == b'(' { b')' } else { open };

        let mut title = String::new();
        let mut mark = start_index + 1;
        let mut cursor = start_index + 1;

        while cursor < bytes.len() {
            let byte = bytes[cursor];

            if byte == close {
                let string = if mark == 1 {
                    (cursor - start_index + 1, text[mark..cursor].into())
                } else {
                    title.push_str(&text[mark..cursor]);
                    (cursor - start_index + 1, title.into())
                };

                return Some(string);
            }
            if byte == open {
                return None;
            }

            if (byte == b'\n' || byte == b'\r')
                && let Some(node_index) = scan_nodes_to_index(&self.tree, node, cursor + 1)
                && self.tree[node_index].item.start > cursor
            {
                title.push_str(&text[mark..cursor]);
                title.push('\n');
                cursor = self.tree[node_index].item.start;
                mark = cursor;
                continue;
            }
            if byte == b'&'
                && let (count, Some(value)) = scan_entity(&bytes[cursor..])
            {
                title.push_str(&text[mark..cursor]);
                title.push_str(&value);
                cursor += count;
                mark = cursor;
                continue;
            }
            if self.tree.is_in_table()
                && byte == b'\\'
                && cursor + 2 < bytes.len()
                && bytes[cursor + 1] == b'\\'
                && bytes[cursor + 2] == b'|'
            {
                title.push_str(&text[mark..cursor]);
                cursor += 2;
                mark = cursor;
            }
            if byte == b'\\' && cursor + 1 < bytes.len() && is_ascii_punctuation(bytes[cursor + 1])
            {
                title.push_str(&text[mark..cursor]);
                cursor += 1;
                mark = cursor;
            }

            cursor += 1;
        }

        None
    }

    fn make_code_span(&mut self, open: TreeIndex, close: TreeIndex, preceding_backslash: bool) {
        let span_start = self.tree[open].item.end;
        let span_end = self.tree[close].item.start;
        let mut buffer: Option<String> = None;

        let spanned_text = &self.text[span_start..span_end];
        let spanned_bytes = spanned_text.as_bytes();
        let mut start_index = 0;
        let mut index = 0;
        while index < spanned_bytes.len() {
            let byte = spanned_bytes[index];
            if byte == b'\r' || byte == b'\n' {
                let buffer =
                    buffer.get_or_insert_with(|| String::with_capacity(spanned_bytes.len()));
                buffer.push_str(&spanned_text[start_index..index]);
                buffer.push(' ');
                index += 1;
                index += skip_container_prefixes(&self.tree, &spanned_bytes[index..], self.options);
                start_index = index;
            } else if byte == b'\\'
                && spanned_bytes.get(index + 1) == Some(&b'|')
                && self.tree.is_in_table()
            {
                let buffer =
                    buffer.get_or_insert_with(|| String::with_capacity(spanned_bytes.len()));
                buffer.push_str(&spanned_text[start_index..index]);
                buffer.push('|');
                index += 2;
                start_index = index;
            } else {
                index += 1;
            }
        }

        let (opening, closing, all_spaces) = {
            let source = if let Some(buffer) = &mut buffer {
                buffer.push_str(&spanned_text[start_index..]);
                &buffer[..]
            } else {
                spanned_text
            };
            (
                source.as_bytes().first() == Some(&b' '),
                source.as_bytes().last() == Some(&b' '),
                source.bytes().all(|other_byte| other_byte == b' '),
            )
        };

        let string: CowStr<'input> = if !all_spaces && opening && closing {
            if let Some(mut buffer) = buffer {
                if !buffer.is_empty() {
                    buffer.remove(0);
                    buffer.pop();
                }
                buffer.into()
            } else {
                spanned_text[1..(spanned_text.len() - 1).max(1)].into()
            }
        } else if let Some(buffer) = buffer {
            buffer.into()
        } else {
            spanned_text.into()
        };

        if preceding_backslash {
            self.tree[open].item.body = ItemBody::Text {
                backslash_escaped: true,
            };
            self.tree[open].item.end = self.tree[open].item.start + 1;
            self.tree[open].next = Some(close);
            self.tree[close].item.body = ItemBody::Code(self.allocations.allocate_string(string));
            self.tree[close].item.start = self.tree[open].item.start + 1;
        } else {
            self.tree[open].item.body = ItemBody::Code(self.allocations.allocate_string(string));
            self.tree[open].item.end = self.tree[close].item.end;
            self.tree[open].next = self.tree[close].next;
        }
    }

    fn scan_inline_html(&mut self, bytes: &[u8], index: usize) -> Option<(Vec<u8>, usize)> {
        let byte = *bytes.get(index)?;
        if byte == b'!' {
            Some((
                vec![],
                scan_inline_html_comment(bytes, index + 1, &mut self.html_scan_guard)?,
            ))
        } else if byte == b'?' {
            Some((
                vec![],
                scan_inline_html_processing(bytes, index + 1, &mut self.html_scan_guard)?,
            ))
        } else {
            let (span, cursor) = scan_html_block_inner(
                &bytes[(index - 1)..],
                Some(&|bytes| skip_container_prefixes(&self.tree, bytes, self.options)),
            )?;
            Some((span, cursor + index - 1))
        }
    }

    pub const fn into_offset_iter(self) -> OffsetIter<'input> {
        OffsetIter { inner: self }
    }
}

pub(crate) fn scan_containers(
    tree: &Tree<Item>,
    line_start: &mut LineStart<'_>,
    options: Options,
) -> usize {
    let mut cursor = 0;
    for &node_index in tree.walk_spine() {
        match tree[node_index].item.body {
            ItemBody::BlockQuote(..) => {
                let save = line_start.clone();
                let _ = line_start.scan_space(BLOCK_INDENT_COLUMNS_MAX);
                if !line_start.scan_blockquote_marker() {
                    *line_start = save;
                    break;
                }
            }
            ItemBody::ListItem(indent) => {
                let save = line_start.clone();
                if !line_start.scan_space(indent) && !line_start.is_at_end_of_line() {
                    *line_start = save;
                    break;
                }
            }
            ItemBody::FootnoteDefinition(..) if options.contains(Options::ENABLE_FOOTNOTES) => {
                let save = line_start.clone();
                if !line_start.scan_space(CODE_INDENT_COLUMNS) && !line_start.is_at_end_of_line() {
                    *line_start = save;
                    break;
                }
            }
            _ => (),
        }
        cursor += 1;
    }
    cursor
}
pub(crate) fn skip_container_prefixes(tree: &Tree<Item>, bytes: &[u8], options: Options) -> usize {
    let mut line_start = LineStart::new(bytes);
    let _ = scan_containers(tree, &mut line_start, options);
    line_start.bytes_scanned()
}

impl Tree<Item> {
    pub(crate) fn append_text(&mut self, start: usize, end: usize, backslash_escaped: bool) {
        if end > start {
            if let Some(index) = self.current()
                && matches!(self[index].item.body, ItemBody::Text { .. })
                && self[index].item.end == start
            {
                self[index].item.end = end;
                return;
            }
            self.append(Item {
                start,
                end,
                body: ItemBody::Text { backslash_escaped },
            });
        }
    }

    pub(crate) fn is_in_table(&self) -> bool {
        const fn might_be_in_table(item: &Item) -> bool {
            item.body.is_inline()
                || matches!(item.body, |ItemBody::TableHead| ItemBody::TableRow
                    | ItemBody::TableCell)
        }
        for &index in self.walk_spine().rev() {
            if matches!(self[index].item.body, ItemBody::Table(_)) {
                return true;
            }
            if !might_be_in_table(&self[index].item) {
                return false;
            }
        }
        false
    }
}

#[derive(Copy, Clone, Debug)]
struct InlineElement {
    start: TreeIndex,
    count: usize,
    run_length: usize,
    delimiter: u8,
    both: bool,
}

#[derive(Debug, Clone, Default)]
struct InlineStack {
    stack: Vec<InlineElement>,
    lower_bounds: [usize; LOWER_BOUND_SLOTS],
}

impl InlineStack {
    const UNDERSCORE_NOT_BOTH: usize = 0;
    const ASTERISK_NOT_BOTH: usize = 1;
    const ASTERISK_BASE: usize = 2;
    const TILDES: usize = 5;
    const UNDERSCORE_BASE: usize = 6;

    fn pop_all(&mut self, tree: &mut Tree<Item>) {
        for element in self.stack.drain(..) {
            for cursor in 0..element.count {
                tree[element.start + cursor].item.body = ItemBody::Text {
                    backslash_escaped: false,
                };
            }
        }
        self.lower_bounds = [0; LOWER_BOUND_SLOTS];
    }

    fn get_lower_bound(&self, byte: u8, count: usize, both: bool) -> usize {
        if byte == b'_' {
            let modulo_three_lower =
                self.lower_bounds[Self::UNDERSCORE_BASE + count % DELIMITER_RUN_MULTIPLE];
            if both {
                modulo_three_lower
            } else {
                min(
                    modulo_three_lower,
                    self.lower_bounds[Self::UNDERSCORE_NOT_BOTH],
                )
            }
        } else if byte == b'*' {
            let modulo_three_lower =
                self.lower_bounds[Self::ASTERISK_BASE + count % DELIMITER_RUN_MULTIPLE];
            if both {
                modulo_three_lower
            } else {
                min(
                    modulo_three_lower,
                    self.lower_bounds[Self::ASTERISK_NOT_BOTH],
                )
            }
        } else {
            self.lower_bounds[Self::TILDES]
        }
    }

    const fn set_lower_bound(&mut self, byte: u8, count: usize, both: bool, new_bound: usize) {
        if byte == b'_' {
            if both {
                self.lower_bounds[Self::UNDERSCORE_BASE + count % DELIMITER_RUN_MULTIPLE] =
                    new_bound;
            } else {
                self.lower_bounds[Self::UNDERSCORE_NOT_BOTH] = new_bound;
            }
        } else if byte == b'*' {
            self.lower_bounds[Self::ASTERISK_BASE + count % DELIMITER_RUN_MULTIPLE] = new_bound;
            if !both {
                self.lower_bounds[Self::ASTERISK_NOT_BOTH] = new_bound;
            }
        } else {
            self.lower_bounds[Self::TILDES] = new_bound;
        }
    }

    fn truncate(&mut self, new_bound: usize) {
        self.stack.truncate(new_bound);
        for lower_bound in &mut self.lower_bounds {
            if *lower_bound > new_bound {
                *lower_bound = new_bound;
            }
        }
    }

    fn find_match(
        &mut self,
        tree: &mut Tree<Item>,
        byte: u8,
        run_length: usize,
        both: bool,
    ) -> Option<InlineElement> {
        let lower_bound = min(
            self.stack.len(),
            self.get_lower_bound(byte, run_length, both),
        );
        let res = self.stack[lower_bound..]
            .iter()
            .cloned()
            .enumerate()
            .rfind(|(_, element)| {
                if byte == b'~' && run_length != element.run_length {
                    return false;
                }
                element.delimiter == byte
                    && (!both && !element.both
                        || !(run_length + element.run_length).is_multiple_of(3)
                        || run_length.is_multiple_of(3))
            });

        if let Some((matching_index, matching_element)) = res {
            let matching_index = matching_index + lower_bound;
            for element in &self.stack[(matching_index + 1)..] {
                for cursor in 0..element.count {
                    tree[element.start + cursor].item.body = ItemBody::Text {
                        backslash_escaped: false,
                    };
                }
            }
            self.truncate(matching_index);
            Some(matching_element)
        } else {
            self.set_lower_bound(byte, run_length, both, self.stack.len());
            None
        }
    }

    fn trim_lower_bound(&mut self, index: usize) {
        self.lower_bounds[index] = self.lower_bounds[index].min(self.stack.len());
    }

    fn push(&mut self, element: InlineElement) {
        if element.delimiter == b'~' {
            self.trim_lower_bound(Self::TILDES);
        }
        self.stack.push(element)
    }
}

#[derive(Debug, Clone)]
enum ReferenceScan<'a> {
    LinkLabel(CowStr<'a>, usize),
    Collapsed(Option<TreeIndex>),
    UnexpectedFootnote,
    Failed,
}

fn scan_nodes_to_index(
    tree: &Tree<Item>,
    mut node: Option<TreeIndex>,
    index: usize,
) -> Option<TreeIndex> {
    while let Some(node_index) = node {
        if tree[node_index].item.end <= index {
            node = tree[node_index].next;
        } else {
            break;
        }
    }
    node
}

fn scan_link_label<'text>(
    tree: &Tree<Item>,
    text: &'text str,
    options: Options,
) -> Option<(usize, ReferenceLabel<'text>)> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'[' {
        return None;
    }
    let line_break_handler = |bytes: &[u8]| Some(skip_container_prefixes(tree, bytes, options));
    if options.contains(Options::ENABLE_FOOTNOTES)
        && b'^' == bytes[1]
        && bytes.get(2) != Some(&b']')
        && let Some((byte_index, string)) =
            scan_link_label_rest(&text[2..], &|_| None, tree.is_in_table())
    {
        return Some((byte_index + 2, ReferenceLabel::Footnote(string)));
    }
    let (byte_index, string) =
        scan_link_label_rest(&text[1..], &line_break_handler, tree.is_in_table())?;
    Some((byte_index + 1, ReferenceLabel::Link(string)))
}

fn scan_reference<'b>(
    tree: &Tree<Item>,
    text: &'b str,
    current: Option<TreeIndex>,
    options: Options,
) -> ReferenceScan<'b> {
    let current_index = match current {
        None => return ReferenceScan::Failed,
        Some(current_index) => current_index,
    };
    let start = tree[current_index].item.start;
    let tail = &text.as_bytes()[start..];

    if tail.starts_with(b"[]") {
        let closing_node = tree[current_index].next.unwrap();
        ReferenceScan::Collapsed(tree[closing_node].next)
    } else {
        let label = scan_link_label(tree, &text[start..], options);
        match label {
            Some((index, ReferenceLabel::Link(label))) => {
                ReferenceScan::LinkLabel(label, start + index)
            }
            Some((_index, ReferenceLabel::Footnote(_label))) => ReferenceScan::UnexpectedFootnote,
            None => ReferenceScan::Failed,
        }
    }
}

#[derive(Clone, Default)]
struct LinkStack {
    inner: Vec<LinkStackElement>,
    disabled_index: usize,
}

impl LinkStack {
    fn push(&mut self, element: LinkStackElement) {
        self.inner.push(element);
    }

    fn pop(&mut self) -> Option<LinkStackElement> {
        let element = self.inner.pop();
        self.disabled_index = std::cmp::min(self.disabled_index, self.inner.len());
        element
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.disabled_index = 0;
    }

    fn disable_all_links(&mut self) {
        for element in &mut self.inner[self.disabled_index..] {
            if element.kind == LinkStackKind::Link {
                element.kind = LinkStackKind::Disabled;
            }
        }
        self.disabled_index = self.inner.len();
    }
}

#[derive(Clone, Debug)]
struct LinkStackElement {
    node: TreeIndex,
    kind: LinkStackKind,
}

#[derive(PartialEq, Clone, Debug)]
enum LinkStackKind {
    Link,
    Image,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct LinkDefinition<'a> {
    pub destination: CowStr<'a>,
    pub title: Option<CowStr<'a>>,
    pub span: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct FootnoteUse {
    pub use_count: usize,
}

struct CodeDelimiters {
    inner: HashMap<usize, VecDeque<TreeIndex>>,
    seen_first: bool,
}

impl CodeDelimiters {
    fn new() -> Self {
        Self {
            inner: Default::default(),
            seen_first: false,
        }
    }

    fn insert(&mut self, count: usize, index: TreeIndex) {
        if self.seen_first {
            self.inner.entry(count).or_default().push_back(index);
        } else {
            self.seen_first = true;
        }
    }

    fn is_populated(&self) -> bool {
        !self.inner.is_empty()
    }

    fn find(&mut self, open_index: TreeIndex, count: usize) -> Option<TreeIndex> {
        while let Some(index) = self.inner.get_mut(&count)?.pop_front() {
            if index > open_index {
                return Some(index);
            }
        }
        None
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.seen_first = false;
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct LinkIndex(usize);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct StringIndex(usize);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct AlignmentIndex(usize);

#[derive(Clone)]
pub(crate) struct Allocations<'a> {
    pub reference_definitions: ReferenceDefinitions<'a>,
    pub footnote_definitions: FootnoteDefinitions<'a>,
    links: Vec<(LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>)>,
    strings: Vec<CowStr<'a>>,
    alignments: Vec<Vec<Alignment>>,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct ReferenceDefinitions<'input>(
    pub(crate) HashMap<LinkLabel<'input>, LinkDefinition<'input>>,
);

#[derive(Clone, Default, Debug)]
pub(crate) struct FootnoteDefinitions<'input>(pub(crate) HashMap<LinkLabel<'input>, FootnoteUse>);

impl<'input> ReferenceDefinitions<'input> {
    pub fn get(&self, key: CowStr<'input>) -> Option<&LinkDefinition<'input>> {
        self.0.get(&LinkLabel::new(key))
    }
}

impl<'input> FootnoteDefinitions<'input> {
    pub fn contains(&self, key: CowStr<'input>) -> bool {
        self.0.contains_key(&LinkLabel::new(key))
    }

    pub fn get_mut(&mut self, key: CowStr<'input>) -> Option<&mut FootnoteUse> {
        self.0.get_mut(&LinkLabel::new(key))
    }
}

impl<'a> Allocations<'a> {
    pub fn new() -> Self {
        Self {
            reference_definitions: ReferenceDefinitions::default(),
            footnote_definitions: FootnoteDefinitions::default(),
            links: Vec::with_capacity(LINK_CAPACITY),
            strings: Vec::new(),
            alignments: Vec::new(),
        }
    }

    pub fn allocate_string(&mut self, string: CowStr<'a>) -> StringIndex {
        let index = self.strings.len();
        self.strings.push(string);
        StringIndex(index)
    }

    pub fn allocate_link(
        &mut self,
        kind: LinkType,
        url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    ) -> LinkIndex {
        let index = self.links.len();
        self.links.push((kind, url, title, id));
        LinkIndex(index)
    }

    pub fn allocate_alignment(&mut self, alignment: Vec<Alignment>) -> AlignmentIndex {
        let index = self.alignments.len();
        self.alignments.push(alignment);
        AlignmentIndex(index)
    }

    pub fn take_string(&mut self, index: StringIndex) -> CowStr<'a> {
        std::mem::replace(&mut self.strings[index.0], "".into())
    }

    pub fn take_link(
        &mut self,
        index: LinkIndex,
    ) -> (LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>) {
        let default_link = (LinkType::ShortcutUnknown, "".into(), "".into(), "".into());
        std::mem::replace(&mut self.links[index.0], default_link)
    }

    pub fn take_alignment(&mut self, index: AlignmentIndex) -> Vec<Alignment> {
        std::mem::take(&mut self.alignments[index.0])
    }
}

impl<'a> Index<StringIndex> for Allocations<'a> {
    type Output = CowStr<'a>;

    fn index(&self, index: StringIndex) -> &Self::Output {
        self.strings.index(index.0)
    }
}

impl<'a> Index<LinkIndex> for Allocations<'a> {
    type Output = (LinkType, CowStr<'a>, CowStr<'a>, CowStr<'a>);

    fn index(&self, index: LinkIndex) -> &Self::Output {
        self.links.index(index.0)
    }
}

impl<'a> Index<AlignmentIndex> for Allocations<'a> {
    type Output = Vec<Alignment>;

    fn index(&self, index: AlignmentIndex) -> &Self::Output {
        self.alignments.index(index.0)
    }
}

#[derive(Clone, Default)]
pub(crate) struct HtmlScanGuard {
    pub cdata: usize,
    pub processing: usize,
    pub declaration: usize,
    pub comment: usize,
}

pub struct OffsetIter<'a> {
    inner: Parser<'a>,
}

impl<'a> Iterator for OffsetIter<'a> {
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next_event()
    }
}

impl FusedIterator for OffsetIter<'_> {}

impl<'a> Iterator for Parser<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        self.next_event().map(|(event, _)| event)
    }
}

impl FusedIterator for Parser<'_> {}

impl<'a> Parser<'a> {
    fn next_event(&mut self) -> Option<(Event<'a>, Range<usize>)> {
        loop {
            let Some(current_index) = self.tree.current() else {
                let index = self.tree.pop()?;
                if matches!(self.tree[index].item.body, ItemBody::TightParagraph) {
                    self.tree.next_sibling(index);
                    continue;
                }
                let tag_end = body_to_tag_end(&self.tree[index].item.body);
                self.tree.next_sibling(index);
                let span = self.tree[index].item.start..self.tree[index].item.end;
                assert!(span.start <= span.end);
                return Some((Event::End(tag_end), span));
            };
            let current_index =
                if matches!(self.tree[current_index].item.body, ItemBody::TightParagraph) {
                    self.tree.push();
                    let Some(child) = self.tree.current() else {
                        continue;
                    };
                    child
                } else {
                    current_index
                };
            if self.tree[current_index].item.body.is_maybe_inline() {
                self.handle_inline();
            }
            let item = self.tree[current_index].item;
            let event = item_to_event(item, self.text, &mut self.allocations);
            if let Event::Start(..) = event {
                self.tree.push();
            } else {
                self.tree.next_sibling(current_index);
            }
            assert!(item.start <= item.end);
            return Some((event, item.start..item.end));
        }
    }
}
fn body_to_tag_end(body: &ItemBody) -> TagEnd {
    match *body {
        ItemBody::Paragraph => TagEnd::Paragraph,
        ItemBody::Emphasis => TagEnd::Emphasis,
        ItemBody::Strong => TagEnd::Strong,
        ItemBody::Strikethrough => TagEnd::Strikethrough,
        ItemBody::Link(..) => TagEnd::Link,
        ItemBody::Image(..) => TagEnd::Image,
        ItemBody::Heading(level) => TagEnd::Heading(level),
        ItemBody::IndentCodeBlock | ItemBody::FencedCodeBlock(..) => TagEnd::CodeBlock,
        ItemBody::BlockQuote(kind) => TagEnd::BlockQuote(kind),
        ItemBody::HtmlBlock => TagEnd::HtmlBlock,
        ItemBody::List(_, byte, _) => {
            let is_ordered = byte == b'.' || byte == b')';
            TagEnd::List(is_ordered)
        }
        ItemBody::ListItem(_) => TagEnd::Item,
        ItemBody::TableHead => TagEnd::TableHead,
        ItemBody::TableCell => TagEnd::TableCell,
        ItemBody::TableRow => TagEnd::TableRow,
        ItemBody::Table(..) => TagEnd::Table,
        ItemBody::FootnoteDefinition(..) => TagEnd::FootnoteDefinition,
        _ => panic!("unexpected item body {:?}", body),
    }
}

fn item_to_event<'a>(item: Item, text: &'a str, allocations: &mut Allocations<'a>) -> Event<'a> {
    let tag = match item.body {
        ItemBody::Text { .. } => return Event::Text(text[item.start..item.end].into()),
        ItemBody::Code(string_index) => return Event::Code(allocations.take_string(string_index)),
        ItemBody::SynthesizeText(string_index) => {
            return Event::Text(allocations.take_string(string_index));
        }
        ItemBody::SynthesizeChar(byte) => return Event::Text(byte.into()),
        ItemBody::HtmlBlock => Tag::HtmlBlock,
        ItemBody::Html => return Event::Html(text[item.start..item.end].into()),
        ItemBody::InlineHtml => return Event::InlineHtml(text[item.start..item.end].into()),
        ItemBody::OwnedInlineHtml(string_index) => {
            return Event::InlineHtml(allocations.take_string(string_index));
        }
        ItemBody::SoftBreak => return Event::SoftBreak,
        ItemBody::HardBreak(_) => return Event::HardBreak,
        ItemBody::FootnoteReference(string_index) => {
            return Event::FootnoteReference(allocations.take_string(string_index));
        }
        ItemBody::TaskListMarker(checked) => return Event::TaskListMarker(checked),
        ItemBody::Rule => return Event::Rule,
        ItemBody::Paragraph => Tag::Paragraph,
        ItemBody::Emphasis => Tag::Emphasis,
        ItemBody::Strong => Tag::Strong,
        ItemBody::Strikethrough => Tag::Strikethrough,
        ItemBody::Link(link_index) => {
            let (link_type, dest_url, title, id) = allocations.take_link(link_index);
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }
        }
        ItemBody::Image(link_index) => {
            let (link_type, dest_url, title, id) = allocations.take_link(link_index);
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }
        }
        ItemBody::Heading(level) => Tag::Heading { level },
        ItemBody::FencedCodeBlock(string_index) => {
            Tag::CodeBlock(CodeBlockKind::Fenced(allocations.take_string(string_index)))
        }
        ItemBody::IndentCodeBlock => Tag::CodeBlock(CodeBlockKind::Indented),
        ItemBody::BlockQuote(kind) => Tag::BlockQuote(kind),
        ItemBody::List(_, byte, list_item_start) => {
            if byte == b'.' || byte == b')' {
                Tag::List(Some(list_item_start))
            } else {
                Tag::List(None)
            }
        }
        ItemBody::ListItem(_) => Tag::Item,
        ItemBody::TableHead => Tag::TableHead,
        ItemBody::TableCell => Tag::TableCell,
        ItemBody::TableRow => Tag::TableRow,
        ItemBody::Table(alignment_index) => Tag::Table(allocations.take_alignment(alignment_index)),
        ItemBody::FootnoteDefinition(string_index) => {
            Tag::FootnoteDefinition(allocations.take_string(string_index))
        }

        _ => panic!("unexpected item body {:?}", item.body),
    };

    Event::Start(tag)
}
