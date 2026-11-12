use super::{
    grammar::{ClearAmount, Scope, Syntax, SyntaxSet},
    parser::{ParseError, ParseState, ScopeStackOp},
};
use crate::ui::style::{Color, Modifier, Style};

const ATOM_LEN_BITS: u16 = 3;
const UNSCORED: f64 = -1.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FontStyle {
    bits: u8,
}

impl FontStyle {
    pub const BOLD: Self = Self { bits: 1 };
    pub const UNDERLINE: Self = Self { bits: 2 };
    pub const ITALIC: Self = Self { bits: 4 };

    const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }
}

#[derive(Clone, Copy)]
struct StyleModifier {
    foreground: Color,
    font_style: Option<FontStyle>,
}

pub struct Theme {
    foreground: Color,
    selectors: Vec<(Scope, StyleModifier)>,
}

impl Theme {
    pub fn new(foreground: Color, rules: &[(&[&str], Color, Option<FontStyle>)]) -> Self {
        let mut selectors = Vec::new();
        for &(scopes, foreground, font_style) in rules {
            for scope in scopes {
                selectors.push((
                    Scope::parse(scope),
                    StyleModifier {
                        foreground,
                        font_style,
                    },
                ));
            }
        }
        selectors.sort_by_key(|selector| std::cmp::Reverse(selector.0.len()));
        Self {
            foreground,
            selectors,
        }
    }

    fn default_style(&self) -> ScoredStyle {
        ScoredStyle {
            foreground: (UNSCORED, self.foreground),
            font_style: (UNSCORED, FontStyle::default()),
        }
    }

    fn push(&self, current: &ScoredStyle, path: &[Scope]) -> ScoredStyle {
        let mut style = *current;
        let last = path[path.len() - 1];
        let depth =
            u16::try_from(path.len() - 1).expect("scope paths are shorter than 65536 scopes");
        let exponent = ATOM_LEN_BITS
            .checked_mul(depth)
            .expect("scope paths are shorter than 21846 scopes");
        let weight = f64::from(exponent).exp2();
        for (scope, modifier) in &self.selectors {
            if !scope.is_prefix_of(last) {
                continue;
            }
            let score = f64::from(scope.len()) * weight;
            if score > style.foreground.0 {
                style.foreground = (score, modifier.foreground);
            }
            if score > style.font_style.0
                && let Some(font_style) = modifier.font_style
            {
                style.font_style = (score, font_style);
            }
        }
        style
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScoredStyle {
    foreground: (f64, Color),
    font_style: (f64, FontStyle),
}

impl ScoredStyle {
    fn style(&self) -> Style {
        let font_style = self.font_style.1;
        let mut modifier = Modifier::empty();
        if font_style.contains(FontStyle::BOLD) {
            modifier.insert(Modifier::BOLD);
        }
        if font_style.contains(FontStyle::ITALIC) {
            modifier.insert(Modifier::ITALIC);
        }
        if font_style.contains(FontStyle::UNDERLINE) {
            modifier.insert(Modifier::UNDERLINED);
        }
        Style::default()
            .fg(self.foreground.1)
            .add_modifier(modifier)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HighlightState {
    styles: Vec<Style>,
    caches: Vec<ScoredStyle>,
    scopes: Vec<Scope>,
    cleared: Vec<Vec<Scope>>,
}

impl HighlightState {
    fn new(theme: &Theme) -> Self {
        let default = theme.default_style();
        Self {
            styles: vec![default.style()],
            caches: vec![default],
            scopes: Vec::new(),
            cleared: Vec::new(),
        }
    }

    fn push(&mut self, theme: &Theme, scope: Scope) {
        self.scopes.push(scope);
        let cache = match self.caches.last() {
            Some(previous) => theme.push(previous, &self.scopes),
            None => theme.push(&theme.default_style(), &self.scopes),
        };
        self.styles.push(cache.style());
        self.caches.push(cache);
    }

    fn pop(&mut self) {
        self.scopes.pop();
        self.styles.pop();
        self.caches.pop();
    }

    fn apply(&mut self, theme: &Theme, op: &ScopeStackOp) -> bool {
        match *op {
            ScopeStackOp::Push(scope) => self.push(theme, scope),
            ScopeStackOp::Pop(count) => {
                for _ in 0..count {
                    self.pop();
                }
            }
            ScopeStackOp::Clear(amount) => {
                let cleared = match amount {
                    ClearAmount::TopN(count) => {
                        let keep = self.scopes.len() - count.min(self.scopes.len());
                        self.scopes.split_off(keep)
                    }
                    ClearAmount::All => std::mem::take(&mut self.scopes),
                };
                for _ in 0..cleared.len() {
                    self.styles.pop();
                    self.caches.pop();
                }
                self.cleared.push(cleared);
            }
            ScopeStackOp::Restore => {
                let Some(restored) = self.cleared.pop() else {
                    return false;
                };
                for scope in restored {
                    self.push(theme, scope);
                }
            }
            ScopeStackOp::Noop => {}
        }
        true
    }
}

pub struct HighlightLines<'a> {
    theme: &'a Theme,
    parse_state: ParseState,
    highlight_state: HighlightState,
}

impl<'a> HighlightLines<'a> {
    pub fn new(syntax: &Syntax, theme: &'a Theme) -> Self {
        Self {
            theme,
            parse_state: ParseState::new(syntax),
            highlight_state: HighlightState::new(theme),
        }
    }

    pub const fn from_state(
        theme: &'a Theme,
        highlight_state: HighlightState,
        parse_state: ParseState,
    ) -> Self {
        Self {
            theme,
            parse_state,
            highlight_state,
        }
    }

    pub fn state(self) -> (HighlightState, ParseState) {
        (self.highlight_state, self.parse_state)
    }

    pub fn highlight_line<'b>(
        &mut self,
        line: &'b str,
        syntaxes: &SyntaxSet,
    ) -> Result<Vec<(Style, &'b str)>, ParseError> {
        let ops = self.parse_state.parse_line(line, syntaxes)?;
        let state = &mut self.highlight_state;
        let mut regions = Vec::with_capacity(ops.len() + 1);
        let mut position = 0;
        for (end, op) in ops
            .iter()
            .map(|(end, op)| (*end, op))
            .chain(std::iter::once((line.len(), &ScopeStackOp::Noop)))
        {
            let style = state
                .styles
                .last()
                .copied()
                .unwrap_or_else(|| Style::default().fg(Color::Reset));
            let text = &line[position..end];
            if !state.apply(self.theme, op) {
                return Ok(regions);
            }
            position = end;
            if !text.is_empty() {
                regions.push((style, text));
            }
        }
        Ok(regions)
    }
}
