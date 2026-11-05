use crate::{
    Button, Chord, Key,
    keys::x11::{ALT_L, CONTROL_L, SHIFT_L, SUPER_L, keysym},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use std::{collections::HashMap, thread, time::Duration};
use x11rb::{
    connection::Connection as _,
    errors::ReplyError,
    protocol::{
        xproto::{
            AtomEnum, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, ImageFormat,
            ImageOrder, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, MapState, Window,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

pub type Rect = (i32, i32, i32, i32);

const POINTER_ROOT: Window = 1;
const REMAP_SETTLE: Duration = Duration::from_millis(100);
const DRAG_STEPS: i32 = 12;

pub struct Display {
    conn: RustConnection,
    root: Window,
    size: (i32, i32),
    pid: u32,
    clients: u32,
    application: u32,
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
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode(rgb, width, height, ExtendedColorType::Rgb8)
            .ok()?;
        Some(STANDARD.encode(bytes)).filter(|data| data.len() <= max)
    })
}

impl Display {
    pub fn open() -> Result<Self, String> {
        let (conn, screen) = RustConnection::connect(None)
            .map_err(|error| format!("the X display could not be opened ({error})"))?;
        let (root, size) = {
            let screen = conn
                .setup()
                .roots
                .get(screen)
                .ok_or("the X display has no such screen")?;
            (
                screen.root,
                (
                    i32::from(screen.width_in_pixels),
                    i32::from(screen.height_in_pixels),
                ),
            )
        };
        let atom = |name: &[u8]| -> Result<u32, String> {
            Ok(conn
                .intern_atom(false, name)
                .map_err(failed)?
                .reply()
                .map_err(failed)?
                .atom)
        };
        let (pid, clients) = (atom(b"_NET_WM_PID")?, atom(b"_NET_CLIENT_LIST")?);
        let application = atom(b"_GTK_APPLICATION_ID")?;
        Ok(Self {
            conn,
            root,
            size,
            pid,
            clients,
            application,
        })
    }

    pub fn alive(&self) -> bool {
        self.conn
            .get_input_focus()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some()
    }

    pub fn atspi_address(&self) -> Option<String> {
        let atom = self
            .conn
            .intern_atom(true, b"AT_SPI_BUS")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let reply = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        String::from_utf8(reply.value)
            .ok()
            .filter(|address| !address.is_empty())
    }

    fn pid(&self, window: Window) -> Result<Option<u32>, ReplyError> {
        let reply = self
            .conn
            .get_property(false, window, self.pid, AtomEnum::CARDINAL, 0, 1)?
            .reply()?;
        Ok(reply.value32().and_then(|mut values| values.next()))
    }

    fn owner(&self, mut window: Window) -> Result<Option<u32>, ReplyError> {
        if window == self.root {
            return Ok(None);
        }
        loop {
            if let Some(pid) = self.pid(window)? {
                return Ok(Some(pid));
            }
            let parent = self.conn.query_tree(window)?.reply()?.parent;
            if parent == self.root || parent == 0 {
                break;
            }
            window = parent;
        }
        for child in self.conn.query_tree(window)?.reply()?.children {
            if let Some(pid) = self.pid(child)? {
                return Ok(Some(pid));
            }
        }
        Ok(None)
    }

    pub fn focused(&self, pid: u32) -> Result<bool, ReplyError> {
        let mut focus = self.conn.get_input_focus()?.reply()?.focus;
        if focus == POINTER_ROOT {
            focus = self.conn.query_pointer(self.root)?.reply()?.child;
        }
        if focus == 0 {
            return Ok(false);
        }
        Ok(self.owner(focus)? == Some(pid))
    }

    fn rect(&self, window: Window) -> Result<Rect, ReplyError> {
        let geometry = self.conn.get_geometry(window)?.reply()?;
        let origin = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)?
            .reply()?;
        Ok((
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            i32::from(geometry.width),
            i32::from(geometry.height),
        ))
    }

    fn candidates(&self) -> Result<Vec<Window>, ReplyError> {
        let listed = self
            .conn
            .get_property(false, self.root, self.clients, AtomEnum::WINDOW, 0, 4096)?
            .reply()?;
        let mut candidates: Vec<Window> =
            listed.value32().map(Iterator::collect).unwrap_or_default();
        if candidates.is_empty() {
            for top in self.conn.query_tree(self.root)?.reply()?.children {
                candidates.push(top);
                candidates.extend(self.conn.query_tree(top)?.reply()?.children);
            }
        }
        Ok(candidates)
    }

    pub fn classes(&self) -> Result<HashMap<u32, Vec<String>>, ReplyError> {
        let text = |window, property| {
            self.conn
                .get_property(false, window, property, AtomEnum::ANY, 0, 1024)
        };
        let mut asked = Vec::new();
        for window in self.candidates()? {
            asked.push((
                self.conn
                    .get_property(false, window, self.pid, AtomEnum::CARDINAL, 0, 1)?,
                text(window, self.application)?,
                text(window, AtomEnum::WM_CLASS.into())?,
            ));
        }
        let mut classes: HashMap<u32, Vec<String>> = HashMap::new();
        for (pid, application, class) in asked {
            let Some(pid) = pid.reply()?.value32().and_then(|mut values| values.next()) else {
                continue;
            };
            let names = classes.entry(pid).or_default();
            for value in [application.reply()?.value, class.reply()?.value] {
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

    fn windows(&self, pid: u32) -> Result<Vec<(Window, Rect)>, ReplyError> {
        let mut found = Vec::new();
        for window in self.candidates()? {
            if self.pid(window)? != Some(pid) {
                continue;
            }
            let attributes = self.conn.get_window_attributes(window)?.reply()?;
            if attributes.map_state == MapState::VIEWABLE {
                found.push((window, self.rect(window)?));
            }
        }
        Ok(found)
    }

    pub fn window(
        &self,
        pid: u32,
        hint: Option<Rect>,
    ) -> Result<Option<(Window, Rect)>, ReplyError> {
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

    fn keymap(&self) -> Result<Keymap, ReplyError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)?
            .reply()?;
        Ok(Keymap {
            min,
            per: usize::from(reply.keysyms_per_keycode),
            syms: reply.keysyms,
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), ReplyError> {
        self.conn
            .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)?
            .check()?;
        Ok(())
    }

    fn sync(&self) -> Result<(), ReplyError> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    pub fn press(&self, chord: Chord) -> Result<(), String> {
        self.chord(chord).map_err(failed)?
    }

    fn chord(&self, chord: Chord) -> Result<Result<(), String>, ReplyError> {
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
                self.conn
                    .change_keyboard_mapping(1, spare, 2, &[sym, sym])?
                    .check()?;
                self.sync()?;
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
                self.fake(KEY_PRESS_EVENT, code, 0, 0)?;
            }
            self.fake(KEY_PRESS_EVENT, code, 0, 0)?;
            self.fake(KEY_RELEASE_EVENT, code, 0, 0)?;
            for &code in held.iter().rev() {
                self.fake(KEY_RELEASE_EVENT, code, 0, 0)?;
            }
            self.sync()
        })();
        if remapped {
            thread::sleep(REMAP_SETTLE);
            self.conn
                .change_keyboard_mapping(1, code, 2, &[0, 0])?
                .check()?;
            self.sync()?;
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
            self.fake(MOTION_NOTIFY_EVENT, 0, x, y)?;
            for _ in 0..count.max(1) {
                self.fake(BUTTON_PRESS_EVENT, button, x, y)?;
                self.fake(BUTTON_RELEASE_EVENT, button, x, y)?;
            }
            self.sync()
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
            self.fake(MOTION_NOTIFY_EVENT, 0, start.0, start.1)?;
            self.fake(BUTTON_PRESS_EVENT, 1, start.0, start.1)?;
            self.sync()?;
            for step in 1..=DRAG_STEPS {
                let along = |a: i16, b: i16| {
                    (i32::from(a) + (i32::from(b) - i32::from(a)) * step / DRAG_STEPS) as i16
                };
                self.fake(
                    MOTION_NOTIFY_EVENT,
                    0,
                    along(start.0, end.0),
                    along(start.1, end.1),
                )?;
                self.sync()?;
                thread::sleep(Duration::from_millis(10));
            }
            self.fake(BUTTON_RELEASE_EVENT, 1, end.0, end.1)?;
            self.sync()
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
            .get_image(
                ImageFormat::Z_PIXMAP,
                window,
                (left - x) as i16,
                (top - y) as i16,
                width,
                height,
                !0,
            )
            .map_err(failed)?
            .reply()
            .map_err(failed)?;
        let setup = self.conn.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == image.depth)
            .ok_or("the window's pixel format is unknown")?;
        let masks = setup
            .roots
            .iter()
            .flat_map(|screen| &screen.allowed_depths)
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == image.visual)
            .map_or([0xff_0000, 0xff00, 0xff], |visual| {
                [visual.red_mask, visual.green_mask, visual.blue_mask]
            });
        let layout = Layout {
            bits: usize::from(format.bits_per_pixel),
            pad: usize::from(format.scanline_pad),
            masks,
            msb: setup.image_byte_order == ImageOrder::MSB_FIRST,
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
