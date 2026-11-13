use crate::postgres::{DatabaseError, PgError};
use nanocode_durability_core::SqlRow;
use std::ops::Range;

pub(super) const PROTOCOL_VERSION: i32 = 196_608;
pub(super) const SSL_REQUEST: i32 = 80_877_103;
pub(super) const TEXT_OID: u32 = 25;
const MESSAGE_BYTES_MAX: usize = 0x3fff_ffff;
const LENGTH_BYTES: usize = 4;
const HEADER_BYTES: usize = 1 + LENGTH_BYTES;
pub(super) const READ_BUFFER_BYTES: usize = 8192;
const TEXT_FORMAT: i16 = 0;
const NULL_LENGTH: i32 = -1;

pub(super) mod tag {
    pub const AUTHENTICATION: u8 = b'R';
    pub const BACKEND_KEY: u8 = b'K';
    pub const BIND: u8 = b'B';
    pub const BIND_COMPLETE: u8 = b'2';
    pub const CLOSE_COMPLETE: u8 = b'3';
    pub const COMMAND_COMPLETE: u8 = b'C';
    pub const DATA_ROW: u8 = b'D';
    pub const EMPTY_QUERY: u8 = b'I';
    pub const ERROR: u8 = b'E';
    pub const EXECUTE: u8 = b'E';
    pub const NO_DATA: u8 = b'n';
    pub const NOTICE: u8 = b'N';
    pub const NOTIFICATION: u8 = b'A';
    pub const PARAMETER_DESCRIPTION: u8 = b't';
    pub const PARAMETER_STATUS: u8 = b'S';
    pub const PARSE: u8 = b'P';
    pub const PARSE_COMPLETE: u8 = b'1';
    pub const PASSWORD: u8 = b'p';
    pub const QUERY: u8 = b'Q';
    pub const READY: u8 = b'Z';
    pub const ROW_DESCRIPTION: u8 = b'T';
    pub const SYNC: u8 = b'S';
    pub const TERMINATE: u8 = b'X';
}

fn frame(out: &mut Vec<u8>, tag: Option<u8>, body: impl FnOnce(&mut Vec<u8>)) {
    if let Some(tag) = tag {
        out.push(tag);
    }
    let at = out.len();
    out.extend_from_slice(&[0; LENGTH_BYTES]);
    body(out);
    let length =
        i32::try_from(out.len() - at).expect("a frontend message fits the protocol's length");
    out[at..at + LENGTH_BYTES].copy_from_slice(&length.to_be_bytes());
}

fn checked(text: &str) -> Result<&str, PgError> {
    if text.contains('\0') {
        return Err(PgError::Configuration(format!(
            "{text:?} holds a NUL byte, which the Postgres protocol cannot carry in a name or statement"
        )));
    }
    Ok(text)
}

fn cstr(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(0);
}

fn count(length: usize, what: &str) -> Result<i16, PgError> {
    i16::try_from(length).map_err(|_| {
        PgError::Configuration(format!(
            "{length} {what} exceed the Postgres protocol's {}",
            i16::MAX
        ))
    })
}

fn length(bytes: usize) -> Result<i32, PgError> {
    i32::try_from(bytes).map_err(|_| {
        PgError::Configuration(format!(
            "a {bytes}-byte value exceeds the Postgres protocol's length"
        ))
    })
}

pub(super) fn ssl_request(out: &mut Vec<u8>) {
    frame(out, None, |out| {
        out.extend_from_slice(&SSL_REQUEST.to_be_bytes())
    });
}

pub(super) fn startup(out: &mut Vec<u8>, parameters: &[(&str, &str)]) -> Result<(), PgError> {
    for (name, value) in parameters {
        checked(name)?;
        checked(value)?;
    }
    frame(out, None, |out| {
        out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        for (name, value) in parameters {
            cstr(out, name);
            cstr(out, value);
        }
        out.push(0);
    });
    Ok(())
}

pub(super) fn password(out: &mut Vec<u8>, password: &str) -> Result<(), PgError> {
    checked(password)?;
    frame(out, Some(tag::PASSWORD), |out| cstr(out, password));
    Ok(())
}

pub(super) fn sasl_initial(out: &mut Vec<u8>, mechanism: &str, data: &[u8]) -> Result<(), PgError> {
    let data_length = length(data.len())?;
    frame(out, Some(tag::PASSWORD), |out| {
        cstr(out, mechanism);
        out.extend_from_slice(&data_length.to_be_bytes());
        out.extend_from_slice(data);
    });
    Ok(())
}

pub(super) fn sasl_response(out: &mut Vec<u8>, data: &[u8]) {
    frame(out, Some(tag::PASSWORD), |out| out.extend_from_slice(data));
}

pub(super) fn query(out: &mut Vec<u8>, sql: &str) -> Result<(), PgError> {
    checked(sql)?;
    frame(out, Some(tag::QUERY), |out| cstr(out, sql));
    Ok(())
}

pub(super) fn parse(
    out: &mut Vec<u8>,
    name: &str,
    sql: &str,
    parameters: usize,
) -> Result<(), PgError> {
    checked(sql)?;
    let declared = count(parameters, "parameters")?;
    frame(out, Some(tag::PARSE), |out| {
        cstr(out, name);
        cstr(out, sql);
        out.extend_from_slice(&declared.to_be_bytes());
        for _ in 0..parameters {
            out.extend_from_slice(&TEXT_OID.to_be_bytes());
        }
    });
    Ok(())
}

pub(super) fn bind(out: &mut Vec<u8>, statement: &str, parameters: &[&str]) -> Result<(), PgError> {
    let declared = count(parameters.len(), "parameters")?;
    for parameter in parameters {
        length(parameter.len())?;
    }
    frame(out, Some(tag::BIND), |out| {
        cstr(out, "");
        cstr(out, statement);
        out.extend_from_slice(&1_i16.to_be_bytes());
        out.extend_from_slice(&TEXT_FORMAT.to_be_bytes());
        out.extend_from_slice(&declared.to_be_bytes());
        for parameter in parameters {
            let bytes = i32::try_from(parameter.len()).expect("checked above");
            out.extend_from_slice(&bytes.to_be_bytes());
            out.extend_from_slice(parameter.as_bytes());
        }
        out.extend_from_slice(&1_i16.to_be_bytes());
        out.extend_from_slice(&TEXT_FORMAT.to_be_bytes());
    });
    Ok(())
}

pub(super) fn execute(out: &mut Vec<u8>) {
    frame(out, Some(tag::EXECUTE), |out| {
        cstr(out, "");
        out.extend_from_slice(&0_i32.to_be_bytes());
    });
}

pub(super) fn sync(out: &mut Vec<u8>) {
    frame(out, Some(tag::SYNC), |_| {});
}

pub(super) fn terminate(out: &mut Vec<u8>) {
    frame(out, Some(tag::TERMINATE), |_| {});
}

pub(super) struct Inbox {
    bytes: Vec<u8>,
    start: usize,
}

impl Inbox {
    pub(super) fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(READ_BUFFER_BYTES),
            start: 0,
        }
    }

    pub(super) fn next(&mut self) -> Result<Option<(u8, Range<usize>)>, PgError> {
        let available = &self.bytes[self.start..];
        let Some(header) = available.get(..HEADER_BYTES) else {
            return Ok(None);
        };
        let tag = header[0];
        let declared = i32::from_be_bytes(header[1..].try_into().expect("four length bytes"));
        let body = usize::try_from(declared)
            .ok()
            .and_then(|declared| declared.checked_sub(LENGTH_BYTES))
            .filter(|&body| body <= MESSAGE_BYTES_MAX)
            .ok_or_else(|| {
                PgError::Protocol(format!(
                    "message {:?} declares length {declared}",
                    tag as char
                ))
            })?;
        if available.len() < HEADER_BYTES + body {
            return Ok(None);
        }
        let begin = self.start + HEADER_BYTES;
        self.start = begin + body;
        Ok(Some((tag, begin..begin + body)))
    }

    pub(super) fn body(&self, range: Range<usize>) -> Body<'_> {
        Body(&self.bytes[range])
    }

    pub(super) fn room(&mut self) -> &mut Vec<u8> {
        if self.start > 0 {
            self.bytes.drain(..self.start);
            self.start = 0;
        }
        if self.bytes.capacity() == self.bytes.len() {
            self.bytes.reserve(READ_BUFFER_BYTES);
        }
        &mut self.bytes
    }
}

pub(super) struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    fn take(&mut self, bytes: usize) -> Result<&'a [u8], PgError> {
        if self.0.len() < bytes {
            return Err(PgError::Protocol(format!(
                "a message ended {} bytes short",
                bytes - self.0.len()
            )));
        }
        let (taken, rest) = self.0.split_at(bytes);
        self.0 = rest;
        Ok(taken)
    }

    pub(super) fn byte(&mut self) -> Result<u8, PgError> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn i16(&mut self) -> Result<i16, PgError> {
        Ok(i16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }

    pub(super) fn i32(&mut self) -> Result<i32, PgError> {
        Ok(i32::from_be_bytes(
            self.take(LENGTH_BYTES)?.try_into().expect("four bytes"),
        ))
    }

    pub(super) fn cstr(&mut self) -> Result<&'a str, PgError> {
        let end = self
            .0
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| PgError::Protocol("a string without its NUL end".to_owned()))?;
        let text = self.take(end)?;
        self.take(1)?;
        std::str::from_utf8(text)
            .map_err(|_| PgError::Protocol("a string that is not UTF-8".to_owned()))
    }

    pub(super) const fn rest(&self) -> &'a [u8] {
        self.0
    }

    pub(super) fn row(mut self) -> Result<SqlRow, PgError> {
        let columns = usize::try_from(self.i16()?)
            .map_err(|_| PgError::Protocol("a data row with a negative column count".to_owned()))?;
        let mut row = Vec::with_capacity(columns);
        for _ in 0..columns {
            let declared = self.i32()?;
            if declared == NULL_LENGTH {
                row.push(None);
                continue;
            }
            let bytes = usize::try_from(declared)
                .map_err(|_| PgError::Protocol(format!("a column of length {declared}")))?;
            let value = std::str::from_utf8(self.take(bytes)?)
                .map_err(|_| PgError::Protocol("a text column that is not UTF-8".to_owned()))?;
            row.push(Some(value.to_owned()));
        }
        Ok(row)
    }

    pub(super) fn error(mut self) -> Result<DatabaseError, PgError> {
        let mut error = DatabaseError::default();
        loop {
            let field = self.byte()?;
            if field == 0 {
                return Ok(error);
            }
            let value = self.cstr()?;
            match field {
                b'V' => value.clone_into(&mut error.severity),
                b'S' if error.severity.is_empty() => value.clone_into(&mut error.severity),
                b'C' => value.clone_into(&mut error.code),
                b'M' => value.clone_into(&mut error.message),
                b'D' => error.detail = Some(value.to_owned()),
                b'H' => error.hint = Some(value.to_owned()),
                _ => {}
            }
        }
    }
}
