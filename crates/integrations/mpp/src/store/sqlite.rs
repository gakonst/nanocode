use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 16] = b"SQLite format 3\0";
const HEADER_BYTES: usize = 100;
const PAGE_SIZE_OFFSET: usize = 16;
const RESERVED_OFFSET: usize = 20;
const TEXT_ENCODING_OFFSET: usize = 56;
const UTF8_ENCODING: u32 = 1;
const UNSET_ENCODING: u32 = 0;
const LARGEST_PAGE_MARKER: usize = 1;
const LARGEST_PAGE_BYTES: usize = 65_536;
const SMALLEST_PAGE_BYTES: usize = 512;
const WAL_SUFFIX: &str = "-wal";
const WAL_HEADER_BYTES: usize = 32;
const WAL_FRAME_HEADER_BYTES: usize = 24;
const WAL_MAGIC_LITTLE_ENDIAN: u32 = 0x377f_0682;
const WAL_MAGIC_BIG_ENDIAN: u32 = 0x377f_0683;
const WAL_CHECKSUMMED_HEADER_BYTES: usize = 24;
const WAL_FRAME_CHECKSUMMED_BYTES: usize = 8;
const CHECKSUM_CHUNK_BYTES: usize = 8;
const WORD_BYTES: usize = 4;
const LEAF_TABLE_PAGE: u8 = 0x0d;
const INTERIOR_TABLE_PAGE: u8 = 0x05;
const LEAF_HEADER_BYTES: usize = 8;
const INTERIOR_HEADER_BYTES: usize = 12;
const CELL_COUNT_OFFSET: usize = 3;
const RIGHT_CHILD_OFFSET: usize = 8;
const SCHEMA_PAGE: u32 = 1;
const VARINT_BYTES_MAX: usize = 9;
const VARINT_PAYLOAD_BITS: u32 = 7;
const VARINT_CONTINUE: u8 = 0x80;
const VARINT_LOW_BITS: u8 = 0x7f;
const MAX_LOCAL_RESERVE: usize = 35;
const MIN_LOCAL_RESERVE: usize = 12;
const MIN_LOCAL_SCALE: usize = 32;
const MIN_LOCAL_DIVISOR: usize = 255;
const MIN_LOCAL_OFFSET: usize = 23;
const OVERFLOW_POINTER_BYTES: usize = 4;
const FIRST_TEXT_SERIAL: u64 = 13;
const FIRST_BLOB_SERIAL: u64 = 12;
const ZERO_SERIAL: u64 = 8;
const ONE_SERIAL: u64 = 9;
const FLOAT_SERIAL: u64 = 7;
const INTEGER_SIZES: [usize; 7] = [0, 1, 2, 3, 4, 6, 8];
const SCHEMA_TYPE: usize = 0;
const SCHEMA_NAME: usize = 1;
const SCHEMA_ROOT: usize = 3;
const SCHEMA_SQL: usize = 4;
const TABLE_CONSTRAINTS: [&str; 5] = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"];

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

struct Database {
    file: Vec<u8>,
    frames: HashMap<u32, Vec<u8>>,
    page_bytes: usize,
    usable_bytes: usize,
    pages: usize,
}

fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
    let slice = bytes
        .get(at..at + WORD_BYTES)
        .ok_or_else(|| format!("truncated at byte {at}"))?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn checksum(data: &[u8], big_endian: bool, (mut first, mut second): (u32, u32)) -> (u32, u32) {
    for chunk in data.as_chunks::<CHECKSUM_CHUNK_BYTES>().0 {
        let read = |at: usize| {
            let bytes = [chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]];
            match big_endian {
                true => u32::from_be_bytes(bytes),
                false => u32::from_le_bytes(bytes),
            }
        };
        first = first.wrapping_add(read(0)).wrapping_add(second);
        second = second.wrapping_add(read(WORD_BYTES)).wrapping_add(first);
    }
    (first, second)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn page_size(marker: usize) -> Result<usize, String> {
    let bytes = match marker {
        LARGEST_PAGE_MARKER => LARGEST_PAGE_BYTES,
        bytes => bytes,
    };
    match (SMALLEST_PAGE_BYTES..=LARGEST_PAGE_BYTES).contains(&bytes) && bytes.is_power_of_two() {
        true => Ok(bytes),
        false => Err(format!("invalid page size {marker}")),
    }
}

fn committed_frames(wal: &[u8], page_bytes: usize) -> Result<HashMap<u32, Vec<u8>>, String> {
    let mut committed = HashMap::new();
    if wal.len() < WAL_HEADER_BYTES {
        return Ok(committed);
    }
    let big_endian = match word(wal, 0)? {
        WAL_MAGIC_BIG_ENDIAN => true,
        WAL_MAGIC_LITTLE_ENDIAN => false,
        _ => return Ok(committed),
    };
    if usize::try_from(word(wal, 8)?).ok() != Some(page_bytes) {
        return Ok(committed);
    }
    let salts = (word(wal, 16)?, word(wal, 20)?);
    let mut sums = checksum(&wal[..WAL_CHECKSUMMED_HEADER_BYTES], big_endian, (0, 0));
    if sums != (word(wal, 24)?, word(wal, 28)?) {
        return Ok(committed);
    }
    let mut pending = HashMap::new();
    let frame_bytes = WAL_FRAME_HEADER_BYTES + page_bytes;
    let mut at = WAL_HEADER_BYTES;
    while at + frame_bytes <= wal.len() {
        let header = &wal[at..at + WAL_FRAME_HEADER_BYTES];
        let page = &wal[at + WAL_FRAME_HEADER_BYTES..at + frame_bytes];
        if (word(header, 8)?, word(header, 12)?) != salts {
            break;
        }
        sums = checksum(&header[..WAL_FRAME_CHECKSUMMED_BYTES], big_endian, sums);
        sums = checksum(page, big_endian, sums);
        if sums != (word(header, 16)?, word(header, 20)?) {
            break;
        }
        pending.insert(word(header, 0)?, page.to_vec());
        if word(header, 4)? != 0 {
            committed.extend(pending.drain());
        }
        at += frame_bytes;
    }
    Ok(committed)
}

impl Database {
    fn open(path: &Path) -> Result<Option<Self>, String> {
        let Some(file) = read_optional(path)? else {
            return Ok(None);
        };
        let mut wal_path = path.as_os_str().to_owned();
        wal_path.push(WAL_SUFFIX);
        let wal = read_optional(&PathBuf::from(wal_path))?.unwrap_or_default();
        let marker = match file.len() >= HEADER_BYTES {
            true => {
                if &file[..MAGIC.len()] != MAGIC {
                    return Err("not a SQLite database".into());
                }
                usize::from(u16::from_be_bytes([
                    file[PAGE_SIZE_OFFSET],
                    file[PAGE_SIZE_OFFSET + 1],
                ]))
            }
            false if wal.len() >= WAL_HEADER_BYTES => {
                usize::try_from(word(&wal, 8)?).map_err(|error| error.to_string())?
            }
            false => return Ok(None),
        };
        let page_bytes = page_size(marker)?;
        let frames = committed_frames(&wal, page_bytes)?;
        let mut database = Self {
            file,
            frames,
            page_bytes,
            usable_bytes: page_bytes,
            pages: 0,
        };
        let first = database.page(SCHEMA_PAGE)?.to_vec();
        if first.len() < HEADER_BYTES || &first[..MAGIC.len()] != MAGIC {
            return Err("not a SQLite database".into());
        }
        database.usable_bytes = page_bytes - usize::from(first[RESERVED_OFFSET]);
        match word(&first, TEXT_ENCODING_OFFSET)? {
            UTF8_ENCODING | UNSET_ENCODING => {}
            other => return Err(format!("unsupported text encoding {other}")),
        }
        let last_frame = database.frames.keys().copied().max().unwrap_or(0);
        database.pages = (database.file.len() / page_bytes)
            .max(usize::try_from(last_frame).map_err(|error| error.to_string())?);
        Ok(Some(database))
    }

    fn page(&self, number: u32) -> Result<&[u8], String> {
        if let Some(page) = self.frames.get(&number) {
            return Ok(page);
        }
        let index = usize::try_from(number)
            .ok()
            .and_then(|number| number.checked_sub(1))
            .ok_or_else(|| format!("invalid page number {number}"))?;
        self.file
            .get(index * self.page_bytes..(index + 1) * self.page_bytes)
            .ok_or_else(|| format!("page {number} is past the end of the file"))
    }

    fn overflow(
        &self,
        mut next: u32,
        total: usize,
        mut payload: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let mut visited = 0;
        while payload.len() < total {
            visited += 1;
            if next == 0 || visited > self.pages {
                return Err("broken overflow chain".into());
            }
            let page = self.page(next)?;
            next = word(page, 0)?;
            let content = &page[OVERFLOW_POINTER_BYTES..self.usable_bytes];
            let wanted = (total - payload.len()).min(content.len());
            payload.extend_from_slice(&content[..wanted]);
        }
        Ok(payload)
    }

    fn payload(&self, page: &[u8], at: usize, total: usize) -> Result<Vec<u8>, String> {
        let usable = self.usable_bytes;
        let largest_local = usable - MAX_LOCAL_RESERVE;
        if total <= largest_local {
            return page
                .get(at..at + total)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| "cell runs past its page".to_owned());
        }
        let smallest_local =
            (usable - MIN_LOCAL_RESERVE) * MIN_LOCAL_SCALE / MIN_LOCAL_DIVISOR - MIN_LOCAL_OFFSET;
        let candidate =
            smallest_local + (total - smallest_local) % (usable - OVERFLOW_POINTER_BYTES);
        let local = match candidate <= largest_local {
            true => candidate,
            false => smallest_local,
        };
        let head = page
            .get(at..at + local)
            .ok_or_else(|| "cell runs past its page".to_owned())?
            .to_vec();
        self.overflow(word(page, at + local)?, total, head)
    }

    fn table(&self, root: u32) -> Result<Vec<Vec<Value>>, String> {
        let mut records = Vec::new();
        let mut pending = vec![root];
        let mut visited = 0;
        while let Some(number) = pending.pop() {
            visited += 1;
            if visited > self.pages {
                return Err("table b-tree has a cycle".into());
            }
            let page = self.page(number)?;
            let offset = match number {
                SCHEMA_PAGE => HEADER_BYTES,
                _ => 0,
            };
            let kind = *page.get(offset).ok_or("empty page")?;
            let cells = usize::from(u16::from_be_bytes([
                page[offset + CELL_COUNT_OFFSET],
                page[offset + CELL_COUNT_OFFSET + 1],
            ]));
            let header = match kind {
                LEAF_TABLE_PAGE => LEAF_HEADER_BYTES,
                INTERIOR_TABLE_PAGE => INTERIOR_HEADER_BYTES,
                other => return Err(format!("page {number} has unexpected type {other}")),
            };
            let mut children = Vec::new();
            for cell in 0..cells {
                let pointer = offset + header + 2 * cell;
                let at = usize::from(u16::from_be_bytes([
                    *page.get(pointer).ok_or("cell pointer past its page")?,
                    *page.get(pointer + 1).ok_or("cell pointer past its page")?,
                ]));
                match kind {
                    LEAF_TABLE_PAGE => {
                        let (total, at) = varint(page, at)?;
                        let (_, at) = varint(page, at)?;
                        let total = usize::try_from(total).map_err(|error| error.to_string())?;
                        records.push(record(&self.payload(page, at, total)?)?);
                    }
                    _ => children.push(word(page, at)?),
                }
            }
            if kind == INTERIOR_TABLE_PAGE {
                children.push(word(page, offset + RIGHT_CHILD_OFFSET)?);
                pending.extend(children.into_iter().rev());
            }
        }
        Ok(records)
    }
}

fn varint(bytes: &[u8], mut at: usize) -> Result<(u64, usize), String> {
    let mut value = 0u64;
    for index in 0..VARINT_BYTES_MAX {
        let byte = *bytes.get(at).ok_or("varint runs past its page")?;
        at += 1;
        if index == VARINT_BYTES_MAX - 1 {
            return Ok(((value << u8::BITS) | u64::from(byte), at));
        }
        value = (value << VARINT_PAYLOAD_BITS) | u64::from(byte & VARINT_LOW_BITS);
        if byte & VARINT_CONTINUE == 0 {
            return Ok((value, at));
        }
    }
    Ok((value, at))
}

fn integer(bytes: &[u8]) -> i64 {
    let negative = bytes
        .first()
        .is_some_and(|byte| byte & VARINT_CONTINUE != 0);
    let start = match negative {
        true => -1i64,
        false => 0,
    };
    bytes
        .iter()
        .fold(start, |value, byte| (value << u8::BITS) | i64::from(*byte))
}

fn record(payload: &[u8]) -> Result<Vec<Value>, String> {
    let (header_bytes, mut at) = varint(payload, 0)?;
    let header_end = usize::try_from(header_bytes).map_err(|error| error.to_string())?;
    let mut types = Vec::new();
    while at < header_end {
        let (serial, next) = varint(payload, at)?;
        types.push(serial);
        at = next;
    }
    let mut body = header_end;
    let mut values = Vec::with_capacity(types.len());
    for serial in types {
        let size = match serial {
            0..=6 => INTEGER_SIZES[usize::try_from(serial).map_err(|error| error.to_string())?],
            FLOAT_SERIAL => INTEGER_SIZES[6],
            ZERO_SERIAL | ONE_SERIAL => 0,
            serial if serial >= FIRST_BLOB_SERIAL => {
                usize::try_from((serial - FIRST_BLOB_SERIAL) / 2)
                    .map_err(|error| error.to_string())?
            }
            other => return Err(format!("reserved serial type {other}")),
        };
        let bytes = payload
            .get(body..body + size)
            .ok_or("record value runs past its payload")?;
        body += size;
        values.push(match serial {
            0 => Value::Null,
            1..=6 => Value::Integer(integer(bytes)),
            FLOAT_SERIAL => {
                let mut array = [0u8; 8];
                array.copy_from_slice(bytes);
                Value::Real(f64::from_be_bytes(array))
            }
            ZERO_SERIAL => Value::Integer(0),
            ONE_SERIAL => Value::Integer(1),
            serial if serial >= FIRST_TEXT_SERIAL && serial % 2 == 1 => {
                Value::Text(String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())?)
            }
            _ => Value::Blob(bytes.to_vec()),
        });
    }
    Ok(values)
}

fn identifier(definition: &str) -> Option<String> {
    let mut characters = definition.chars();
    let close = match characters.next()? {
        '"' => '"',
        '\x60' => '\x60',
        '[' => ']',
        _ => {
            return Some(
                definition
                    .split(|character: char| character.is_whitespace() || character == '(')
                    .next()?
                    .to_owned(),
            );
        }
    };
    let mut name = String::new();
    let mut rest = characters.peekable();
    while let Some(character) = rest.next() {
        if character == close {
            if close != ']' && rest.peek() == Some(&close) {
                rest.next();
                name.push(close);
                continue;
            }
            return Some(name);
        }
        name.push(character);
    }
    None
}

fn columns(sql: &str) -> Result<Vec<String>, String> {
    let open = sql.find('(').ok_or("table definition has no columns")?;
    let close = sql.rfind(')').ok_or("table definition has no columns")?;
    let body = sql
        .get(open + 1..close)
        .ok_or("table definition has no columns")?;
    let mut definitions = Vec::new();
    let (mut depth, mut quote, mut start) = (0usize, None::<char>, 0usize);
    for (index, character) in body.char_indices() {
        match (quote, character) {
            (Some(open), character) if character == open || (open == '[' && character == ']') => {
                quote = None
            }
            (Some(_), _) => {}
            (None, '\'' | '"' | '\x60') => quote = Some(character),
            (None, '[') => quote = Some('['),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                definitions.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    definitions.push(&body[start..]);
    Ok(definitions
        .into_iter()
        .map(str::trim)
        .filter(|definition| !definition.is_empty())
        .filter(|definition| {
            let first = definition
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            !TABLE_CONSTRAINTS.contains(&first.as_str())
        })
        .filter_map(identifier)
        .collect())
}

pub(super) fn read_table(
    path: &Path,
    table: &str,
) -> Result<Option<Vec<BTreeMap<String, Value>>>, String> {
    let Some(database) = Database::open(path)? else {
        return Ok(None);
    };
    let schema = database.table(SCHEMA_PAGE)?;
    let Some(entry) = schema.iter().find(|entry| {
        matches!(entry.get(SCHEMA_TYPE), Some(Value::Text(kind)) if kind == "table")
            && matches!(entry.get(SCHEMA_NAME), Some(Value::Text(name)) if name.eq_ignore_ascii_case(table))
    }) else {
        return Ok(None);
    };
    let root = match entry.get(SCHEMA_ROOT) {
        Some(Value::Integer(root)) => u32::try_from(*root).map_err(|error| error.to_string())?,
        _ => return Err(format!("table {table} has no root page")),
    };
    let names = match entry.get(SCHEMA_SQL) {
        Some(Value::Text(sql)) => columns(sql)?,
        _ => return Err(format!("table {table} has no definition")),
    };
    Ok(Some(
        database
            .table(root)?
            .into_iter()
            .map(|values| names.iter().cloned().zip(values).collect())
            .collect(),
    ))
}
