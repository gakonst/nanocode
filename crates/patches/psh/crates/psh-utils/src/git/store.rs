//! What git reads: files through the utility's file system, and the object database
//! (loose objects, packs and their version 2 indexes, alternates) on top of them.
//!
//! On the process's own file system a pack and its index are read where they are needed
//! (pread); on a host's file system a file is one read, which is what such a host
//! serves. Written for psh from git's documented formats (gitformat-pack(5)).

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{self, Read};
use std::rc::Rc;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
use psh_engine::host::Metadata;
use psh_engine::util::SyncCtx;

/// An object name (SHA-1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct Oid(pub [u8; 20]);

impl Oid {
    pub const ZERO: Oid = Oid([0; 20]);

    pub fn from_hex(s: &[u8]) -> Option<Oid> {
        if s.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, pair) in s.chunks(2).enumerate() {
            out[i] = hex_val(pair[0])? << 4 | hex_val(pair[1])?;
        }
        Some(Oid(out))
    }

    pub fn from_bytes(s: &[u8]) -> Oid {
        let mut out = [0u8; 20];
        out.copy_from_slice(&s[..20]);
        Oid(out)
    }

    pub fn hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 15) as usize] as char);
        }
        s
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The hex digits two names share at their start.
fn common_hex(a: &Oid, b: &Oid) -> usize {
    for i in 0..20 {
        if a.0[i] != b.0[i] {
            return i * 2 + usize::from(a.0[i] >> 4 == b.0[i] >> 4);
        }
    }
    40
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }

    fn from_name(s: &[u8]) -> Option<Kind> {
        match s {
            b"commit" => Some(Kind::Commit),
            b"tree" => Some(Kind::Tree),
            b"blob" => Some(Kind::Blob),
            b"tag" => Some(Kind::Tag),
            _ => None,
        }
    }
}

pub type Data = Rc<Vec<u8>>;

pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(dir.len() + name.len() + 1);
    out.extend_from_slice(dir);
    if out.last() != Some(&b'/') {
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out
}

fn absent(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::IsADirectory
    )
}

pub fn corrupt(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// The utility's file system, read only. Paths are the shell's, absolute.
pub struct Fs<'a> {
    ctx: &'a SyncCtx,
}

impl<'a> Fs<'a> {
    pub fn new(ctx: &'a SyncCtx) -> Fs<'a> {
        Fs { ctx }
    }

    pub fn read(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let mut file = self.ctx.open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        Ok(data)
    }

    /// A file's bytes, or None when there is no such file.
    pub fn read_opt(&self, path: &[u8]) -> io::Result<Option<Vec<u8>>> {
        match self.read(path) {
            Ok(data) => Ok(Some(data)),
            Err(e) if absent(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn stat(&self, path: &[u8], follow: bool) -> io::Result<Metadata> {
        self.ctx.stat(path, follow)
    }

    pub fn stat_opt(&self, path: &[u8], follow: bool) -> io::Result<Option<Metadata>> {
        match self.ctx.stat(path, follow) {
            Ok(m) => Ok(Some(m)),
            Err(e) if absent(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The names in a directory, sorted; empty when there is no such directory.
    pub fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        match self.ctx.read_dir(path) {
            Ok(entries) => {
                let mut names: Vec<Vec<u8>> = entries.into_iter().map(|e| e.name).collect();
                names.sort();
                Ok(names)
            }
            Err(e) if absent(&e) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn read_link(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.ctx.read_link(path)
    }

    /// The physical path where the files are the process's own, else the path as it is.
    pub fn physical(&self, path: &[u8]) -> Vec<u8> {
        #[cfg(unix)]
        if let Some(Ok(native)) = self.ctx.native_path(path)
            && let Ok(real) = std::fs::canonicalize(native)
        {
            use std::os::unix::ffi::OsStrExt;
            return real.as_os_str().as_bytes().to_vec();
        }
        path.to_vec()
    }

    /// A file to read parts of.
    pub fn random(&self, path: &[u8]) -> io::Result<Random> {
        #[cfg(any(unix, windows))]
        if let Some(native) = self.ctx.native_path(path) {
            let file = std::fs::File::open(native?)?;
            let len = file.metadata()?.len();
            return Ok(Random::Os(file, len));
        }
        Ok(Random::Whole(self.read(path)?))
    }
}

/// A file read in parts: held whole, or the process's own file read where asked.
pub enum Random {
    Whole(Vec<u8>),
    #[cfg(any(unix, windows))]
    Os(std::fs::File, u64),
}

impl Random {
    /// Up to len bytes at off: fewer only at the end of the file.
    pub fn upto(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>> {
        match self {
            Random::Whole(data) => {
                let start = usize::try_from(off).unwrap_or(usize::MAX).min(data.len());
                let end = start.saturating_add(len).min(data.len());
                Ok(Cow::Borrowed(&data[start..end]))
            }
            #[cfg(any(unix, windows))]
            Random::Os(file, total) => {
                let len = len.min(usize::try_from(total.saturating_sub(off)).unwrap_or(usize::MAX));
                let mut buf = vec![0u8; len];
                let mut got = 0;
                while got < len {
                    #[cfg(unix)]
                    let n = {
                        use std::os::unix::fs::FileExt;
                        file.read_at(&mut buf[got..], off + got as u64)?
                    };
                    #[cfg(windows)]
                    let n = {
                        use std::os::windows::fs::FileExt;
                        file.seek_read(&mut buf[got..], off + got as u64)?
                    };
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                buf.truncate(got);
                Ok(Cow::Owned(buf))
            }
        }
    }

    /// Exactly len bytes at off.
    pub fn at(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>> {
        let data = self.upto(off, len)?;
        if data.len() != len {
            return Err(corrupt("a pack or index file is shorter than it says"));
        }
        Ok(data)
    }

    fn whole(&self) -> bool {
        matches!(self, Random::Whole(_))
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

struct Pack {
    idx: Random,
    path: Vec<u8>,
    data: OnceCell<Random>,
    fanout: Vec<u32>,
    count: u32,
    /// A multi-pack index names it.
    indexed: bool,
    /// An object was found in it.
    used: Cell<bool>,
}

const IDX_NAMES: u64 = 8 + 1024;

impl Pack {
    fn open(fs: &Fs<'_>, idx_path: &[u8]) -> io::Result<Pack> {
        let idx = fs.random(idx_path)?;
        let head = idx.at(0, 8 + 1024)?;
        if head[..4] != [0xff, b't', b'O', b'c'] || be32(&head[4..]) != 2 {
            return Err(corrupt("a pack index is not version 2"));
        }
        let fanout: Vec<u32> = head[8..].chunks(4).map(be32).collect();
        let count = fanout[255];
        drop(head);
        let mut path = idx_path[..idx_path.len() - 3].to_vec();
        path.extend_from_slice(b"pack");
        Ok(Pack {
            idx,
            path,
            data: OnceCell::new(),
            fanout,
            count,
            indexed: false,
            used: Cell::new(false),
        })
    }

    fn data(&self, fs: &Fs<'_>) -> io::Result<&Random> {
        if let Some(data) = self.data.get() {
            return Ok(data);
        }
        let data = fs.random(&self.path)?;
        Ok(self.data.get_or_init(|| data))
    }

    fn name(&self, i: u32) -> io::Result<Oid> {
        Ok(Oid::from_bytes(
            &self.idx.at(IDX_NAMES + u64::from(i) * 20, 20)?,
        ))
    }

    /// The position of oid among the names, or where it would be.
    fn position(&self, oid: &Oid) -> io::Result<Result<u32, u32>> {
        let first = oid.0[0] as usize;
        let mut lo = if first == 0 {
            0
        } else {
            self.fanout[first - 1]
        };
        let mut hi = self.fanout[first];
        // One read for the last names of the search.
        while hi - lo > 32 {
            let mid = lo + (hi - lo) / 2;
            match self.name(mid)?.cmp(oid) {
                std::cmp::Ordering::Equal => return Ok(Ok(mid)),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        let names = self
            .idx
            .at(IDX_NAMES + u64::from(lo) * 20, (hi - lo) as usize * 20)?;
        for (i, name) in names.chunks(20).enumerate() {
            match name.cmp(&oid.0[..]) {
                std::cmp::Ordering::Equal => return Ok(Ok(lo + i as u32)),
                std::cmp::Ordering::Greater => return Ok(Err(lo + i as u32)),
                std::cmp::Ordering::Less => {}
            }
        }
        Ok(Err(hi))
    }

    fn offset(&self, i: u32) -> io::Result<u64> {
        let n = u64::from(self.count);
        let small = be32(&self.idx.at(IDX_NAMES + n * 24 + u64::from(i) * 4, 4)?);
        if small & 0x8000_0000 == 0 {
            return Ok(u64::from(small));
        }
        let at = IDX_NAMES + n * 28 + u64::from(small & 0x7fff_ffff) * 8;
        let large = self.idx.at(at, 8)?;
        Ok(u64::from(be32(&large)) << 32 | u64::from(be32(&large[4..])))
    }
}

#[derive(Default)]
struct Bases {
    held: HashMap<(usize, u64), (Kind, Data)>,
    bytes: usize,
}

/// git's default for core.deltaBaseCacheLimit (git-config(1)): 96 MiB.
pub const DELTA_BASE_CACHE: usize = 96 * 1024 * 1024;

/// The loose objects of one directory by the first byte of their names.
type Loose = HashMap<(usize, u8), Rc<Vec<Oid>>>;

/// The object database of a repository.
pub struct Odb<'a> {
    fs: &'a Fs<'a>,
    dirs: Vec<Vec<u8>>,
    packs: OnceCell<Vec<Pack>>,
    /// The objects multi-pack indexes list.
    indexed: Cell<u64>,
    loose: RefCell<Loose>,
    bases: RefCell<Bases>,
    base_limit: usize,
    inflater: RefCell<Box<DecompressorOxide>>,
}

impl<'a> Odb<'a> {
    /// The database in objects and the alternates it names.
    pub fn open(fs: &'a Fs<'a>, objects: Vec<u8>, base_limit: usize) -> io::Result<Odb<'a>> {
        let mut dirs = vec![objects];
        let mut i = 0;
        while i < dirs.len() {
            if let Some(list) = fs.read_opt(&join(&dirs[i], b"info/alternates"))? {
                for line in list.split(|&b| b == b'\n') {
                    if line.is_empty() || line[0] == b'#' {
                        continue;
                    }
                    let dir = if line[0] == b'/' {
                        line.to_vec()
                    } else {
                        join(&dirs[i], line)
                    };
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }
            }
            i += 1;
        }
        Ok(Odb {
            fs,
            dirs,
            packs: OnceCell::new(),
            indexed: Cell::new(0),
            loose: RefCell::default(),
            bases: RefCell::default(),
            base_limit,
            inflater: RefCell::new(Box::default()),
        })
    }

    fn packs(&self) -> io::Result<&[Pack]> {
        if let Some(packs) = self.packs.get() {
            return Ok(packs);
        }
        let mut packs = Vec::new();
        let mut indexed = 0;
        for dir in &self.dirs {
            let dir = join(dir, b"pack");
            let names = self.fs.list(&dir)?;
            let mut listed: Vec<Vec<u8>> = Vec::new();
            if names.iter().any(|n| n == b"multi-pack-index") {
                let midx = self.fs.random(&join(&dir, b"multi-pack-index"))?;
                let head = midx.at(0, 12)?;
                if &head[..4] == b"MIDX" {
                    let table = midx.at(12, (usize::from(head[6]) + 1) * 12)?;
                    let chunks: Vec<(&[u8], u64)> = table
                        .chunks(12)
                        .map(|c| {
                            (
                                &c[..4],
                                u64::from(be32(&c[4..])) << 32 | u64::from(be32(&c[8..])),
                            )
                        })
                        .collect();
                    for (i, (id, at)) in chunks.iter().enumerate() {
                        if *id == b"OIDF" {
                            indexed += u64::from(be32(&midx.at(at + 255 * 4, 4)?));
                        } else if *id == b"PNAM"
                            && let Some((_, end)) = chunks.get(i + 1)
                        {
                            let names = midx.at(*at, (end - at) as usize)?;
                            listed.extend(
                                names
                                    .split(|&b| b == 0)
                                    .filter(|n| !n.is_empty())
                                    .map(<[u8]>::to_vec),
                            );
                        }
                    }
                }
            }
            for name in names {
                if name.ends_with(b".idx") {
                    let mut pack = Pack::open(self.fs, &join(&dir, &name))?;
                    pack.indexed = listed.contains(&name);
                    packs.push(pack);
                }
            }
        }
        self.indexed.set(indexed);
        Ok(self.packs.get_or_init(|| packs))
    }

    /// What git 2.50 sizes its default abbreviation by: the objects a multi-pack index
    /// lists, those of the packs outside it, and those of its packs that an object
    /// has been found in so far (git adds such a pack to its list when it first uses
    /// it, and counts the list).
    pub fn approximate_count(&self) -> io::Result<u64> {
        let packs = self.packs()?;
        let counted = packs
            .iter()
            .filter(|p| !p.indexed || p.used.get())
            .map(|p| u64::from(p.count));
        Ok(self.indexed.get() + counted.sum::<u64>())
    }

    fn find(&self, oid: &Oid) -> io::Result<Option<(usize, u64)>> {
        for (i, pack) in self.packs()?.iter().enumerate() {
            if let Ok(pos) = pack.position(oid)? {
                pack.used.set(true);
                return Ok(Some((i, pack.offset(pos)?)));
            }
        }
        Ok(None)
    }

    fn loose_path(dir: &[u8], oid: &Oid) -> Vec<u8> {
        let hex = oid.hex();
        let mut path = join(dir, &hex.as_bytes()[..2]);
        path.push(b'/');
        path.extend_from_slice(&hex.as_bytes()[2..]);
        path
    }

    fn loose(&self, oid: &Oid) -> io::Result<Option<(Kind, Data)>> {
        for dir in &self.dirs {
            let Some(raw) = self.fs.read_opt(&Self::loose_path(dir, oid))? else {
                continue;
            };
            let data = miniz_oxide::inflate::decompress_to_vec_zlib(&raw)
                .map_err(|_| corrupt("a loose object does not inflate"))?;
            let nul = data
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| corrupt("a loose object has no header"))?;
            let space = data[..nul]
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| corrupt("a loose object has no header"))?;
            let kind = Kind::from_name(&data[..space])
                .ok_or_else(|| corrupt("a loose object has an unknown type"))?;
            return Ok(Some((kind, Rc::new(data[nul + 1..].to_vec()))));
        }
        Ok(None)
    }

    /// The loose objects whose names start with the byte first.
    fn loose_names(&self, dir: usize, first: u8) -> io::Result<Rc<Vec<Oid>>> {
        if let Some(names) = self.loose.borrow().get(&(dir, first)) {
            return Ok(names.clone());
        }
        let sub = format!("{first:02x}");
        let mut names = Vec::new();
        for name in self.fs.list(&join(&self.dirs[dir], sub.as_bytes()))? {
            let mut hex = sub.clone().into_bytes();
            hex.extend_from_slice(&name);
            if let Some(oid) = Oid::from_hex(&hex) {
                names.push(oid);
            }
        }
        let names = Rc::new(names);
        self.loose.borrow_mut().insert((dir, first), names.clone());
        Ok(names)
    }

    /// An object, or None when the repository does not have it.
    pub fn read(&self, oid: &Oid) -> io::Result<Option<(Kind, Data)>> {
        match self.find(oid)? {
            Some((pack, off)) => self.packed(pack, off).map(Some),
            None => self.loose(oid),
        }
    }

    pub fn has(&self, oid: &Oid) -> io::Result<bool> {
        if self.find(oid)?.is_some() {
            return Ok(true);
        }
        for dir in &self.dirs {
            if self
                .fs
                .stat_opt(&Self::loose_path(dir, oid), true)?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn inflate(&self, data: &Random, off: u64, size: usize) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; size.max(1)];
        let mut state = self.inflater.borrow_mut();
        state.init();
        let mut pos = off;
        let mut written = 0;
        // A stream is seldom longer than what it holds; stored blocks add five bytes
        // to each 65,535.
        let mut want = if data.whole() {
            usize::MAX
        } else {
            size + size / 8192 + 64
        };
        loop {
            let chunk = data.upto(pos, want)?;
            let more = !data.whole() && chunk.len() == want;
            let mut flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
                | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
            if more {
                flags |= inflate_flags::TINFL_FLAG_HAS_MORE_INPUT;
            }
            let (status, used, wrote) = decompress(&mut state, &chunk, &mut out, written, flags);
            written += wrote;
            pos += used as u64;
            match status {
                TINFLStatus::Done => break,
                TINFLStatus::NeedsMoreInput if more => want = 4096,
                _ => return Err(corrupt("a packed object does not inflate")),
            }
        }
        if written != size {
            return Err(corrupt("a packed object is not the size its header says"));
        }
        out.truncate(size);
        Ok(out)
    }

    fn packed(&self, mut pack: usize, mut off: u64) -> io::Result<(Kind, Data)> {
        let packs = self.packs()?;
        let mut deltas: Vec<((usize, u64), Vec<u8>)> = Vec::new();
        let (kind, mut data) = loop {
            if let Some(hit) = self.bases.borrow().held.get(&(pack, off)) {
                break hit.clone();
            }
            let file = packs[pack].data(self.fs)?;
            let head = file.upto(off, 64)?;
            let mut i = 0;
            let mut next = || -> io::Result<u8> {
                let b = *head
                    .get(i)
                    .ok_or_else(|| corrupt("a pack ends inside an object header"))?;
                i += 1;
                Ok(b)
            };
            let mut c = next()?;
            let kind = (c >> 4) & 7;
            let mut size = u64::from(c & 15);
            let mut shift = 4;
            while c & 0x80 != 0 {
                c = next()?;
                size |= u64::from(c & 0x7f) << shift;
                shift += 7;
            }
            let size = usize::try_from(size).map_err(|_| corrupt("an object is too large"))?;
            match kind {
                1..=4 => {
                    let kind = [Kind::Commit, Kind::Tree, Kind::Blob, Kind::Tag][kind as usize - 1];
                    let at = off + i as u64;
                    break (kind, Rc::new(self.inflate(file, at, size)?));
                }
                6 => {
                    let mut c = next()?;
                    let mut back = u64::from(c & 0x7f);
                    while c & 0x80 != 0 {
                        c = next()?;
                        back = ((back + 1) << 7) | u64::from(c & 0x7f);
                    }
                    let at = off + i as u64;
                    deltas.push(((pack, off), self.inflate(file, at, size)?));
                    off = off
                        .checked_sub(back)
                        .ok_or_else(|| corrupt("a delta's base is before its pack"))?;
                }
                7 => {
                    if head.len() < i + 20 {
                        return Err(corrupt("a pack ends inside an object header"));
                    }
                    let base = Oid::from_bytes(&head[i..i + 20]);
                    let at = off + i as u64 + 20;
                    deltas.push(((pack, off), self.inflate(file, at, size)?));
                    match self.find(&base)? {
                        Some((p, o)) => {
                            pack = p;
                            off = o;
                        }
                        None => {
                            break self
                                .loose(&base)?
                                .ok_or_else(|| corrupt("a delta's base object is missing"))?;
                        }
                    }
                }
                _ => return Err(corrupt("a packed object has an unknown type")),
            }
        };
        let mut base = (pack, off);
        while let Some((at, delta)) = deltas.pop() {
            self.hold(base, kind, &data);
            data = Rc::new(apply_delta(&data, &delta)?);
            base = at;
        }
        Ok((kind, data))
    }

    fn hold(&self, at: (usize, u64), kind: Kind, data: &Data) {
        let mut bases = self.bases.borrow_mut();
        if bases.held.contains_key(&at) {
            return;
        }
        if bases.bytes + data.len() > self.base_limit {
            bases.held.clear();
            bases.bytes = 0;
        }
        if data.len() <= self.base_limit {
            bases.bytes += data.len();
            bases.held.insert(at, (kind, data.clone()));
        }
    }

    /// The shortest prefix of the name, of at least min digits, that names no other
    /// object.
    pub fn abbrev(&self, oid: &Oid, min: usize) -> io::Result<String> {
        let mut len = min;
        let mut other = |o: &Oid| {
            if o != oid {
                len = len.max(common_hex(oid, o) + 1);
            }
        };
        for pack in self.packs()? {
            let pos = match pack.position(oid)? {
                Ok(pos) | Err(pos) => pos,
            };
            for i in [pos.wrapping_sub(1), pos, pos.wrapping_add(1)] {
                if i < pack.count {
                    other(&pack.name(i)?);
                }
            }
        }
        for dir in 0..self.dirs.len() {
            for o in self.loose_names(dir, oid.0[0])?.iter() {
                other(o);
            }
        }
        let mut hex = oid.hex();
        hex.truncate(len.min(40));
        Ok(hex)
    }

    /// The objects whose names start with the hex digits of prefix.
    pub fn with_prefix(&self, prefix: &[u8]) -> io::Result<Vec<Oid>> {
        let mut low = [0u8; 20];
        let mut high = [0xffu8; 20];
        for (i, &c) in prefix.iter().enumerate() {
            let v = hex_val(c).unwrap_or(0);
            if i % 2 == 0 {
                low[i / 2] = v << 4;
                high[i / 2] = v << 4 | 15;
            } else {
                low[i / 2] |= v;
                high[i / 2] = low[i / 2];
            }
        }
        let (low, high) = (Oid(low), Oid(high));
        let mut found = Vec::new();
        for pack in self.packs()? {
            let mut pos = match pack.position(&low)? {
                Ok(pos) | Err(pos) => pos,
            };
            while pos < pack.count {
                let name = pack.name(pos)?;
                if name > high {
                    break;
                }
                found.push(name);
                pos += 1;
            }
        }
        if prefix.len() >= 2 {
            for dir in 0..self.dirs.len() {
                for o in self.loose_names(dir, low.0[0])?.iter() {
                    if *o >= low && *o <= high {
                        found.push(*o);
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        Ok(found)
    }
}

fn apply_delta(base: &[u8], delta: &[u8]) -> io::Result<Vec<u8>> {
    let bad = || corrupt("a delta does not fit its base");
    let mut i = 0;
    let mut varint = || -> io::Result<usize> {
        let mut value = 0usize;
        let mut shift = 0;
        loop {
            let c = *delta.get(i).ok_or_else(bad)?;
            i += 1;
            value |= usize::from(c & 0x7f) << shift;
            shift += 7;
            if c & 0x80 == 0 {
                return Ok(value);
            }
        }
    };
    if varint()? != base.len() {
        return Err(bad());
    }
    let size = varint()?;
    let mut out = Vec::with_capacity(size);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let mut off = 0usize;
            let mut len = 0usize;
            for bit in 0..4 {
                if op & (1 << bit) != 0 {
                    off |= usize::from(*delta.get(i).ok_or_else(bad)?) << (8 * bit);
                    i += 1;
                }
            }
            for bit in 0..3 {
                if op & (0x10 << bit) != 0 {
                    len |= usize::from(*delta.get(i).ok_or_else(bad)?) << (8 * bit);
                    i += 1;
                }
            }
            if len == 0 {
                len = 0x10000;
            }
            out.extend_from_slice(base.get(off..off + len).ok_or_else(bad)?);
        } else if op != 0 {
            let len = usize::from(op);
            out.extend_from_slice(delta.get(i..i + len).ok_or_else(bad)?);
            i += len;
        } else {
            return Err(bad());
        }
    }
    if out.len() != size {
        return Err(bad());
    }
    Ok(out)
}
