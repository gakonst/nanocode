mod case_fold;
mod entities;
mod first_pass;
mod label;
mod parse;
mod punctuation;
mod scan;
mod text;
mod tree;

pub use parse::{OffsetIter, Parser};
pub use text::{CowStr, InlineStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Options(u32);

impl Options {
    pub const ENABLE_TABLES: Self = Self(1 << 1);
    pub const ENABLE_FOOTNOTES: Self = Self(1 << 2);
    pub const ENABLE_STRIKETHROUGH: Self = Self(1 << 3);
    pub const ENABLE_TASKLISTS: Self = Self(1 << 4);
    pub const ENABLE_SMART_PUNCTUATION: Self = Self(1 << 5);
    pub const ENABLE_GFM: Self = Self(1 << 11);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for Options {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl std::ops::BitOrAssign for Options {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CodeBlockKind<'a> {
    Indented,
    Fenced(CowStr<'a>),
}

impl CodeBlockKind<'_> {
    pub const fn is_indented(&self) -> bool {
        matches!(*self, Self::Indented)
    }

    pub const fn is_fenced(&self) -> bool {
        matches!(*self, Self::Fenced(_))
    }

    pub fn into_static(self) -> CodeBlockKind<'static> {
        match self {
            Self::Indented => CodeBlockKind::Indented,
            Self::Fenced(language) => CodeBlockKind::Fenced(language.into_static()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockQuoteKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag<'a> {
    Paragraph,
    Heading {
        level: HeadingLevel,
    },
    BlockQuote(Option<BlockQuoteKind>),
    CodeBlock(CodeBlockKind<'a>),
    HtmlBlock,
    List(Option<u64>),
    Item,
    FootnoteDefinition(CowStr<'a>),
    Table(Vec<Alignment>),
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Link {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    },
    Image {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    },
}

impl Tag<'_> {
    pub const fn to_end(&self) -> TagEnd {
        match self {
            Self::Paragraph => TagEnd::Paragraph,
            Self::Heading { level } => TagEnd::Heading(*level),
            Self::BlockQuote(kind) => TagEnd::BlockQuote(*kind),
            Self::CodeBlock(_) => TagEnd::CodeBlock,
            Self::HtmlBlock => TagEnd::HtmlBlock,
            Self::List(start) => TagEnd::List(start.is_some()),
            Self::Item => TagEnd::Item,
            Self::FootnoteDefinition(_) => TagEnd::FootnoteDefinition,
            Self::Table(_) => TagEnd::Table,
            Self::TableHead => TagEnd::TableHead,
            Self::TableRow => TagEnd::TableRow,
            Self::TableCell => TagEnd::TableCell,
            Self::Emphasis => TagEnd::Emphasis,
            Self::Strong => TagEnd::Strong,
            Self::Strikethrough => TagEnd::Strikethrough,
            Self::Link { .. } => TagEnd::Link,
            Self::Image { .. } => TagEnd::Image,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TagEnd {
    Paragraph,
    Heading(HeadingLevel),
    BlockQuote(Option<BlockQuoteKind>),
    CodeBlock,
    HtmlBlock,
    List(bool),
    Item,
    FootnoteDefinition,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Link,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HeadingLevel {
    H1 = 1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkType {
    Inline,
    Reference,
    ReferenceUnknown,
    Collapsed,
    CollapsedUnknown,
    Shortcut,
    ShortcutUnknown,
    Autolink,
    Email,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    Start(Tag<'a>),
    End(TagEnd),
    Text(CowStr<'a>),
    Code(CowStr<'a>),
    Html(CowStr<'a>),
    InlineHtml(CowStr<'a>),
    FootnoteReference(CowStr<'a>),
    SoftBreak,
    HardBreak,
    Rule,
    TaskListMarker(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}
