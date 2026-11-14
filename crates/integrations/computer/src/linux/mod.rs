mod a11y;
mod act;
mod atspi;
mod dbus;
mod desktop;
mod identity;
mod wire;
mod x11;
mod xproto;

use crate::{
    App, Backend, Capabilities, Chord, Direction, Key, Pointer, Press, Read, Selection, Target,
};
use a11y::Interfaces;
use atspi::Element;
use dbus::Bus;
use nanocode_executor::{Builder, Handle, Runtime};
use nanocode_sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use std::{collections::HashMap, future::Future, time::Duration};

const NO_BUS: &str = "Computer use needs a desktop session with the AT-SPI accessibility bus (DISPLAY or WAYLAND_DISPLAY and at-spi2-core), so nothing ran.";
const NO_X: &str = "needs an X11 display (DISPLAY), which this session lacks, so nothing was sent.";
const NO_SHOT: &str = "screenshots need X11; the state above stands without one";

enum Exec {
    Shared(Handle),
    Own(Runtime),
}

struct Watch {
    changes: UnboundedReceiver<()>,
    sender: UnboundedSender<()>,
    activation: Duration,
}

#[derive(Default)]
pub struct AtSpi {
    exec: Option<Exec>,
    bus: Option<Bus>,
    display: Option<x11::Display>,
    apps: HashMap<i64, Vec<Element>>,
    entries: desktop::Index,
    unwatched: bool,
    before: Option<u32>,
    startup: Option<String>,
    guard: Option<(x11::Guard, bool)>,
    known: HashMap<String, (u32, String, Duration)>,
    activations: HashMap<i64, Duration>,
    registered: bool,
    watching: Option<Watch>,
}

impl AtSpi {
    pub fn new() -> Self {
        Self::default()
    }

    fn block<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        match self.exec.as_ref()? {
            Exec::Shared(handle) => Some(handle.block_on(work)),
            Exec::Own(runtime) => Some(runtime.block_on(work)),
        }
    }

    fn bus(&self) -> Result<Bus, String> {
        self.bus.clone().ok_or_else(|| NO_BUS.to_owned())
    }

    fn display(&mut self) -> Result<&x11::Display, String> {
        if !x11::available() {
            return Err(format!("This {NO_X}"));
        }
        if self
            .display
            .as_ref()
            .is_some_and(|display| !display.alive())
        {
            self.display = None;
        }
        if self.display.is_none() {
            self.display = Some(x11::Display::open()?);
        }
        self.display.as_ref().ok_or_else(|| NO_X.to_owned())
    }

    fn extents(&self, element: Option<&Element>) -> Option<x11::Rect> {
        let bus = self.bus().ok()?;
        self.block(atspi::extents(&bus, element?)).flatten()
    }

    fn origin(&mut self, app: &App, window: Option<&Element>) -> Result<(i32, i32), String> {
        let hint = self.extents(window);
        let pid = pid(app)?;
        self.display()?.origin(pid, hint)
    }

    fn grab(&mut self, element: &Element) -> bool {
        if self.unwatched {
            return true;
        }
        let pid = self
            .apps
            .iter()
            .find(|(_, elements)| elements.iter().any(|known| known.bus == element.bus))
            .and_then(|(&pid, _)| u32::try_from(pid).ok());
        x11::available()
            && pid.is_some_and(|pid| {
                self.display()
                    .ok()
                    .and_then(|display| display.focused(pid).ok())
                    .unwrap_or(false)
            })
    }

    fn list(&mut self, shown: bool) -> Vec<App> {
        let Ok(bus) = self.bus() else {
            return Vec::new();
        };
        let known = std::mem::take(&mut self.known);
        let registered = self
            .block(atspi::registered(&bus, &known, shown))
            .unwrap_or_default();
        for found in &registered {
            self.known.insert(
                found.element.bus.clone(),
                (found.pid, found.name.clone(), found.activation),
            );
        }
        let classes = if x11::available() {
            self.display()
                .ok()
                .and_then(|display| display.classes().ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let entries = self.entries.entries();
        let own = std::process::id();
        self.apps.clear();
        let mut apps: Vec<App> = Vec::new();
        for found in registered {
            if found.pid == own || found.pid == 0 {
                continue;
            }
            let pid = i64::from(found.pid);
            self.apps.entry(pid).or_default().push(found.element);
            let activation = self.activations.entry(pid).or_default();
            *activation = found.activation.max(*activation);
            if let Some(app) = apps.iter_mut().find(|app| app.pid == pid) {
                app.windows |= found.windows;
                continue;
            }
            let path = desktop::exe(found.pid).unwrap_or_default();
            let name = if found.name.trim().is_empty() {
                path.rsplit('/').next().unwrap_or_default().to_owned()
            } else {
                found.name
            };
            let window = classes.get(&found.pid).map_or(&[][..], Vec::as_slice);
            let mut aliases = identity::of(found.pid, &path, window, entries);
            let id = if aliases.is_empty() {
                name.clone()
            } else {
                aliases.remove(0)
            };
            apps.push(App {
                pid,
                name,
                id,
                aliases,
                path,
                background: false,
                windows: found.windows,
            });
        }
        apps
    }

    fn silent(app: &App) -> String {
        format!(
            "{} is not answering accessibility requests, so its state could not be read; it may be busy or hung. Read it again shortly.",
            app.name
        )
    }

    fn keys_allowed(&mut self, app: &App, what: &str) -> Result<(), String> {
        let pid = pid(app)?;
        let display = self.display().map_err(|_| format!("{what} {NO_X}"))?;
        if display.focused(pid).map_err(x11::failed)? {
            Ok(())
        } else {
            Err(format!(
                "{what} reaches only the window with keyboard focus, and {} doesn't have it, so nothing was sent.",
                app.name
            ))
        }
    }
}

impl Drop for AtSpi {
    fn drop(&mut self) {
        self.unwatch();
        if let Some(Exec::Own(runtime)) = self.exec.take() {
            runtime.shutdown_background();
        }
    }
}

fn pid(app: &App) -> Result<u32, String> {
    u32::try_from(app.pid)
        .ok()
        .filter(|&pid| pid > 0)
        .ok_or_else(|| format!("{} is not running, so nothing ran.", app.name))
}

fn label(chord: Chord) -> String {
    let modifiers = chord.modifiers;
    let mut parts: Vec<String> = [
        (modifiers.command, "super"),
        (modifiers.control, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.shift, "shift"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_owned())
    .collect();
    parts.push(match chord.key {
        Key::Char(c) => c.to_string(),
        Key::F(number) => format!("F{number}"),
        other => format!("{other:?}"),
    });
    parts.join("+")
}

impl Backend for AtSpi {
    type Element = Element;

    fn capabilities(&self) -> Capabilities {
        let x11 = x11::available();
        Capabilities {
            pointer: x11,
            screenshots: x11,
            diffs: false,
            policy: false,
        }
    }

    fn ready(&mut self, _: &str) -> Result<(), String> {
        self.guard = None;
        self.exec = match (Handle::try_current(), self.exec.take()) {
            (Ok(handle), Some(Exec::Own(runtime))) => {
                runtime.shutdown_background();
                Some(Exec::Shared(handle))
            }
            (Ok(handle), _) => Some(Exec::Shared(handle)),
            (Err(_), Some(Exec::Own(runtime))) => Some(Exec::Own(runtime)),
            (Err(_), _) => Some(Exec::Own(
                Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        format!("Computer use could not start ({error}), so nothing ran.")
                    })?,
            )),
        };
        if let Some(bus) = &self.bus
            && self.block(atspi::ping(bus)) == Some(true)
        {
            return Ok(());
        }
        self.bus = None;
        let fallback = if x11::available() {
            self.display().ok().and_then(x11::Display::atspi_address)
        } else {
            None
        };
        self.bus = self.block(atspi::connect(fallback)).flatten();
        self.registered = match &self.bus {
            Some(bus) => self.block(atspi::register(bus)) == Some(true),
            None => false,
        };
        self.bus
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| NO_BUS.to_owned())
    }

    fn apps(&mut self) -> Vec<App> {
        self.list(false)
    }

    fn listing(&mut self) -> Vec<App> {
        self.list(true)
    }

    fn installed(&mut self, wanted: &str) -> Option<App> {
        desktop::installed(self.entries.entries(), wanted)
    }

    fn keys_by_bundle(&self) -> bool {
        true
    }

    fn watched(&mut self, watched: bool) {
        self.unwatched = !watched;
    }

    fn launch(&mut self, app: &App) -> Result<(), String> {
        let display = if self.unwatched || !x11::available() {
            None
        } else {
            self.display().ok()
        };
        let before = display.and_then(x11::Display::active);
        let (startup, child) =
            desktop::launch(app, |bin, class| display?.announce(&app.name, bin, class))?;
        self.guard = before
            .and_then(|before| x11::Display::guard(before, child))
            .map(|guard| (guard, false));
        (self.before, self.startup) = (before, startup);
        Ok(())
    }

    fn launched(&mut self, app: &App) {
        let Ok(pid) = pid(app) else {
            return;
        };
        self.guard = match self.guard.take() {
            Some((guard, false)) => Some((guard, true)),
            _ => None,
        };
        let (before, startup) = (self.before, self.startup.take());
        let Ok(display) = self.display() else {
            return;
        };
        if let Some(before) = before
            && display.took(before, pid)
        {
            let _ = display.activate(before);
        }
        let finished = startup.as_deref().is_some_and(|id| {
            matches!(display.window(pid, None), Ok(Some(_))) && display.finish(id)
        });
        if !finished {
            self.startup = startup;
        }
    }

    fn watch(&mut self, app: &App) -> bool {
        self.unwatch();
        let Ok(bus) = self.bus() else {
            return false;
        };
        let senders: Vec<String> = self
            .apps
            .get(&app.pid)
            .map(|elements| elements.iter().map(|element| element.bus.clone()).collect())
            .unwrap_or_default();
        if !self.registered || senders.is_empty() {
            return false;
        }
        let (sender, changes) = mpsc::unbounded_channel();
        let watched = self.block(async {
            for watched in &senders {
                atspi::changes(&bus, watched, sender.clone()).await?;
            }
            Some(())
        });
        if watched != Some(Some(())) {
            let _ = bus.forget(sender);
            return false;
        }
        self.watching = Some(Watch {
            changes,
            sender,
            activation: self.activations.get(&app.pid).copied().unwrap_or_default(),
        });
        true
    }

    fn settle(&mut self, first: Duration, quiet: Duration) -> bool {
        let Some(mut watch) = self.watching.take() else {
            return false;
        };
        let changed = self
            .block(atspi::settle(
                &mut watch.changes,
                if first.is_zero() {
                    first
                } else {
                    first + watch.activation
                },
                quiet,
            ))
            .unwrap_or(false);
        self.watching = Some(watch);
        changed
    }

    fn unwatch(&mut self) {
        if let Some(watch) = self.watching.take()
            && let Ok(bus) = self.bus()
        {
            let _ = bus.forget(watch.sender);
        }
    }

    fn types_into(&mut self, element: &Element) -> bool {
        let Ok(bus) = self.bus() else {
            return false;
        };
        self.block(atspi::interfaces(&bus, element))
            .is_some_and(|interfaces| interfaces.contains(Interfaces::EDITABLE_TEXT))
    }

    fn read(&mut self, app: &App) -> Result<Read<Element>, String> {
        let bus = self.bus()?;
        let known = self.apps.get(&app.pid).cloned().unwrap_or_default();
        let mut found = self
            .block(atspi::window(&bus, &known))
            .ok_or(NO_BUS)?
            .map_err(|()| Self::silent(app))?;
        if found.is_none() {
            self.apps();
            let known = self.apps.get(&app.pid).cloned().unwrap_or_default();
            found = self
                .block(atspi::window(&bus, &known))
                .ok_or(NO_BUS)?
                .map_err(|()| Self::silent(app))?;
        }
        let Some((window, title)) = found else {
            return Ok(Read {
                title: None,
                root: None,
                window: None,
                focused: None,
                complete: true,
                note: None,
            });
        };
        let tree = self.block(atspi::tree(&bus, &window)).ok_or(NO_BUS)?;
        Ok(Read {
            title,
            root: tree.root,
            window: Some(window),
            focused: tree.focused,
            complete: tree.complete,
            note: None,
        })
    }

    fn alive(&mut self, element: &Element) -> bool {
        let Ok(bus) = self.bus() else {
            return false;
        };
        self.block(atspi::states(&bus, element))
            .flatten()
            .is_some_and(atspi::listed)
    }

    fn focus_unchanged(&mut self, _: &App, then: Option<&Element>) -> bool {
        let Some(then) = then else {
            return true;
        };
        let Ok(bus) = self.bus() else {
            return false;
        };
        self.block(act::focused(&bus, then)).unwrap_or(false)
    }

    fn press(&mut self, element: &Element, press: Press) -> Result<String, String> {
        let bus = self.bus()?;
        let grab = self.grab(element);
        self.block(act::press(&bus, element, press, grab))
            .ok_or(NO_BUS)?
    }

    fn set_value(&mut self, element: &Element, value: &str) -> Result<String, String> {
        let bus = self.bus()?;
        self.block(act::set_value(&bus, element, value))
            .ok_or(NO_BUS)?
    }

    fn value(&mut self, element: &Element) -> Option<String> {
        let bus = self.bus().ok()?;
        self.block(act::value(&bus, element)).flatten()
    }

    fn select(
        &mut self,
        element: &Element,
        value: &str,
        selection: Selection,
    ) -> Result<String, String> {
        let bus = self.bus()?;
        self.block(act::select(&bus, element, value, selection))
            .ok_or(NO_BUS)?
    }

    fn scroll(
        &mut self,
        element: &Element,
        direction: Direction,
        pages: f64,
    ) -> Result<String, String> {
        let bus = self.bus()?;
        self.block(act::scroll(&bus, element, direction, pages))
            .ok_or(NO_BUS)?
    }

    fn perform(&mut self, element: &Element, action: &str) -> Result<String, String> {
        let bus = self.bus()?;
        self.block(act::perform(&bus, element, action))
            .ok_or(NO_BUS)?
    }

    fn type_text(&mut self, app: &App, focused: &Element, text: &str) -> Result<String, String> {
        let bus = self.bus()?;
        if self
            .block(act::type_text(&bus, focused, text))
            .ok_or(NO_BUS)??
        {
            return Ok("Typed text.".into());
        }
        self.keys_allowed(app, "Typing into an element without editable text")?;
        self.display()?.type_keys(text)?;
        Ok("Typed text as keys.".into())
    }

    fn press_key(&mut self, app: &App, chord: Chord) -> Result<String, String> {
        self.keys_allowed(app, "press_key")?;
        self.display()?.press(chord)?;
        Ok(format!("Pressed {}.", label(chord)))
    }

    fn screenshot(
        &mut self,
        app: &App,
        window: &Element,
        max: usize,
    ) -> Result<Option<String>, String> {
        if !x11::available() {
            return Err(NO_SHOT.into());
        }
        let hint = self.extents(Some(window));
        let pid = pid(app)?;
        let display = self.display()?;
        let (xid, rect) = display
            .window(pid, hint)
            .map_err(x11::failed)?
            .ok_or("the window could not be found on screen")?;
        let (width, height, pixels) = display.capture(xid, rect)?;
        Ok(x11::jpeg(width, height, &pixels, max))
    }

    fn pointer(
        &mut self,
        app: &App,
        window: Option<&Element>,
        pointer: Pointer<'_, Element>,
    ) -> Result<String, String> {
        match pointer {
            Pointer::Click {
                at: Target::Point(x, y),
                button,
                count,
            } => {
                let (left, top) = self.origin(app, window)?;
                let at = (left + x.round() as i32, top + y.round() as i32);
                self.display()?.click(at, button, count)?;
                Ok(format!("Clicked at ({x:.0}, {y:.0})."))
            }
            Pointer::Click {
                at: Target::Element(element),
                button,
                count,
            } => {
                let bus = self.bus()?;
                let at = self
                    .block(act::center(&bus, element))
                    .flatten()
                    .ok_or("That element has no place on screen, so nothing ran.")?;
                self.display()?.click(at, button, count)?;
                Ok("Clicked.".into())
            }
            Pointer::Drag { from, to } => {
                let (left, top) = self.origin(app, window)?;
                let point = |(x, y): (f64, f64)| (left + x.round() as i32, top + y.round() as i32);
                self.display()?.drag(point(from), point(to))?;
                Ok("Dragged.".into())
            }
        }
    }
}
