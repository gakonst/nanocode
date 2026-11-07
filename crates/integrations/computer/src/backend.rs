use crate::{keys::Chord, state::Node, state::Selection};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub pointer: bool,
    pub screenshots: bool,
    pub diffs: bool,
    pub policy: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct App {
    pub pid: i64,
    pub name: String,
    pub id: String,
    pub aliases: Vec<String>,
    pub path: String,
    pub background: bool,
    pub windows: bool,
}

pub struct Read<E> {
    pub title: Option<String>,
    pub root: Option<Node<E>>,
    pub window: Option<E>,
    pub focused: Option<E>,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Primary,
    Menu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn parse(direction: &str) -> Self {
        match direction {
            "up" => Self::Up,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => Self::Down,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

pub enum Target<'a, E> {
    Point(f64, f64),
    Element(&'a E),
}

pub enum Pointer<'a, E> {
    Click {
        at: Target<'a, E>,
        button: Button,
        count: u32,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
    },
}

pub trait Backend: Send + 'static {
    type Element: Clone + Send + 'static;

    fn capabilities(&self) -> Capabilities;

    fn ready(&mut self, host: &str) -> Result<(), String>;

    fn apps(&mut self) -> Vec<App>;

    fn listing(&mut self) -> Vec<App> {
        self.apps()
    }

    fn installed(&mut self, wanted: &str) -> Option<App>;

    fn keys_by_bundle(&self) -> bool {
        false
    }

    fn watched(&mut self, watched: bool) {
        let _ = watched;
    }

    fn launch(&mut self, app: &App) -> Result<(), String>;

    fn launched(&mut self, app: &App) {
        let _ = app;
    }

    fn types_into(&mut self, element: &Self::Element) -> bool {
        let _ = element;
        false
    }

    fn read(&mut self, app: &App) -> Result<Read<Self::Element>, String>;

    fn alive(&mut self, element: &Self::Element) -> bool;

    fn focus_unchanged(&mut self, app: &App, then: Option<&Self::Element>) -> bool;

    fn press(&mut self, element: &Self::Element, press: Press) -> Result<String, String>;

    fn set_value(&mut self, element: &Self::Element, value: &str) -> Result<String, String>;

    fn value(&mut self, element: &Self::Element) -> Option<String>;

    fn select(
        &mut self,
        element: &Self::Element,
        value: &str,
        selection: Selection,
    ) -> Result<String, String>;

    fn scroll(
        &mut self,
        element: &Self::Element,
        direction: Direction,
        pages: f64,
    ) -> Result<String, String>;

    fn perform(&mut self, element: &Self::Element, action: &str) -> Result<String, String>;

    fn type_text(
        &mut self,
        app: &App,
        focused: &Self::Element,
        text: &str,
    ) -> Result<String, String>;

    fn press_key(&mut self, app: &App, chord: Chord) -> Result<String, String>;

    fn screenshot(
        &mut self,
        app: &App,
        window: &Self::Element,
        max: usize,
    ) -> Result<Option<String>, String>;

    fn pointer(
        &mut self,
        app: &App,
        window: Option<&Self::Element>,
        pointer: Pointer<'_, Self::Element>,
    ) -> Result<String, String> {
        let _ = (app, window, pointer);
        Err("This engine has no pointer, so nothing ran.".into())
    }
}
