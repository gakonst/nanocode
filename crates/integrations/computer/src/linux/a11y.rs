#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Role(u32);

impl Role {
    pub const ALERT: Self = Self(2);
    pub const COMBO_BOX: Self = Self(11);
    pub const DIALOG: Self = Self(16);
    pub const FRAME: Self = Self(23);
    pub const LIST: Self = Self(31);
    pub const MENU: Self = Self(33);
    pub const PASSWORD_TEXT: Self = Self(40);
    pub const PUSH_BUTTON: Self = Self(43);
    pub const SCROLL_BAR: Self = Self(48);
    pub const SCROLL_PANE: Self = Self(49);
    pub const TERMINAL: Self = Self(60);
    pub const TEXT: Self = Self(61);
    pub const UNKNOWN: Self = Self(67);
    pub const WINDOW: Self = Self(69);
    pub const APPLICATION: Self = Self(75);
    pub const ENTRY: Self = Self(79);

    pub fn of(value: u32) -> Self {
        if usize::try_from(value).is_ok_and(|value| value < ROLE_NAMES.len()) {
            Self(value)
        } else {
            Self::UNKNOWN
        }
    }

    pub const fn name(self) -> &'static str {
        ROLE_NAMES[self.0 as usize]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct States(u64);

impl States {
    pub const ACTIVE: u32 = 1;
    pub const CHECKED: u32 = 4;
    pub const DEFUNCT: u32 = 6;
    pub const ENABLED: u32 = 8;
    pub const EXPANDABLE: u32 = 9;
    pub const EXPANDED: u32 = 10;
    pub const FOCUSED: u32 = 12;
    pub const HORIZONTAL: u32 = 14;
    pub const SELECTABLE: u32 = 22;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const VERTICAL: u32 = 29;
    pub const VISIBLE: u32 = 30;
    const DEFINED: u32 = 44;
    const WORDS: usize = 2;

    pub fn of(words: &[u32]) -> Option<Self> {
        if words.len() != Self::WORDS {
            return None;
        }
        let bits = u64::from(words[0]) | u64::from(words[1]) << 32;
        (bits >> Self::DEFINED == 0).then_some(Self(bits))
    }

    pub const fn contains(self, state: u32) -> bool {
        self.0 & 1 << state != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Interfaces(u8);

impl Interfaces {
    pub const ACTION: u8 = 1 << 0;
    pub const EDITABLE_TEXT: u8 = 1 << 1;
    pub const SELECTION: u8 = 1 << 2;
    pub const TEXT: u8 = 1 << 3;
    pub const VALUE: u8 = 1 << 4;
    const NAMES: [(&str, u8); 5] = [
        ("org.a11y.atspi.Action", Self::ACTION),
        ("org.a11y.atspi.EditableText", Self::EDITABLE_TEXT),
        ("org.a11y.atspi.Selection", Self::SELECTION),
        ("org.a11y.atspi.Text", Self::TEXT),
        ("org.a11y.atspi.Value", Self::VALUE),
    ];

    pub fn of<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self(
            names
                .into_iter()
                .filter_map(|name| Self::NAMES.iter().find(|(known, _)| *known == name))
                .fold(0, |bits, (_, bit)| bits | bit),
        )
    }

    pub const fn contains(self, interface: u8) -> bool {
        self.0 & interface != 0
    }
}

const ROLE_NAMES: [&str; 130] = [
    "invalid",
    "accelerator label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check box",
    "check menu item",
    "color chooser",
    "column header",
    "combo box",
    "date editor",
    "desktop icon",
    "desktop frame",
    "dial",
    "dialog",
    "directory pane",
    "drawing area",
    "file chooser",
    "filler",
    "focus traversable",
    "font chooser",
    "frame",
    "glass pane",
    "html container",
    "icon",
    "image",
    "internal frame",
    "label",
    "layered pane",
    "list",
    "list item",
    "menu",
    "menu bar",
    "menu item",
    "option pane",
    "page tab",
    "page tab list",
    "panel",
    "password text",
    "popup menu",
    "progress bar",
    "button",
    "radio button",
    "radio menu item",
    "root pane",
    "row header",
    "scroll bar",
    "scroll pane",
    "separator",
    "slider",
    "spin button",
    "split pane",
    "status bar",
    "table",
    "table cell",
    "table column header",
    "table row header",
    "tearoff menu item",
    "terminal",
    "text",
    "toggle button",
    "tool bar",
    "tool tip",
    "tree",
    "tree table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document frame",
    "heading",
    "page",
    "section",
    "redundant object",
    "form",
    "link",
    "input method window",
    "table row",
    "tree item",
    "document spreadsheet",
    "document presentation",
    "document text",
    "document web",
    "document email",
    "comment",
    "list box",
    "grouping",
    "image map",
    "notification",
    "info bar",
    "level bar",
    "title bar",
    "block quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math fraction",
    "math root",
    "subscript",
    "superscript",
    "description list",
    "description term",
    "description value",
    "footnote",
    "content deletion",
    "content insertion",
    "mark",
    "suggestion",
    "push button menu",
];
