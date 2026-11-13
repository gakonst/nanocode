use super::xproto::{
    ANY_PROPERTY_TYPE, ATOM_CARDINAL, ATOM_STRING, ATOM_WINDOW, ATOM_WM_CLASS, Atom, BUTTON_PRESS,
    BUTTON_RELEASE, Closer, Connection, KEY_PRESS, KEY_RELEASE, MAP_STATE_VIEWABLE, MOTION_NOTIFY,
    PROP_MODE_APPEND, PROPERTY_CHANGE_MASK, PROPERTY_NOTIFY, Result as XResult,
    SUBSTRUCTURE_NOTIFY_MASK, SUBSTRUCTURE_REDIRECT_MASK, Window,
};
use crate::{
    Button, Chord, Key,
    keys::x11::{ALT_L, CONTROL_L, SHIFT_L, SUPER_L, keysym},
};
use nanocode_sys::base64::STANDARD;
use std::{collections::HashMap, thread, time::Duration};

pub type Rect = (i32, i32, i32, i32);

const POINTER_ROOT: Window = 1;
const NO_WINDOW: Window = 0;
const REMAP_SETTLE: Duration = Duration::from_millis(100);
const DRAG_STEPS: i32 = 12;
const ORDERED_TIMES: u32 = 1 << 31;
const STARTUP_CHUNK: usize = 20;
const ACTIVE_SOURCE_PAGER: u32 = 2;
const WINDOW_LIST_WORDS: u32 = 4096;
const TEXT_PROPERTY_WORDS: u32 = 1024;

pub struct Display {
    conn: Connection,
    root: Window,
    screen: usize,
    size: (i32, i32),
    pid: Atom,
    clients: Atom,
    stacking: Atom,
    application: Atom,
    active: Atom,
    startup: [Atom; 2],
}

pub struct Guard {
    closer: Closer,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.closer.close();
    }
}

pub struct Keymap {
    pub min: u8,
    pub per: usize,
    pub syms: Vec<u32>,
}

pub struct Layout {
    pub bits: usize,
    pub pad: usize,
    pub masks: [u32; 3],
    pub msb: bool,
}

pub fn available() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty())
}

fn quoted(value: &str) -> String {
    let mut out = String::from('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

pub fn failed(error: impl std::fmt::Display) -> String {
    format!("the X server did not answer ({error})")
}

impl Keymap {
    pub fn find(&self, sym: u32) -> Option<(u8, bool)> {
        let at = |level: usize| {
            self.syms
                .chunks(self.per.max(1))
                .position(|codes| codes.get(level) == Some(&sym))
                .and_then(|index| u8::try_from(usize::from(self.min) + index).ok())
        };
        at(0)
            .map(|code| (code, false))
            .or_else(|| at(1).map(|code| (code, true)))
            .or_else(|| {
                let lower = (0x41..=0x5a).contains(&sym).then_some(sym + 0x20)?;
                Self::find(self, lower)
                    .filter(|(_, shift)| !shift)
                    .map(|(code, _)| (code, true))
            })
    }

    pub fn spare(&self) -> Option<u8> {
        self.syms
            .chunks(self.per.max(1))
            .rposition(|codes| codes.iter().all(|&sym| sym == 0))
            .and_then(|index| u8::try_from(usize::from(self.min) + index).ok())
    }
}

pub fn rgb(data: &[u8], width: usize, height: usize, layout: &Layout) -> Option<Vec<u8>> {
    let bytes = match layout.bits {
        16 => 2,
        24 => 3,
        32 => 4,
        _ => return None,
    };
    let pad = layout.pad.max(8);
    let stride = (width * layout.bits).div_ceil(pad) * pad / 8;
    if data.len() < stride * height {
        return None;
    }
    let channel = |pixel: u32, mask: u32| {
        if mask == 0 {
            return 0;
        }
        let value = u64::from((pixel & mask) >> mask.trailing_zeros());
        let top = u64::from(mask >> mask.trailing_zeros());
        (value * 255 / top) as u8
    };
    let mut out = Vec::with_capacity(width * height * 3);
    for row in data.chunks(stride).take(height) {
        for pixel in row[..width * bytes].chunks(bytes) {
            let pixel = pixel.iter().enumerate().fold(0u32, |sum, (index, &byte)| {
                let shift = if layout.msb { bytes - 1 - index } else { index };
                sum | u32::from(byte) << (8 * shift)
            });
            out.extend(layout.masks.map(|mask| channel(pixel, mask)));
        }
    }
    Some(out)
}

pub fn jpeg(width: u32, height: u32, rgb: &[u8], max: usize) -> Option<String> {
    [80, 50, 30].into_iter().find_map(|quality| {
        let bytes =
            nanocode_jpeg::encode(rgb, width, height, nanocode_jpeg::Color::Rgb, quality, None)
                .ok()?;
        Some(STANDARD.encode(bytes)).filter(|data| data.len() <= max)
    })
}

impl Display {
    pub fn open() -> Result<Self, String> {
        let conn = Connection::open()
            .map_err(|error| format!("the X display could not be opened ({error})"))?;
        let (root, screen) = (conn.setup.root, conn.screen);
        let size = (i32::from(conn.setup.width), i32::from(conn.setup.height));
        let atom = |name: &[u8]| conn.intern_atom(false, name).map_err(failed);
        let (pid, clients) = (atom(b"_NET_WM_PID")?, atom(b"_NET_CLIENT_LIST")?);
        let application = atom(b"_GTK_APPLICATION_ID")?;
        let (active, stacking) = (
            atom(b"_NET_ACTIVE_WINDOW")?,
            atom(b"_NET_CLIENT_LIST_STACKING")?,
        );
        let startup = [
            atom(b"_NET_STARTUP_INFO_BEGIN")?,
            atom(b"_NET_STARTUP_INFO")?,
        ];
        Ok(Self {
            conn,
            root,
            screen,
            size,
            pid,
            clients,
            stacking,
            application,
            active,
            startup,
        })
    }

    pub fn active(&self) -> Option<Window> {
        let cookie = self
            .conn
            .get_property(self.root, self.active, ATOM_WINDOW, 1)
            .ok()?;
        self.conn
            .property(cookie)
            .ok()?
            .value32()?
            .next()
            .filter(|&window| window != NO_WINDOW)
    }

    pub fn active_pid(&self) -> Option<u32> {
        self.owner(self.active()?).ok().flatten()
    }

    pub fn guard(user: Window, pid: u32) -> Option<Guard> {
        let display = Self::open().ok()?;
        let selected = display
            .conn
            .select_events(display.root, PROPERTY_CHANGE_MASK)
            .ok()?;
        display.conn.check(selected).ok()?;
        let closer = display.conn.closer().ok()?;
        thread::spawn(move || {
            while let Ok(event) = display.conn.event() {
                if event.kind == PROPERTY_NOTIFY
                    && event.window == display.root
                    && (event.atom == display.active || event.atom == display.stacking)
                    && display.took(user, pid)
                {
                    let _ = display.activate(user);
                }
            }
        });
        Some(Guard { closer })
    }

    pub fn took(&self, user: Window, pid: u32) -> bool {
        let active = self.active();
        active != Some(user) && self.active_pid() == Some(pid)
            || active == Some(user) && self.top_pid() == Some(pid)
    }

    pub fn top_pid(&self) -> Option<u32> {
        let cookie = self
            .conn
            .get_property(self.root, self.stacking, ATOM_WINDOW, WINDOW_LIST_WORDS)
            .ok()?;
        let top = self.conn.property(cookie).ok()?.value32()?.last()?;
        self.owner(top).ok().flatten()
    }

    pub fn announce(&self, name: &str, bin: &str, class: &str) -> Option<String> {
        self.broadcast(|now| {
            let past = now.wrapping_sub(ORDERED_TIMES / 2);
            let id = format!("nanocode-computer-{}-{now}_TIME{past}", std::process::id());
            let fields = [
                ("ID", id.as_str()),
                ("NAME", name),
                ("BIN", bin),
                ("WMCLASS", class),
            ]
            .map(|(key, value)| format!("{key}={}", quoted(value)))
            .join(" ");
            let message = format!("new: {fields} SCREEN={} TIMESTAMP={past}", self.screen);
            (id, message)
        })
    }

    pub fn finish(&self, id: &str) -> bool {
        self.broadcast(|_| ((), format!("remove: ID={}", quoted(id))))
            .is_some()
    }

    fn broadcast<T>(&self, message: impl FnOnce(u32) -> (T, String)) -> Option<T> {
        let window = self.messenger().ok()?;
        let sent = (|| {
            let (value, message) = message(self.now(window)?);
            let mut bytes = message.into_bytes();
            bytes.push(0);
            for (at, chunk) in bytes.chunks(STARTUP_CHUNK).enumerate() {
                let mut data = [0; STARTUP_CHUNK];
                data[..chunk.len()].copy_from_slice(chunk);
                let kind = self.startup[usize::from(at > 0)];
                self.conn.send_client_message(
                    self.root,
                    PROPERTY_CHANGE_MASK,
                    8,
                    window,
                    kind,
                    data,
                )?;
            }
            Ok::<_, super::xproto::Failure>(value)
        })();
        let _ = self.conn.destroy_window(window);
        self.conn.sync().ok()?;
        sent.ok()
    }

    fn messenger(&self) -> XResult<Window> {
        let window = self.conn.generate_id()?;
        let created = self
            .conn
            .create_input_only(window, self.root, PROPERTY_CHANGE_MASK)?;
        self.conn.check(created)?;
        Ok(window)
    }

    fn now(&self, window: Window) -> XResult<u32> {
        let kind = self.startup[0];
        let changed =
            self.conn
                .change_property(PROP_MODE_APPEND, window, kind, ATOM_STRING, &[])?;
        self.conn.check(changed)?;
        loop {
            let event = self.conn.event()?;
            if event.kind == PROPERTY_NOTIFY && event.window == window {
                return Ok(event.time);
            }
        }
    }

    pub fn activate(&self, window: Window) -> XResult<()> {
        let mut data = [0; STARTUP_CHUNK];
        data[..4].copy_from_slice(&ACTIVE_SOURCE_PAGER.to_le_bytes());
        self.conn.send_client_message(
            self.root,
            SUBSTRUCTURE_REDIRECT_MASK | SUBSTRUCTURE_NOTIFY_MASK,
            32,
            window,
            self.active,
            data,
        )?;
        self.conn.sync()
    }

    pub fn alive(&self) -> bool {
        self.conn.input_focus().is_ok()
    }

    pub fn atspi_address(&self) -> Option<String> {
        let atom = self.conn.intern_atom(true, b"AT_SPI_BUS").ok()?;
        let cookie = self
            .conn
            .get_property(self.root, atom, ATOM_STRING, TEXT_PROPERTY_WORDS)
            .ok()?;
        String::from_utf8(self.conn.property(cookie).ok()?.value)
            .ok()
            .filter(|address| !address.is_empty())
    }

    fn pid(&self, window: Window) -> XResult<Option<u32>> {
        let cookie = self.conn.get_property(window, self.pid, ATOM_CARDINAL, 1)?;
        let property = self.conn.property(cookie)?;
        Ok(property.value32().and_then(|mut values| values.next()))
    }

    fn owner(&self, mut window: Window) -> XResult<Option<u32>> {
        if window == self.root {
            return Ok(None);
        }
        loop {
            if let Some(pid) = self.pid(window)? {
                return Ok(Some(pid));
            }
            let (parent, _) = self.conn.query_tree(window)?;
            if parent == self.root || parent == NO_WINDOW {
                break;
            }
            window = parent;
        }
        for child in self.conn.query_tree(window)?.1 {
            if let Some(pid) = self.pid(child)? {
                return Ok(Some(pid));
            }
        }
        Ok(None)
    }

    pub fn focused(&self, pid: u32) -> XResult<bool> {
        let mut focus = self.conn.input_focus()?;
        if focus == POINTER_ROOT {
            focus = self.conn.pointer_child(self.root)?;
        }
        if focus == NO_WINDOW {
            return Ok(false);
        }
        Ok(self.owner(focus)? == Some(pid))
    }

    fn rect(&self, window: Window) -> XResult<Rect> {
        let (width, height) = self.conn.geometry(window)?;
        let (x, y) = self.conn.translate(window, self.root)?;
        Ok((
            i32::from(x),
            i32::from(y),
            i32::from(width),
            i32::from(height),
        ))
    }

    fn candidates(&self) -> XResult<Vec<Window>> {
        let cookie =
            self.conn
                .get_property(self.root, self.clients, ATOM_WINDOW, WINDOW_LIST_WORDS)?;
        let listed = self.conn.property(cookie)?;
        let mut candidates: Vec<Window> =
            listed.value32().map(Iterator::collect).unwrap_or_default();
        if candidates.is_empty() {
            for top in self.conn.query_tree(self.root)?.1 {
                candidates.push(top);
                candidates.extend(self.conn.query_tree(top)?.1);
            }
        }
        Ok(candidates)
    }

    pub fn classes(&self) -> XResult<HashMap<u32, Vec<String>>> {
        let text = |window, property| {
            self.conn
                .get_property(window, property, ANY_PROPERTY_TYPE, TEXT_PROPERTY_WORDS)
        };
        let mut asked = Vec::new();
        for window in self.candidates()? {
            asked.push((
                self.conn.get_property(window, self.pid, ATOM_CARDINAL, 1)?,
                text(window, self.application)?,
                text(window, ATOM_WM_CLASS)?,
            ));
        }
        let mut classes: HashMap<u32, Vec<String>> = HashMap::new();
        for (pid, application, class) in asked {
            let pid = self.conn.property(pid)?;
            let Some(pid) = pid.value32().and_then(|mut values| values.next()) else {
                continue;
            };
            let names = classes.entry(pid).or_default();
            for value in [
                self.conn.property(application)?.value,
                self.conn.property(class)?.value,
            ] {
                names.extend(
                    value
                        .split(|&byte| byte == 0)
                        .filter(|name| !name.is_empty())
                        .map(|name| String::from_utf8_lossy(name).into_owned()),
                );
            }
        }
        Ok(classes)
    }

    fn windows(&self, pid: u32) -> XResult<Vec<(Window, Rect)>> {
        let mut found = Vec::new();
        for window in self.candidates()? {
            if self.pid(window)? != Some(pid) {
                continue;
            }
            if self.conn.map_state(window)? == MAP_STATE_VIEWABLE {
                found.push((window, self.rect(window)?));
            }
        }
        Ok(found)
    }

    pub fn window(&self, pid: u32, hint: Option<Rect>) -> XResult<Option<(Window, Rect)>> {
        let windows = self.windows(pid)?;
        Ok(match hint {
            Some((x, y, width, height)) => windows.into_iter().min_by_key(|(_, rect)| {
                (rect.0 - x).abs()
                    + (rect.1 - y).abs()
                    + (rect.2 - width).abs()
                    + (rect.3 - height).abs()
            }),
            None => windows
                .into_iter()
                .max_by_key(|(_, rect)| i64::from(rect.2) * i64::from(rect.3)),
        })
    }

    fn keymap(&self) -> XResult<Keymap> {
        let keyboard = self.conn.keyboard()?;
        Ok(Keymap {
            min: self.conn.setup.min_keycode,
            per: usize::from(keyboard.per),
            syms: keyboard.syms,
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> XResult<()> {
        let sent = self.conn.fake_input(kind, detail, self.root, x, y)?;
        self.conn.check(sent)
    }

    pub fn press(&self, chord: Chord) -> Result<(), String> {
        self.chord(chord).map_err(failed)?
    }

    fn chord(&self, chord: Chord) -> XResult<Result<(), String>> {
        let map = self.keymap()?;
        let modifier = |sym: u32| map.find(sym).map(|(code, _)| code);
        let sym = keysym(chord.key);
        let (code, shift, remapped) = match map.find(sym) {
            Some((code, shift)) => (code, shift, false),
            None => {
                let Some(spare) = map.spare() else {
                    return Ok(Err(
                        "That key has no keycode on this keyboard and none is free to borrow, so nothing was sent.".into(),
                    ));
                };
                let remapped = self.conn.remap(spare, &[sym, sym])?;
                self.conn.check(remapped)?;
                self.conn.sync()?;
                thread::sleep(REMAP_SETTLE);
                (spare, false, true)
            }
        };
        let wanted = chord.modifiers;
        let held: Vec<u8> = [
            (wanted.command, SUPER_L),
            (wanted.control, CONTROL_L),
            (wanted.alt, ALT_L),
            (wanted.shift || shift, SHIFT_L),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .filter_map(|(_, sym)| modifier(sym))
        .collect();
        let sent = (|| {
            for &code in &held {
                self.fake(KEY_PRESS, code, 0, 0)?;
            }
            self.fake(KEY_PRESS, code, 0, 0)?;
            self.fake(KEY_RELEASE, code, 0, 0)?;
            for &code in held.iter().rev() {
                self.fake(KEY_RELEASE, code, 0, 0)?;
            }
            self.conn.sync()
        })();
        if remapped {
            thread::sleep(REMAP_SETTLE);
            let restored = self.conn.remap(code, &[0, 0])?;
            self.conn.check(restored)?;
            self.conn.sync()?;
        }
        sent.map(Ok)
    }

    pub fn type_keys(&self, text: &str) -> Result<(), String> {
        for c in text.chars() {
            let key = match c {
                '\n' => Key::Return,
                '\t' => Key::Tab,
                ' ' => Key::Space,
                c => Key::Char(c),
            };
            self.press(Chord {
                key,
                modifiers: crate::Modifiers::default(),
            })?;
        }
        Ok(())
    }

    pub fn origin(&self, pid: u32, hint: Option<Rect>) -> Result<(i32, i32), String> {
        let (_, (x, y, _, _)) = self
            .window(pid, hint)
            .map_err(failed)?
            .ok_or("the app's window is not on screen, so nothing ran.")?;
        Ok((x, y))
    }

    pub fn click(&self, (x, y): (i32, i32), button: Button, count: u32) -> Result<(), String> {
        let button = match button {
            Button::Left => 1,
            Button::Middle => 2,
            Button::Right => 3,
        };
        let point = |value: i32| {
            i16::try_from(value).map_err(|_| "That point is off screen, so nothing ran.")
        };
        let (x, y) = (point(x)?, point(y)?);
        (|| {
            self.fake(MOTION_NOTIFY, 0, x, y)?;
            for _ in 0..count.max(1) {
                self.fake(BUTTON_PRESS, button, x, y)?;
                self.fake(BUTTON_RELEASE, button, x, y)?;
            }
            self.conn.sync()
        })()
        .map_err(failed)
    }

    pub fn drag(&self, from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
        let point = |(x, y): (i32, i32)| -> Result<(i16, i16), String> {
            match (i16::try_from(x), i16::try_from(y)) {
                (Ok(x), Ok(y)) => Ok((x, y)),
                _ => Err("That point is off screen, so nothing ran.".into()),
            }
        };
        let (start, end) = (point(from)?, point(to)?);
        (|| {
            self.fake(MOTION_NOTIFY, 0, start.0, start.1)?;
            self.fake(BUTTON_PRESS, 1, start.0, start.1)?;
            self.conn.sync()?;
            for step in 1..=DRAG_STEPS {
                let along = |a: i16, b: i16| {
                    (i32::from(a) + (i32::from(b) - i32::from(a)) * step / DRAG_STEPS) as i16
                };
                self.fake(
                    MOTION_NOTIFY,
                    0,
                    along(start.0, end.0),
                    along(start.1, end.1),
                )?;
                self.conn.sync()?;
                thread::sleep(Duration::from_millis(10));
            }
            self.fake(BUTTON_RELEASE, 1, end.0, end.1)?;
            self.conn.sync()
        })()
        .map_err(failed)
    }

    pub fn capture(&self, window: Window, rect: Rect) -> Result<(u32, u32, Vec<u8>), String> {
        let (x, y, width, height) = rect;
        let (left, top) = (x.max(0), y.max(0));
        let (right, bottom) = ((x + width).min(self.size.0), (y + height).min(self.size.1));
        if right <= left || bottom <= top {
            return Err("the window is off screen".into());
        }
        let (width, height) = ((right - left) as u16, (bottom - top) as u16);
        let image = self
            .conn
            .image(window, (left - x) as i16, (top - y) as i16, width, height)
            .map_err(failed)?;
        let setup = &self.conn.setup;
        let format = setup
            .formats
            .iter()
            .find(|format| format.depth == image.depth)
            .ok_or("the window's pixel format is unknown")?;
        let masks = setup
            .visuals
            .iter()
            .find(|visual| visual.id == image.visual)
            .map_or([0xff_0000, 0xff00, 0xff], |visual| visual.masks);
        let layout = Layout {
            bits: usize::from(format.bits_per_pixel),
            pad: usize::from(format.scanline_pad),
            masks,
            msb: setup.image_msb_first,
        };
        let pixels = rgb(
            &image.data,
            usize::from(width),
            usize::from(height),
            &layout,
        )
        .ok_or("the window's pixel format is not one screenshots read")?;
        Ok((u32::from(width), u32::from(height), pixels))
    }
}
