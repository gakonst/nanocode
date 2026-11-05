use crate::{App, Backend, Capabilities, Chord, Direction, Press, Read, Selection};

const REFUSED: &str = "Computer use on Windows is not built yet (UI Automation is designed but not implemented), so nothing ran. Continue without it.";

pub struct Uia;

impl Backend for Uia {
    type Element = ();

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn ready(&mut self, _: &str) -> Result<(), String> {
        Err(REFUSED.into())
    }

    fn apps(&mut self) -> Vec<App> {
        Vec::new()
    }

    fn installed(&mut self, _: &str) -> Option<App> {
        None
    }

    fn launch(&mut self, _: &App) -> Result<(), String> {
        Err(REFUSED.into())
    }

    fn read(&mut self, _: &App) -> Result<Read<()>, String> {
        Err(REFUSED.into())
    }

    fn alive(&mut self, _: &()) -> bool {
        false
    }

    fn focus_unchanged(&mut self, _: &App, _: Option<&()>) -> bool {
        false
    }

    fn press(&mut self, _: &(), _: Press) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn set_value(&mut self, _: &(), _: &str) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn value(&mut self, _: &()) -> Option<String> {
        None
    }

    fn select(&mut self, _: &(), _: &str, _: Selection) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn scroll(&mut self, _: &(), _: Direction, _: f64) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn perform(&mut self, _: &(), _: &str) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn type_text(&mut self, _: &App, _: &(), _: &str) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn press_key(&mut self, _: &App, _: Chord) -> Result<String, String> {
        Err(REFUSED.into())
    }

    fn screenshot(&mut self, _: &App, _: &(), _: usize) -> Result<Option<String>, String> {
        Err(REFUSED.into())
    }
}
