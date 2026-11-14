use super::wire::{BIG_ENDIAN, LITTLE_ENDIAN, Malformed, Reader, Value, Writer, signature};
use std::{
    collections::HashMap,
    fmt, io,
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixStream as StdStream},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{mpsc, oneshot},
};

pub const BUS: &str = "org.freedesktop.DBus";
pub const BUS_PATH: &str = "/org/freedesktop/DBus";
pub const PEER: &str = "org.freedesktop.DBus.Peer";
pub const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";
const PROTOCOL_VERSION: u8 = 1;
const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;
const NO_REPLY_EXPECTED: u8 = 0x1;
const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_ERROR_NAME: u8 = 4;
const FIELD_REPLY_SERIAL: u8 = 5;
const FIELD_DESTINATION: u8 = 6;
const FIELD_SENDER: u8 = 7;
const FIELD_SIGNATURE: u8 = 8;
const FIXED_HEADER_BYTES: usize = 16;
const BODY_LENGTH_AT: usize = 4;
const SERIAL_AT: usize = 8;
const FIELDS_LENGTH_AT: usize = 12;
const MESSAGE_BYTES_MAX: usize = 1 << 27;
const NAME_BYTES_MAX: usize = 255;
const GUID_HEX_BYTES: usize = 32;
const SASL_OK: &str = "OK ";
const SASL_DATA: &str = "DATA";
const SASL_LINE_END: &str = "\r\n";
const SASL_LINE_BYTES_MAX: usize = SASL_OK.len() + GUID_HEX_BYTES + SASL_LINE_END.len();

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Method(String),
    Closed,
    Full,
    Invalid(&'static str),
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Method(name) => write!(formatter, "{name}"),
            Self::Closed => write!(formatter, "the D-Bus connection closed"),
            Self::Full => write!(formatter, "the D-Bus connection's queue is full"),
            Self::Invalid(what) => write!(formatter, "{what}"),
            Self::Malformed(what) => write!(formatter, "a malformed D-Bus reply: {what}"),
        }
    }
}

impl From<Malformed> for Error {
    fn from(malformed: Malformed) -> Self {
        Self::Malformed(malformed.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Call,
    Return,
    Error,
    Signal,
}

pub struct Message {
    pub kind: Kind,
    flags: u8,
    serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    error: Option<String>,
    reply_to: Option<u32>,
    pub sender: Option<String>,
    signature: String,
    big: bool,
    body_at: usize,
    bytes: Vec<u8>,
}

impl Message {
    pub fn body(&self) -> Result<Vec<Value>, Error> {
        let mut reader = Reader::new(&self.bytes[self.body_at..], 0, self.big);
        let values = reader.values(&self.signature)?;
        if reader.at() != self.bytes.len() - self.body_at {
            return Err(Error::Malformed("a body is longer than its signature"));
        }
        Ok(values)
    }
}

pub type Keep = fn(&Message) -> bool;

struct Watch {
    rule: String,
    sender: String,
    keep: Keep,
    changes: mpsc::UnboundedSender<()>,
}

enum Command {
    Call {
        message: Vec<u8>,
        reply: oneshot::Sender<Message>,
    },
    Watch {
        watch: Watch,
        message: Vec<u8>,
        reply: oneshot::Sender<Message>,
    },
    Forget {
        changes: mpsc::UnboundedSender<()>,
    },
}

#[derive(Clone)]
pub struct Bus {
    commands: mpsc::Sender<Command>,
}

pub fn valid_name(name: &str) -> bool {
    let (unique, elements) = match name.strip_prefix(':') {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    let element = |element: &str| {
        !element.is_empty()
            && element
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            && (unique || !element.as_bytes()[0].is_ascii_digit())
    };
    name.len() <= NAME_BYTES_MAX
        && elements.split('.').count() >= 2
        && elements.split('.').all(element)
}

pub fn valid_path(path: &str) -> bool {
    path == "/"
        || path.strip_prefix('/').is_some_and(|rest| {
            rest.split('/').all(|element| {
                !element.is_empty()
                    && element
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        })
}

pub fn method(
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
    args: &[Value],
) -> Vec<u8> {
    let mut body = Writer::default();
    args.iter().for_each(|arg| body.value(arg));
    let mut fields: Vec<(u8, Value)> = vec![
        (FIELD_PATH, Value::Path(path.to_owned())),
        (FIELD_INTERFACE, Value::Str(interface.to_owned())),
        (FIELD_MEMBER, Value::Str(member.to_owned())),
        (FIELD_DESTINATION, Value::Str(destination.to_owned())),
    ];
    if !args.is_empty() {
        fields.push((FIELD_SIGNATURE, Value::Signature(signature(args))));
    }
    encode(METHOD_CALL, 0, &fields, &body.bytes)
}

fn encode(kind: u8, flags: u8, fields: &[(u8, Value)], body: &[u8]) -> Vec<u8> {
    let mut out = Writer::default();
    out.byte(LITTLE_ENDIAN);
    out.byte(kind);
    out.byte(flags);
    out.byte(PROTOCOL_VERSION);
    out.u32(u32::try_from(body.len()).expect("a D-Bus body we send fits in 32 bits"));
    out.u32(0);
    let mut header = Writer::default();
    for (code, value) in fields {
        header.pad(8);
        header.byte(*code);
        header.value(&Value::Variant(Box::new(value.clone())));
    }
    out.u32(u32::try_from(header.bytes.len()).expect("a D-Bus header fits in 32 bits"));
    out.bytes.extend_from_slice(&header.bytes);
    out.pad(8);
    out.bytes.extend_from_slice(body);
    assert!(out.bytes.len() <= MESSAGE_BYTES_MAX);
    out.bytes
}

fn parse(bytes: Vec<u8>) -> Result<Message, Malformed> {
    let big = match bytes[0] {
        LITTLE_ENDIAN => false,
        BIG_ENDIAN => true,
        _ => return Err(Malformed("a message names no byte order")),
    };
    if bytes[3] != PROTOCOL_VERSION {
        return Err(Malformed("a message has another protocol version"));
    }
    let kind = match bytes[1] {
        METHOD_CALL => Kind::Call,
        METHOD_RETURN => Kind::Return,
        ERROR => Kind::Error,
        SIGNAL => Kind::Signal,
        _ => return Err(Malformed("a message has an unknown type")),
    };
    let mut reader = Reader::new(&bytes, SERIAL_AT, big);
    let serial = reader.u32()?;
    let Value::Array(fields) = reader.values("a(yv)")?.remove(0) else {
        unreachable!("a(yv) reads as an array");
    };
    reader.pad(8)?;
    let body_at = reader.at();
    let mut message = Message {
        kind,
        flags: bytes[2],
        serial,
        path: None,
        interface: None,
        member: None,
        error: None,
        reply_to: None,
        sender: None,
        signature: String::new(),
        big,
        body_at,
        bytes: Vec::new(),
    };
    for field in &fields {
        let (Some(Value::Byte(code)), Some(value)) = (
            field.fields().and_then(|parts| parts.first()),
            field.fields().and_then(|parts| parts.get(1)?.inner()),
        ) else {
            return Err(Malformed("a header field is not a code and a value"));
        };
        let text = || value.str().map(str::to_owned);
        match *code {
            FIELD_PATH => message.path = text(),
            FIELD_INTERFACE => message.interface = text(),
            FIELD_MEMBER => message.member = text(),
            FIELD_ERROR_NAME => message.error = text(),
            FIELD_REPLY_SERIAL => message.reply_to = value.u32(),
            FIELD_SENDER => message.sender = text(),
            FIELD_SIGNATURE => message.signature = text().unwrap_or_default(),
            _ => {}
        }
    }
    message.bytes = bytes;
    Ok(message)
}

fn total_bytes(header: &[u8]) -> Result<usize, Malformed> {
    let word = |at: usize| {
        let bytes: [u8; 4] = header[at..at + 4].try_into().expect("four bytes");
        if header[0] == BIG_ENDIAN {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    };
    let fields = word(FIELDS_LENGTH_AT) as usize;
    let body = word(BODY_LENGTH_AT) as usize;
    let total = (FIXED_HEADER_BYTES + fields).next_multiple_of(8) + body;
    if total > MESSAGE_BYTES_MAX {
        return Err(Malformed("a message is longer than D-Bus allows"));
    }
    Ok(total)
}

#[derive(Default)]
struct Incoming {
    bytes: Vec<u8>,
    filled: usize,
}

impl Incoming {
    async fn next(&mut self, read: &mut OwnedReadHalf) -> io::Result<Vec<u8>> {
        loop {
            let needed = if self.filled < FIXED_HEADER_BYTES {
                FIXED_HEADER_BYTES
            } else {
                total_bytes(&self.bytes[..FIXED_HEADER_BYTES])
                    .map_err(|malformed| io::Error::new(io::ErrorKind::InvalidData, malformed.0))?
            };
            if self.filled == needed {
                self.filled = 0;
                return Ok(std::mem::take(&mut self.bytes));
            }
            self.bytes.resize(needed, 0);
            let read = read.read(&mut self.bytes[self.filled..needed]).await?;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.filled += read;
        }
    }
}

fn stream(address: &str) -> io::Result<StdStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no unix address to connect to");
    for entry in address.split(';') {
        let Some(("unix", pairs)) = entry.split_once(':') else {
            continue;
        };
        let pairs: HashMap<&str, String> = pairs
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (key, unescape(value)))
            .collect();
        let connected = if let Some(path) = pairs.get("path") {
            StdStream::connect(path)
        } else if let Some(name) = pairs.get("abstract") {
            SocketAddr::from_abstract_name(name.as_bytes())
                .and_then(|address| StdStream::connect_addr(&address))
        } else {
            continue;
        };
        match connected {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn line(stream: &mut UnixStream) -> io::Result<String> {
    let mut line = Vec::new();
    while !line.ends_with(SASL_LINE_END.as_bytes()) {
        if line.len() == SASL_LINE_BYTES_MAX {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the bus refused EXTERNAL authentication",
            ));
        }
        line.push(stream.read_u8().await?);
    }
    line.truncate(line.len() - SASL_LINE_END.len());
    String::from_utf8(line).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the bus answered with other than text",
        )
    })
}

async fn authenticate(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"\0AUTH EXTERNAL\r\n").await?;
    let refused = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the bus refused EXTERNAL authentication",
        )
    };
    if line(stream).await? != SASL_DATA {
        return Err(refused());
    }
    stream.write_all(b"DATA\r\n").await?;
    let answer = line(stream).await?;
    let guid = answer.strip_prefix(SASL_OK).ok_or_else(refused)?;
    if guid.len() != GUID_HEX_BYTES || !guid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(refused());
    }
    stream.write_all(b"BEGIN\r\n").await
}

impl Bus {
    pub async fn connect(address: &str, queue: usize) -> io::Result<Self> {
        let stream = stream(address)?;
        stream.set_nonblocking(true)?;
        let mut stream = UnixStream::from_std(stream)?;
        authenticate(&mut stream).await?;
        let (commands, receiver) = mpsc::channel(queue);
        nanocode_platform::spawn_task(own(stream, receiver));
        let bus = Self { commands };
        bus.call(BUS, BUS_PATH, BUS, "Hello", &[])
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(bus)
    }

    pub fn call(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        args: &[Value],
    ) -> impl Future<Output = Result<Message, Error>> + Send + use<> {
        let message = if !valid_name(destination) {
            Err(Error::Invalid("the destination is not a D-Bus name"))
        } else if !valid_path(path) {
            Err(Error::Invalid("the path is not a D-Bus object path"))
        } else {
            Ok(method(destination, path, interface, member, args))
        };
        let commands = self.commands.clone();
        async move {
            let (reply, answer) = oneshot::channel();
            commands
                .send(Command::Call {
                    message: message?,
                    reply,
                })
                .await
                .map_err(|_| Error::Closed)?;
            answered(answer.await.map_err(|_| Error::Closed)?)
        }
    }

    pub async fn watch(
        &self,
        sender: &str,
        keep: Keep,
        changes: mpsc::UnboundedSender<()>,
    ) -> Result<(), Error> {
        if !valid_name(sender) {
            return Err(Error::Invalid("the sender is not a D-Bus name"));
        }
        let rule = format!("type='signal',sender='{sender}'");
        let message = method(BUS, BUS_PATH, BUS, "AddMatch", &[Value::Str(rule.clone())]);
        let (reply, answer) = oneshot::channel();
        let watch = Watch {
            rule,
            sender: sender.to_owned(),
            keep,
            changes,
        };
        self.commands
            .send(Command::Watch {
                watch,
                message,
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        answered(answer.await.map_err(|_| Error::Closed)?).map(|_| ())
    }

    pub fn forget(&self, changes: mpsc::UnboundedSender<()>) -> Result<(), Error> {
        self.commands
            .try_send(Command::Forget { changes })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::Full,
                mpsc::error::TrySendError::Closed(_) => Error::Closed,
            })
    }
}

fn answered(message: Message) -> Result<Message, Error> {
    match message.kind {
        Kind::Error => Err(Error::Method(message.error.unwrap_or_default())),
        _ => Ok(message),
    }
}

struct Owner {
    write: OwnedWriteHalf,
    serial: u32,
    pending: HashMap<u32, oneshot::Sender<Message>>,
    watches: Vec<Watch>,
}

impl Owner {
    async fn send(
        &mut self,
        mut message: Vec<u8>,
        reply: Option<oneshot::Sender<Message>>,
    ) -> io::Result<()> {
        self.serial = self.serial.checked_add(1).unwrap_or(1);
        message[SERIAL_AT..SERIAL_AT + 4].copy_from_slice(&self.serial.to_le_bytes());
        if let Some(reply) = reply {
            self.pending.retain(|_, waiting| !waiting.is_closed());
            self.pending.insert(self.serial, reply);
        }
        self.write.write_all(&message).await
    }

    async fn command(&mut self, command: Command) -> io::Result<()> {
        match command {
            Command::Call { message, reply } => self.send(message, Some(reply)).await,
            Command::Watch {
                watch,
                message,
                reply,
            } => {
                self.watches.push(watch);
                self.send(message, Some(reply)).await
            }
            Command::Forget { changes } => {
                let (gone, kept) = std::mem::take(&mut self.watches)
                    .into_iter()
                    .partition(|watch| watch.changes.same_channel(&changes));
                self.watches = kept;
                self.remove(gone.into_iter().map(|watch| watch.rule)).await
            }
        }
    }

    async fn remove(&mut self, rules: impl Iterator<Item = String>) -> io::Result<()> {
        for rule in rules {
            let message = method(BUS, BUS_PATH, BUS, "RemoveMatch", &[Value::Str(rule)]);
            self.send(message, None).await?;
        }
        Ok(())
    }

    async fn received(&mut self, message: Message) -> io::Result<()> {
        match message.kind {
            Kind::Return | Kind::Error => {
                if let Some(reply) = message
                    .reply_to
                    .and_then(|serial| self.pending.remove(&serial))
                {
                    let _ = reply.send(message);
                }
                Ok(())
            }
            Kind::Signal => self.signal(&message).await,
            Kind::Call => self.refuse(&message).await,
        }
    }

    async fn signal(&mut self, message: &Message) -> io::Result<()> {
        let mut closed = Vec::new();
        self.watches.retain(|watch| {
            let open = !watch.changes.is_closed();
            if !open {
                closed.push(watch.rule.clone());
            }
            open
        });
        for watch in &self.watches {
            if message.sender.as_deref() == Some(watch.sender.as_str()) && (watch.keep)(message) {
                let _ = watch.changes.send(());
            }
        }
        self.remove(closed.into_iter()).await
    }

    async fn refuse(&mut self, call: &Message) -> io::Result<()> {
        if call.flags & NO_REPLY_EXPECTED != 0 {
            return Ok(());
        }
        let Some(sender) = &call.sender else {
            return Ok(());
        };
        let mut fields = vec![
            (FIELD_REPLY_SERIAL, Value::U32(call.serial)),
            (FIELD_DESTINATION, Value::Str(sender.clone())),
        ];
        let (kind, body) =
            if call.interface.as_deref() == Some(PEER) && call.member.as_deref() == Some("Ping") {
                (METHOD_RETURN, Vec::new())
            } else {
                fields.push((FIELD_ERROR_NAME, Value::Str(UNKNOWN_OBJECT.to_owned())));
                (ERROR, Vec::new())
            };
        let message = encode(kind, NO_REPLY_EXPECTED, &fields, &body);
        self.send(message, None).await
    }
}

async fn own(stream: UnixStream, mut commands: mpsc::Receiver<Command>) {
    let (mut read, write) = stream.into_split();
    let mut owner = Owner {
        write,
        serial: 0,
        pending: HashMap::new(),
        watches: Vec::new(),
    };
    let mut incoming = Incoming::default();
    loop {
        let done = tokio::select! {
            command = commands.recv() => match command {
                Some(command) => owner.command(command).await,
                None => break,
            },
            bytes = incoming.next(&mut read) => match bytes.and_then(|bytes| {
                parse(bytes).map_err(|malformed| io::Error::new(io::ErrorKind::InvalidData, malformed.0))
            }) {
                Ok(message) => owner.received(message).await,
                Err(error) => Err(error),
            },
        };
        if done.is_err() {
            break;
        }
    }
}
