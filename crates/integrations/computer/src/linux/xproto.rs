use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    env, fmt, fs,
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, TcpStream},
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixStream},
    },
    path::PathBuf,
};

pub type Window = u32;
pub type Atom = u32;

pub const ANY_PROPERTY_TYPE: Atom = 0;
pub const ATOM_CARDINAL: Atom = 6;
pub const ATOM_STRING: Atom = 31;
pub const ATOM_WINDOW: Atom = 33;
pub const ATOM_WM_CLASS: Atom = 67;
pub const PROPERTY_CHANGE_MASK: u32 = 0x0040_0000;
pub const SUBSTRUCTURE_NOTIFY_MASK: u32 = 0x0008_0000;
pub const SUBSTRUCTURE_REDIRECT_MASK: u32 = 0x0010_0000;
pub const PROP_MODE_APPEND: u8 = 2;
pub const MAP_STATE_VIEWABLE: u8 = 2;
pub const KEY_PRESS: u8 = 2;
pub const KEY_RELEASE: u8 = 3;
pub const BUTTON_PRESS: u8 = 4;
pub const BUTTON_RELEASE: u8 = 5;
pub const MOTION_NOTIFY: u8 = 6;
pub const PROPERTY_NOTIFY: u8 = 28;
const CLIENT_MESSAGE: u8 = 33;
const GENERIC_EVENT: u8 = 35;
const SEND_EVENT_BIT: u8 = 0x80;
const ERROR: u8 = 0;
const REPLY: u8 = 1;
const PACKET_BYTES: usize = 32;
const WORD_BYTES: usize = 4;
const LITTLE_ENDIAN: u8 = b'l';
const PROTOCOL_MAJOR: u16 = 11;
const PROTOCOL_MINOR: u16 = 0;
const SETUP_SUCCESS: u8 = 1;
const SETUP_FIXED_BYTES: usize = 8;
const SETUP_VENDOR_AT: usize = 40;
const FORMAT_BYTES: usize = 8;
const SCREEN_BYTES: usize = 40;
const DEPTH_BYTES: usize = 8;
const VISUAL_BYTES: usize = 24;
const TCP_PORT_BASE: u16 = 6000;
const UNIX_SOCKET_PREFIX: &str = "/tmp/.X11-unix/X";
const AUTHORITY_FILE: &str = ".Xauthority";
const MIT_MAGIC_COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";
const FAMILY_INTERNET: u16 = 0;
const FAMILY_INTERNET6: u16 = 6;
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;
const WINDOW_CLASS_INPUT_ONLY: u16 = 2;
const COPY_FROM_PARENT: u32 = 0;
const CW_OVERRIDE_REDIRECT: u32 = 0x0200;
const CW_EVENT_MASK: u32 = 0x0800;
const IMAGE_FORMAT_Z_PIXMAP: u8 = 2;
const IMAGE_ORDER_MSB_FIRST: u8 = 1;
const ALL_PLANES: u32 = !0;
const SEQUENCE_SPAN: u64 = 1 << 16;
const CREATE_WINDOW: u8 = 1;
const CHANGE_WINDOW_ATTRIBUTES: u8 = 2;
const GET_WINDOW_ATTRIBUTES: u8 = 3;
const DESTROY_WINDOW: u8 = 4;
const GET_GEOMETRY: u8 = 14;
const QUERY_TREE: u8 = 15;
const INTERN_ATOM: u8 = 16;
const CHANGE_PROPERTY: u8 = 18;
const GET_PROPERTY: u8 = 20;
const SEND_EVENT: u8 = 25;
const QUERY_POINTER: u8 = 38;
const TRANSLATE_COORDINATES: u8 = 40;
const GET_INPUT_FOCUS: u8 = 43;
const GET_IMAGE: u8 = 73;
const QUERY_EXTENSION: u8 = 98;
const CHANGE_KEYBOARD_MAPPING: u8 = 100;
const GET_KEYBOARD_MAPPING: u8 = 101;
const XTEST: &[u8] = b"XTEST";
const XTEST_FAKE_INPUT: u8 = 2;

#[derive(Debug)]
pub enum Failure {
    Io(io::Error),
    X {
        code: u8,
        major: u8,
        minor: u16,
        value: u32,
    },
    Malformed(&'static str),
    Missing(&'static str),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::X {
                code,
                major,
                minor,
                value,
            } => write!(
                formatter,
                "X11 error {code} for request {major}.{minor} on {value:#x}"
            ),
            Self::Malformed(what) | Self::Missing(what) => write!(formatter, "{what}"),
        }
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

enum Stream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Stream {
    fn read_exact(&self, bytes: &mut [u8]) -> io::Result<()> {
        match self {
            Self::Unix(stream) => (&*stream).read_exact(bytes),
            Self::Tcp(stream) => (&*stream).read_exact(bytes),
        }
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Unix(stream) => (&*stream).write_all(bytes),
            Self::Tcp(stream) => (&*stream).write_all(bytes),
        }
    }

    fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Self::Unix(stream) => Self::Unix(stream.try_clone()?),
            Self::Tcp(stream) => Self::Tcp(stream.try_clone()?),
        })
    }
}

pub struct Closer(Stream);

impl Closer {
    pub fn close(&self) {
        let _ = match &self.0 {
            Stream::Unix(stream) => stream.shutdown(Shutdown::Both),
            Stream::Tcp(stream) => stream.shutdown(Shutdown::Both),
        };
    }
}

pub struct Format {
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
}

pub struct Visual {
    pub id: u32,
    pub masks: [u32; 3],
}

pub struct Setup {
    pub root: Window,
    pub width: u16,
    pub height: u16,
    pub min_keycode: u8,
    pub max_keycode: u8,
    pub image_msb_first: bool,
    pub formats: Vec<Format>,
    pub visuals: Vec<Visual>,
    request_words_max: usize,
    id_base: u32,
    id_mask: u32,
}

pub struct Property {
    pub format: u8,
    pub value: Vec<u8>,
}

impl Property {
    pub fn value32(&self) -> Option<impl Iterator<Item = u32> + '_> {
        (self.format == 32).then(|| {
            self.value
                .as_chunks::<WORD_BYTES>()
                .0
                .iter()
                .map(|word| u32::from_le_bytes(*word))
        })
    }
}

pub struct Image {
    pub depth: u8,
    pub visual: u32,
    pub data: Vec<u8>,
}

pub struct Keyboard {
    pub per: u8,
    pub syms: Vec<u32>,
}

pub struct Event {
    pub kind: u8,
    pub window: Window,
    pub atom: Atom,
    pub time: u32,
}

#[derive(Clone, Copy)]
pub struct Cookie(u64);

fn byte(bytes: &[u8], at: usize) -> Result<u8> {
    bytes
        .get(at)
        .copied()
        .ok_or(Failure::Malformed("the X server sent a short answer"))
}

fn half(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes([byte(bytes, at)?, byte(bytes, at + 1)?]))
}

fn word(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes([
        byte(bytes, at)?,
        byte(bytes, at + 1)?,
        byte(bytes, at + 2)?,
        byte(bytes, at + 3)?,
    ]))
}

const fn padded(length: usize) -> usize {
    length.next_multiple_of(WORD_BYTES)
}

struct Display {
    host: String,
    number: u16,
    screen: usize,
}

fn parse_display(text: &str) -> Option<Display> {
    let (protocol, rest) = match text.rsplit_once('/') {
        Some((protocol, rest)) => (Some(protocol), rest),
        None => (None, text),
    };
    let (host, rest) = rest.rsplit_once(':')?;
    let (number, screen) = rest.split_once('.').unwrap_or((rest, "0"));
    Some(Display {
        host: if protocol == Some("unix") {
            "unix"
        } else {
            host
        }
        .to_owned(),
        number: number.parse().ok()?,
        screen: screen.parse().ok()?,
    })
}

fn local_address() -> Vec<u8> {
    nanocode_sys::unix::host_name()
        .ok()
        .and_then(|name| name.into_string().ok())
        .map(String::into_bytes)
        .unwrap_or_default()
}

fn connect_stream(display: &Display) -> io::Result<(Stream, u16, Vec<u8>)> {
    if display.host.is_empty() || display.host == "unix" {
        let path = format!("{UNIX_SOCKET_PREFIX}{}", display.number);
        let stream = SocketAddr::from_abstract_name(path.as_bytes())
            .and_then(|address| UnixStream::connect_addr(&address))
            .or_else(|_| UnixStream::connect(&path))?;
        return Ok((Stream::Unix(stream), FAMILY_LOCAL, local_address()));
    }
    let port = TCP_PORT_BASE.checked_add(display.number).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the display number is too large",
        )
    })?;
    let stream = TcpStream::connect((display.host.as_str(), port))?;
    stream.set_nodelay(true)?;
    let ip = match stream.peer_addr()?.ip() {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip => ip,
    };
    let (family, address) = match ip {
        ip if ip.is_loopback() => (FAMILY_LOCAL, local_address()),
        IpAddr::V4(ip) => (FAMILY_INTERNET, ip.octets().to_vec()),
        IpAddr::V6(ip) => (FAMILY_INTERNET6, ip.octets().to_vec()),
    };
    Ok((Stream::Tcp(stream), family, address))
}

struct Entries<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Entries<'a> {
    fn half(&mut self) -> Option<u16> {
        let value = u16::from_be_bytes(self.bytes.get(self.at..self.at + 2)?.try_into().ok()?);
        self.at += 2;
        Some(value)
    }

    fn field(&mut self) -> Option<&'a [u8]> {
        let length = usize::from(self.half()?);
        let value = self.bytes.get(self.at..self.at + length)?;
        self.at += length;
        Some(value)
    }
}

fn authority(family: u16, address: &[u8], number: u16) -> Option<Vec<u8>> {
    let path = env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(AUTHORITY_FILE)))?;
    let bytes = fs::read(path).ok()?;
    let number = number.to_string();
    let mut entries = Entries {
        bytes: &bytes,
        at: 0,
    };
    loop {
        let entry_family = entries.half()?;
        let (entry_address, entry_number) = (entries.field()?, entries.field()?);
        let (name, data) = (entries.field()?, entries.field()?);
        let address_matches =
            entry_family == FAMILY_WILD || (entry_family == family && entry_address == address);
        let number_matches = entry_number.is_empty() || entry_number == number.as_bytes();
        if address_matches && number_matches && name == MIT_MAGIC_COOKIE {
            return Some(data.to_vec());
        }
    }
}

fn parse_setup(body: &[u8], screen: usize) -> Result<Setup> {
    let vendor = usize::from(half(body, 16)?);
    let (screens, formats) = (usize::from(byte(body, 20)?), usize::from(byte(body, 21)?));
    let mut at = SETUP_VENDOR_AT - SETUP_FIXED_BYTES + padded(vendor);
    let mut parsed = Vec::with_capacity(formats);
    for _ in 0..formats {
        parsed.push(Format {
            depth: byte(body, at)?,
            bits_per_pixel: byte(body, at + 1)?,
            scanline_pad: byte(body, at + 2)?,
        });
        at += FORMAT_BYTES;
    }
    let mut chosen = None;
    let mut visuals = Vec::new();
    for index in 0..screens {
        if index == screen {
            chosen = Some((word(body, at)?, half(body, at + 20)?, half(body, at + 22)?));
        }
        let depths = byte(body, at + 39)?;
        at += SCREEN_BYTES;
        for _ in 0..depths {
            let count = half(body, at + 2)?;
            at += DEPTH_BYTES;
            for _ in 0..count {
                visuals.push(Visual {
                    id: word(body, at)?,
                    masks: [
                        word(body, at + 8)?,
                        word(body, at + 12)?,
                        word(body, at + 16)?,
                    ],
                });
                at += VISUAL_BYTES;
            }
        }
    }
    let (root, width, height) =
        chosen.ok_or(Failure::Missing("the X display has no such screen"))?;
    Ok(Setup {
        root,
        width,
        height,
        min_keycode: byte(body, 26)?,
        max_keycode: byte(body, 27)?,
        image_msb_first: byte(body, 22)? == IMAGE_ORDER_MSB_FIRST,
        formats: parsed,
        visuals,
        request_words_max: usize::from(half(body, 18)?),
        id_base: word(body, 4)?,
        id_mask: word(body, 8)?,
    })
}

pub struct Connection {
    stream: Stream,
    pub setup: Setup,
    pub screen: usize,
    sent: Cell<u64>,
    answers: RefCell<BTreeMap<u64, Result<Vec<u8>>>>,
    events: RefCell<VecDeque<Event>>,
    ids: Cell<u32>,
    xtest: Cell<Option<u8>>,
}

impl Connection {
    pub fn open() -> Result<Self> {
        let name = env::var("DISPLAY").map_err(|_| Failure::Missing("DISPLAY is not set"))?;
        let display = parse_display(&name)
            .ok_or(Failure::Malformed("DISPLAY is not [host]:display[.screen]"))?;
        let (stream, family, address) = connect_stream(&display)?;
        let cookie = authority(family, &address, display.number);
        let (name, data): (&[u8], &[u8]) = match &cookie {
            Some(data) => (MIT_MAGIC_COOKIE, data),
            None => (&[], &[]),
        };
        let mut request = vec![LITTLE_ENDIAN, 0];
        request.extend_from_slice(&PROTOCOL_MAJOR.to_le_bytes());
        request.extend_from_slice(&PROTOCOL_MINOR.to_le_bytes());
        request.extend_from_slice(&(name.len() as u16).to_le_bytes());
        request.extend_from_slice(&(data.len() as u16).to_le_bytes());
        request.extend_from_slice(&[0, 0]);
        for part in [name, data] {
            request.extend_from_slice(part);
            request.resize(padded(request.len()), 0);
        }
        stream.write_all(&request)?;
        let mut head = [0; SETUP_FIXED_BYTES];
        stream.read_exact(&mut head)?;
        let mut body = vec![0; usize::from(half(&head, 6)?) * WORD_BYTES];
        stream.read_exact(&mut body)?;
        if head[0] != SETUP_SUCCESS {
            let reason = body.get(..usize::from(head[1])).unwrap_or_default();
            return Err(Failure::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "the X server refused the connection: {}",
                    String::from_utf8_lossy(reason).trim()
                ),
            )));
        }
        let setup = parse_setup(&body, display.screen)?;
        Ok(Self {
            stream,
            setup,
            screen: display.screen,
            sent: Cell::new(0),
            answers: RefCell::new(BTreeMap::new()),
            events: RefCell::new(VecDeque::new()),
            ids: Cell::new(0),
            xtest: Cell::new(None),
        })
    }

    pub fn closer(&self) -> Result<Closer> {
        Ok(Closer(self.stream.try_clone()?))
    }

    pub fn generate_id(&self) -> Result<u32> {
        let mask = self.setup.id_mask;
        let step = mask.isolate_lowest_one();
        let next = self.ids.get().wrapping_add(step);
        if step == 0 || next & !mask != 0 || next == 0 {
            return Err(Failure::Missing(
                "the X server gave out every resource id it allows",
            ));
        }
        self.ids.set(next);
        Ok(self.setup.id_base | (next - step))
    }

    fn request(&self, opcode: u8, data: u8, body: &[u8]) -> Result<Cookie> {
        let length = WORD_BYTES + padded(body.len());
        if length / WORD_BYTES > self.setup.request_words_max {
            return Err(Failure::Missing(
                "the request is longer than the X server takes",
            ));
        }
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(&[opcode, data]);
        bytes.extend_from_slice(&((length / WORD_BYTES) as u16).to_le_bytes());
        bytes.extend_from_slice(body);
        bytes.resize(length, 0);
        self.stream.write_all(&bytes)?;
        let sequence = self.sent.get() + 1;
        self.sent.set(sequence);
        Ok(Cookie(sequence))
    }

    fn sequence(&self, low: u16) -> Result<u64> {
        let sent = self.sent.get();
        let candidate = (sent & !(SEQUENCE_SPAN - 1)) | u64::from(low);
        if candidate <= sent {
            return Ok(candidate);
        }
        candidate
            .checked_sub(SEQUENCE_SPAN)
            .ok_or(Failure::Malformed(
                "the X server answered a request not sent",
            ))
    }

    fn read_packet(&self) -> Result<()> {
        let mut packet = [0; PACKET_BYTES];
        self.stream.read_exact(&mut packet)?;
        match packet[0] {
            ERROR => {
                let sequence = self.sequence(half(&packet, 2)?)?;
                let failure = Failure::X {
                    code: packet[1],
                    value: word(&packet, 4)?,
                    minor: half(&packet, 8)?,
                    major: packet[10],
                };
                self.answers.borrow_mut().insert(sequence, Err(failure));
            }
            REPLY => {
                let sequence = self.sequence(half(&packet, 2)?)?;
                let mut reply = packet.to_vec();
                reply.resize(PACKET_BYTES + word(&packet, 4)? as usize * WORD_BYTES, 0);
                self.stream.read_exact(&mut reply[PACKET_BYTES..])?;
                self.answers.borrow_mut().insert(sequence, Ok(reply));
            }
            kind if kind & !SEND_EVENT_BIT == GENERIC_EVENT => {
                let mut rest = vec![0; word(&packet, 4)? as usize * WORD_BYTES];
                self.stream.read_exact(&mut rest)?;
            }
            kind if kind & !SEND_EVENT_BIT == PROPERTY_NOTIFY => {
                self.events.borrow_mut().push_back(Event {
                    kind: PROPERTY_NOTIFY,
                    window: word(&packet, 4)?,
                    atom: word(&packet, 8)?,
                    time: word(&packet, 12)?,
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn arrive(&self, cookie: Cookie) -> Result<()> {
        while !self.answers.borrow().contains_key(&cookie.0) {
            self.read_packet()?;
        }
        Ok(())
    }

    pub fn reply(&self, cookie: Cookie) -> Result<Vec<u8>> {
        self.arrive(cookie)?;
        let mut answers = self.answers.borrow_mut();
        let later = answers.split_off(&(cookie.0 + 1));
        let answer = answers.remove(&cookie.0).expect("the answer just arrived");
        *answers = later;
        answer
    }

    pub fn sync(&self) -> Result<()> {
        self.input_focus().map(|_| ())
    }

    pub fn check(&self, cookie: Cookie) -> Result<()> {
        let sync = self.request(GET_INPUT_FOCUS, 0, &[])?;
        self.arrive(sync)?;
        let failed = self.answers.borrow_mut().remove(&cookie.0);
        self.reply(sync)?;
        match failed {
            Some(Err(failure)) => Err(failure),
            _ => Ok(()),
        }
    }

    pub fn event(&self) -> Result<Event> {
        loop {
            if let Some(event) = self.events.borrow_mut().pop_front() {
                return Ok(event);
            }
            self.read_packet()?;
        }
    }

    pub fn intern_atom(&self, only_if_exists: bool, name: &[u8]) -> Result<Atom> {
        let mut body = (name.len() as u16).to_le_bytes().to_vec();
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(name);
        let cookie = self.request(INTERN_ATOM, u8::from(only_if_exists), &body)?;
        word(&self.reply(cookie)?, 8)
    }

    pub fn get_property(
        &self,
        window: Window,
        property: Atom,
        kind: Atom,
        length: u32,
    ) -> Result<Cookie> {
        let mut body = Vec::with_capacity(20);
        for value in [window, property, kind, 0, length] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        self.request(GET_PROPERTY, 0, &body)
    }

    pub fn property(&self, cookie: Cookie) -> Result<Property> {
        let reply = self.reply(cookie)?;
        let format = byte(&reply, 1)?;
        let length = word(&reply, 16)? as usize * usize::from(format / 8);
        let value = reply
            .get(PACKET_BYTES..PACKET_BYTES + length)
            .ok_or(Failure::Malformed("the X server sent a short property"))?
            .to_vec();
        Ok(Property { format, value })
    }

    pub fn change_property(
        &self,
        mode: u8,
        window: Window,
        property: Atom,
        kind: Atom,
        data: &[u8],
    ) -> Result<Cookie> {
        let mut body = Vec::with_capacity(20 + data.len());
        for value in [window, property, kind] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        body.extend_from_slice(&[8, 0, 0, 0]);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        self.request(CHANGE_PROPERTY, mode, &body)
    }

    pub fn select_events(&self, window: Window, mask: u32) -> Result<Cookie> {
        let mut body = window.to_le_bytes().to_vec();
        body.extend_from_slice(&CW_EVENT_MASK.to_le_bytes());
        body.extend_from_slice(&mask.to_le_bytes());
        self.request(CHANGE_WINDOW_ATTRIBUTES, 0, &body)
    }

    pub fn create_input_only(&self, window: Window, parent: Window, mask: u32) -> Result<Cookie> {
        let mut body = Vec::with_capacity(36);
        body.extend_from_slice(&window.to_le_bytes());
        body.extend_from_slice(&parent.to_le_bytes());
        for value in [-1_i16, -1] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1_u16, 1, 0, WINDOW_CLASS_INPUT_ONLY] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            COPY_FROM_PARENT,
            CW_OVERRIDE_REDIRECT | CW_EVENT_MASK,
            1,
            mask,
        ] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        self.request(CREATE_WINDOW, 0, &body)
    }

    pub fn destroy_window(&self, window: Window) -> Result<Cookie> {
        self.request(DESTROY_WINDOW, 0, &window.to_le_bytes())
    }

    pub fn send_client_message(
        &self,
        destination: Window,
        mask: u32,
        format: u8,
        window: Window,
        kind: Atom,
        data: [u8; 20],
    ) -> Result<Cookie> {
        let mut body = destination.to_le_bytes().to_vec();
        body.extend_from_slice(&mask.to_le_bytes());
        body.extend_from_slice(&[CLIENT_MESSAGE, format, 0, 0]);
        body.extend_from_slice(&window.to_le_bytes());
        body.extend_from_slice(&kind.to_le_bytes());
        body.extend_from_slice(&data);
        self.request(SEND_EVENT, 0, &body)
    }

    pub fn input_focus(&self) -> Result<Window> {
        let cookie = self.request(GET_INPUT_FOCUS, 0, &[])?;
        word(&self.reply(cookie)?, 8)
    }

    pub fn pointer_child(&self, window: Window) -> Result<Window> {
        let cookie = self.request(QUERY_POINTER, 0, &window.to_le_bytes())?;
        word(&self.reply(cookie)?, 12)
    }

    pub fn query_tree(&self, window: Window) -> Result<(Window, Vec<Window>)> {
        let cookie = self.request(QUERY_TREE, 0, &window.to_le_bytes())?;
        let reply = self.reply(cookie)?;
        let children = (0..usize::from(half(&reply, 16)?))
            .map(|index| word(&reply, PACKET_BYTES + index * WORD_BYTES))
            .collect::<Result<_>>()?;
        Ok((word(&reply, 12)?, children))
    }

    pub fn geometry(&self, window: Window) -> Result<(u16, u16)> {
        let cookie = self.request(GET_GEOMETRY, 0, &window.to_le_bytes())?;
        let reply = self.reply(cookie)?;
        Ok((half(&reply, 16)?, half(&reply, 18)?))
    }

    pub fn translate(&self, window: Window, to: Window) -> Result<(i16, i16)> {
        let mut body = window.to_le_bytes().to_vec();
        body.extend_from_slice(&to.to_le_bytes());
        body.extend_from_slice(&[0; 4]);
        let cookie = self.request(TRANSLATE_COORDINATES, 0, &body)?;
        let reply = self.reply(cookie)?;
        Ok((half(&reply, 12)? as i16, half(&reply, 14)? as i16))
    }

    pub fn map_state(&self, window: Window) -> Result<u8> {
        let cookie = self.request(GET_WINDOW_ATTRIBUTES, 0, &window.to_le_bytes())?;
        byte(&self.reply(cookie)?, 26)
    }

    pub fn keyboard(&self) -> Result<Keyboard> {
        let (min, max) = (self.setup.min_keycode, self.setup.max_keycode);
        let count = max - min + 1;
        let cookie = self.request(GET_KEYBOARD_MAPPING, 0, &[min, count, 0, 0])?;
        let reply = self.reply(cookie)?;
        let syms = (0..word(&reply, 4)? as usize)
            .map(|index| word(&reply, PACKET_BYTES + index * WORD_BYTES))
            .collect::<Result<_>>()?;
        Ok(Keyboard {
            per: byte(&reply, 1)?,
            syms,
        })
    }

    pub fn remap(&self, code: u8, syms: &[u32]) -> Result<Cookie> {
        let mut body = vec![code, syms.len() as u8, 0, 0];
        syms.iter()
            .for_each(|sym| body.extend_from_slice(&sym.to_le_bytes()));
        self.request(CHANGE_KEYBOARD_MAPPING, 1, &body)
    }

    fn xtest(&self) -> Result<u8> {
        if let Some(major) = self.xtest.get() {
            return Ok(major);
        }
        let mut body = (XTEST.len() as u16).to_le_bytes().to_vec();
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(XTEST);
        let cookie = self.request(QUERY_EXTENSION, 0, &body)?;
        let reply = self.reply(cookie)?;
        if byte(&reply, 8)? == 0 {
            return Err(Failure::Missing("the X server has no XTEST extension"));
        }
        let major = byte(&reply, 9)?;
        self.xtest.set(Some(major));
        Ok(major)
    }

    pub fn fake_input(&self, kind: u8, detail: u8, root: Window, x: i16, y: i16) -> Result<Cookie> {
        let major = self.xtest()?;
        let mut body = vec![kind, detail, 0, 0];
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&root.to_le_bytes());
        body.extend_from_slice(&[0; 8]);
        body.extend_from_slice(&x.to_le_bytes());
        body.extend_from_slice(&y.to_le_bytes());
        body.extend_from_slice(&[0; 8]);
        self.request(major, XTEST_FAKE_INPUT, &body)
    }

    pub fn image(&self, window: Window, x: i16, y: i16, width: u16, height: u16) -> Result<Image> {
        let mut body = window.to_le_bytes().to_vec();
        for value in [x, y] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        for value in [width, height] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        body.extend_from_slice(&ALL_PLANES.to_le_bytes());
        let cookie = self.request(GET_IMAGE, IMAGE_FORMAT_Z_PIXMAP, &body)?;
        let mut reply = self.reply(cookie)?;
        let (depth, visual) = (byte(&reply, 1)?, word(&reply, 8)?);
        Ok(Image {
            depth,
            visual,
            data: reply.split_off(PACKET_BYTES),
        })
    }
}
