//! zip and unzip (nanocode's addition), after Info-ZIP's Zip 3.0 and UnZip 6.0: zip
//! with -r -q -j -x -y -D -X and the levels, adding to or updating an archive that
//! exists; unzip with -l -o -n -q -p -t -j -d -x -Z1 and member patterns. Stored and
//! deflated members; the Unix modes and times Info-ZIP records. No encryption and no
//! zip64, which are answered as missing. The archive is held in memory, and a
//! directory's members are written in name order (Info-ZIP: as the directory lists
//! them).

use psh_engine::host::{FileKind, OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::codec::{crc32, deflate, inflate};
use crate::common::Input;
use crate::fsutil::join;
use crate::git::wildmatch;
use crate::timefmt::{self, Tm};

struct Member {
    name: Vec<u8>,
    method: u16,
    crc: u32,
    size: u32,
    /// As the archive holds it: deflated or stored.
    data: Vec<u8>,
    mtime: i64,
    /// The Unix mode with its file type bits.
    mode: u32,
    uid: u32,
    gid: u32,
    /// The time as DOS keeps it, for a member with no Unix time.
    dos: (u16, u16),
    has_time: bool,
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn dos_time(tm: &Tm) -> (u16, u16) {
    if tm.year < 1980 {
        return (0, 0x21);
    }
    let time = (tm.hour << 11 | tm.min << 5 | (tm.sec / 2)) as u16;
    let date = (((tm.year - 1980) as u32) << 9 | tm.mon << 5 | tm.day) as u16;
    (time, date)
}

/// Reads an archive's members from its central directory.
fn read(data: &[u8]) -> Result<Vec<Member>, &'static str> {
    let bad = "zipfile is corrupt";
    if data.len() < 22 {
        return Err("End-of-central-directory signature not found");
    }
    let mut at = data.len() - 22;
    loop {
        if data[at..].starts_with(b"PK\x05\x06") {
            break;
        }
        if at == 0 {
            return Err("End-of-central-directory signature not found");
        }
        at -= 1;
    }
    let count = usize::from(le16(&data[at + 10..]));
    let mut pos = le32(&data[at + 16..]) as usize;
    if count == 0xffff || pos == 0xffff_ffff {
        return Err("zip64");
    }
    let mut members = Vec::with_capacity(count);
    for _ in 0..count {
        let h = data.get(pos..pos + 46).ok_or(bad)?;
        if !h.starts_with(b"PK\x01\x02") {
            return Err(bad);
        }
        let made_by = h[5];
        let flags = le16(&h[8..]);
        let method = le16(&h[10..]);
        let dos = (le16(&h[12..]), le16(&h[14..]));
        let crc = le32(&h[16..]);
        let packed = le32(&h[20..]) as usize;
        let size = le32(&h[24..]);
        let (n, m, k) = (
            usize::from(le16(&h[28..])),
            usize::from(le16(&h[30..])),
            usize::from(le16(&h[32..])),
        );
        let external = le32(&h[38..]);
        let local = le32(&h[42..]) as usize;
        if flags & 1 != 0 {
            return Err("encryption");
        }
        if packed == 0xffff_ffff || size == 0xffff_ffff || local == 0xffff_ffff {
            return Err("zip64");
        }
        let name = data.get(pos + 46..pos + 46 + n).ok_or(bad)?.to_vec();
        let extra = data.get(pos + 46 + n..pos + 46 + n + m).ok_or(bad)?;
        let mut mtime = 0i64;
        let mut has_time = false;
        let (mut uid, mut gid) = (0, 0);
        let mut e = 0;
        while e + 4 <= extra.len() {
            let id = le16(&extra[e..]);
            let len = usize::from(le16(&extra[e + 2..]));
            let body = extra.get(e + 4..e + 4 + len).unwrap_or_default();
            if id == 0x5455 && body.len() >= 5 && body[0] & 1 != 0 {
                mtime = i64::from(le32(&body[1..]) as i32);
                has_time = true;
            }
            if id == 0x7875 && body.len() >= 11 && body[1] == 4 && body[6] == 4 {
                uid = le32(&body[2..]);
                gid = le32(&body[7..]);
            }
            e += 4 + len;
        }
        let lh = data.get(local..local + 30).ok_or(bad)?;
        if !lh.starts_with(b"PK\x03\x04") {
            return Err(bad);
        }
        let start = local + 30 + usize::from(le16(&lh[26..])) + usize::from(le16(&lh[28..]));
        let body = data.get(start..start + packed).ok_or(bad)?.to_vec();
        let dir = name.ends_with(b"/");
        let mode = if made_by == 3 && external >> 16 != 0 {
            external >> 16
        } else if dir {
            0o40755
        } else {
            0o100644
        };
        members.push(Member {
            name,
            method,
            crc,
            size,
            data: body,
            mtime,
            mode,
            uid,
            gid,
            dos,
            has_time,
        });
        pos += 46 + n + m + k;
    }
    Ok(members)
}

/// An archive of the members, as Zip 3.0 lays it out on Unix.
fn write(members: &[Member], zone: &dyn Fn(i64) -> Tm, plain: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for m in members {
        let offset = out.len() as u32;
        let (time, date) = if m.has_time {
            dos_time(&zone(m.mtime))
        } else {
            m.dos
        };
        let needed: u16 = if m.method == 8 || m.name.ends_with(b"/") {
            20
        } else {
            10
        };
        let mut ux = vec![0x75, 0x78, 11, 0, 1, 4];
        ux.extend_from_slice(&m.uid.to_le_bytes());
        ux.push(4);
        ux.extend_from_slice(&m.gid.to_le_bytes());
        let stamp = (m.mtime as i32).to_le_bytes();
        let mut local_extra = Vec::new();
        let mut central_extra = Vec::new();
        if !plain && m.has_time {
            local_extra.extend_from_slice(&[0x55, 0x54, 9, 0, 3]);
            local_extra.extend_from_slice(&stamp);
            local_extra.extend_from_slice(&stamp);
            local_extra.extend_from_slice(&ux);
            central_extra.extend_from_slice(&[0x55, 0x54, 5, 0, 3]);
            central_extra.extend_from_slice(&stamp);
            central_extra.extend_from_slice(&ux);
        }
        let mut fixed = Vec::new();
        fixed.extend_from_slice(&needed.to_le_bytes());
        fixed.extend_from_slice(&0u16.to_le_bytes());
        fixed.extend_from_slice(&m.method.to_le_bytes());
        fixed.extend_from_slice(&time.to_le_bytes());
        fixed.extend_from_slice(&date.to_le_bytes());
        fixed.extend_from_slice(&m.crc.to_le_bytes());
        fixed.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        fixed.extend_from_slice(&m.size.to_le_bytes());
        fixed.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&fixed);
        out.extend_from_slice(&(local_extra.len() as u16).to_le_bytes());
        out.extend_from_slice(&m.name);
        out.extend_from_slice(&local_extra);
        out.extend_from_slice(&m.data);
        central.extend_from_slice(b"PK\x01\x02\x1e\x03");
        central.extend_from_slice(&fixed);
        central.extend_from_slice(&(central_extra.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 6]);
        let dos_dir = if m.name.ends_with(b"/") { 0x10 } else { 0 };
        central.extend_from_slice(&(m.mode << 16 | dos_dir).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(&m.name);
        central.extend_from_slice(&central_extra);
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

async fn lacks(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], what: &str) -> i32 {
    if let Some(status) = ctx.run_program(argv.to_vec()).await {
        return status;
    }
    ctx.error(&format!(
        "{what} (this simulated shell has no {} for it: stored and deflated members, no encryption, no zip64)",
        ctx.name()
    ))
    .await;
    127
}

fn matches_any(patterns: &[Vec<u8>], name: &[u8]) -> bool {
    patterns.iter().any(|p| wildmatch(p, name, false))
}

pub async fn zip(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let (mut recurse, mut quiet, mut junk, mut links, mut no_dirs, mut plain) =
        (false, false, false, false, false, false);
    let mut level = 6u8;
    let mut excludes: Vec<Vec<u8>> = Vec::new();
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut excluding = false;
    for arg in &argv[1..] {
        if arg.len() > 1 && arg[0] == b'-' {
            excluding = false;
            for &c in &arg[1..] {
                match c {
                    b'r' => recurse = true,
                    b'q' => quiet = true,
                    b'j' => junk = true,
                    b'y' => links = true,
                    b'D' => no_dirs = true,
                    b'X' => plain = true,
                    b'x' => excluding = true,
                    b'0'..=b'9' => level = c - b'0',
                    b'o' | b'v' => {}
                    _ => {
                        let text = String::from_utf8_lossy(arg).into_owned();
                        return lacks(ctx, argv, &format!("the option {text}")).await;
                    }
                }
            }
        } else if excluding {
            excludes.push(arg.clone());
        } else {
            operands.push(arg.clone());
        }
    }
    let Some((archive, paths)) = operands.split_first() else {
        ctx.error_raw(b"\nzip error: Nothing to do!\n").await;
        return 12;
    };
    let mut archive = archive.clone();
    if !archive
        .rsplit(|&b| b == b'/')
        .next()
        .is_some_and(|base| base.contains(&b'.'))
    {
        archive.extend_from_slice(b".zip");
    }
    let shown_archive = String::from_utf8_lossy(&archive).into_owned();
    let mut members = Vec::new();
    if ctx.stat(&archive, true).await.is_ok() {
        let mut data = Vec::new();
        let read_in = match Input::open(ctx, &archive).await {
            Ok(input) => input.read_to_end(ctx, &mut data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read_in {
            ctx.error_raw(
                format!(
                    "\nzip error: Could not open {shown_archive} ({})\n",
                    error_text(&e)
                )
                .as_bytes(),
            )
            .await;
            return 15;
        }
        members = match read(&data) {
            Ok(members) => members,
            Err(what @ ("zip64" | "encryption")) => return lacks(ctx, argv, what).await,
            Err(what) => {
                ctx.error_raw(
                    format!("\nzip error: Zip file structure invalid ({shown_archive}): {what}\n")
                        .as_bytes(),
                )
                .await;
                return 3;
            }
        };
    }
    let mut changed = false;
    let mut stack: Vec<(Vec<u8>, bool)> = paths.iter().rev().map(|p| (p.clone(), true)).collect();
    while let Some((path, named)) = stack.pop() {
        ctx.check().await;
        let meta = match ctx.stat(&path, !links).await {
            Ok(meta) => meta,
            Err(_) => {
                ctx.error_raw(
                    format!(
                        "\tzip warning: name not matched: {}\n",
                        String::from_utf8_lossy(&path)
                    )
                    .as_bytes(),
                )
                .await;
                continue;
            }
        };
        let mut name = path.clone();
        while name.starts_with(b"./") {
            name.drain(..2);
        }
        while name.first() == Some(&b'/') {
            name.remove(0);
        }
        let is_dir = meta.kind == FileKind::Dir;
        if junk {
            name = name
                .rsplit(|&b| b == b'/')
                .next()
                .unwrap_or_default()
                .to_vec();
        }
        if is_dir {
            if recurse {
                match ctx.read_dir(&path).await {
                    Ok(entries) => {
                        let mut names: Vec<Vec<u8>> = entries.into_iter().map(|e| e.name).collect();
                        names.sort();
                        for child in names.into_iter().rev() {
                            stack.push((join(&path, &child), false));
                        }
                    }
                    Err(e) => {
                        ctx.error_raw(
                            format!(
                                "\tzip warning: {}: {}\n",
                                String::from_utf8_lossy(&path),
                                error_text(&e)
                            )
                            .as_bytes(),
                        )
                        .await;
                    }
                }
            }
            if junk || no_dirs || name.is_empty() || name == b"." {
                continue;
            }
            name.push(b'/');
        }
        let _ = named;
        if matches_any(&excludes, &name)
            || matches_any(&excludes, name.strip_suffix(b"/").unwrap_or(&name))
        {
            continue;
        }
        if path == archive {
            continue;
        }
        let content = match meta.kind {
            FileKind::Dir => Vec::new(),
            FileKind::Symlink => match ctx.read_link(&path).await {
                Ok(target) => target,
                Err(_) => continue,
            },
            _ => {
                let mut data = Vec::new();
                let read_in = match Input::open(ctx, &path).await {
                    Ok(input) => input.read_to_end(ctx, &mut data).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = read_in {
                    ctx.error_raw(
                        format!(
                            "\tzip warning: could not open for reading: {} ({})\n",
                            String::from_utf8_lossy(&path),
                            error_text(&e)
                        )
                        .as_bytes(),
                    )
                    .await;
                    continue;
                }
                data
            }
        };
        let Ok(size) = u32::try_from(content.len()) else {
            return lacks(ctx, argv, "zip64").await;
        };
        let packed = if level == 0 || content.is_empty() || meta.kind == FileKind::Symlink {
            None
        } else {
            Some(deflate(&content, level)).filter(|p| p.len() < content.len())
        };
        let (method, data) = match packed {
            Some(packed) => (8, packed),
            None => (0, content.clone()),
        };
        let kind_bits = match meta.kind {
            FileKind::Dir => 0o40000,
            FileKind::Symlink => 0o120000,
            _ => 0o100000,
        };
        let saved = if size == 0 {
            0
        } else {
            let (n, m) = (u64::from(size), data.len() as u64);
            ((n - m.min(n)) * 100 + n / 2) / n
        };
        let member = Member {
            name: name.clone(),
            method,
            crc: crc32(&content),
            size,
            data,
            mtime: meta.mtime_ns.div_euclid(1_000_000_000),
            mode: kind_bits | (meta.mode & 0o7777),
            uid: meta.uid,
            gid: meta.gid,
            dos: (0, 0),
            has_time: true,
        };
        let how = if method == 8 { "deflated" } else { "stored" };
        let verb = match members.iter().position(|m| m.name == name) {
            Some(at) => {
                members[at] = member;
                "updating:"
            }
            None => {
                members.push(member);
                "  adding:"
            }
        };
        changed = true;
        if !quiet {
            let line = format!(
                "{verb} {} ({how} {saved}%)\n",
                String::from_utf8_lossy(&name)
            );
            let _ = ctx.write(line.as_bytes()).await;
        }
    }
    if !changed {
        ctx.error_raw(format!("\nzip error: Nothing to do! ({shown_archive})\n").as_bytes())
            .await;
        return 12;
    }
    let offsets: Vec<i64> = members.iter().map(|m| m.mtime).collect();
    let zones: Vec<Tm> = offsets
        .iter()
        .map(|&t| timefmt::local(ctx, t * 1_000_000_000))
        .collect();
    let zone = |t: i64| -> Tm {
        offsets
            .iter()
            .position(|&o| o == t)
            .map_or_else(|| timefmt::tm_at(t * 1_000_000_000, 0), |i| zones[i])
    };
    let data = write(&members, &zone, plain);
    let written = match ctx.open(&archive, OpenOptions::write()).await {
        Ok(file) => file.write_all(&data).await,
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        ctx.error_raw(
            format!(
                "\nzip error: Could not create output file ({shown_archive}): {}\n",
                error_text(&e)
            )
            .as_bytes(),
        )
        .await;
        return 15;
    }
    0
}

pub async fn unzip(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let (mut list, mut overwrite, mut never, mut pipe, mut test, mut junk, mut names_only) =
        (false, false, false, false, false, false, false);
    let mut quiet = 0;
    let mut dir: Option<Vec<u8>> = None;
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut excludes: Vec<Vec<u8>> = Vec::new();
    let mut excluding = false;
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() > 1 && arg[0] == b'-' {
            let mut k = 1;
            while k < arg.len() {
                let c = arg[k];
                k += 1;
                match c {
                    b'l' => list = true,
                    b'o' => overwrite = true,
                    b'n' => never = true,
                    b'q' => quiet += 1,
                    b'p' => pipe = true,
                    b't' => test = true,
                    b'j' => junk = true,
                    b'x' => excluding = true,
                    b'Z' => {
                        if arg.get(k) == Some(&b'1') {
                            names_only = true;
                            k += 1;
                        } else {
                            return lacks(ctx, argv, "zipinfo's listings").await;
                        }
                    }
                    b'd' => {
                        excluding = false;
                        if k < arg.len() {
                            dir = Some(arg[k..].to_vec());
                        } else if i < argv.len() {
                            dir = Some(argv[i].clone());
                            i += 1;
                        }
                        k = arg.len();
                    }
                    _ => {
                        let text = String::from_utf8_lossy(arg).into_owned();
                        return lacks(ctx, argv, &format!("the option {text}")).await;
                    }
                }
            }
        } else if excluding {
            excludes.push(arg.clone());
        } else {
            operands.push(arg.clone());
        }
    }
    let Some((archive, patterns)) = operands.split_first() else {
        ctx.error_raw(b"UnZip 6.00, by Info-ZIP.  usage: unzip [-lopqtjn] file[.zip] [list] [-x xlist] [-d exdir]\n")
            .await;
        return 10;
    };
    let shown = String::from_utf8_lossy(archive).into_owned();
    let mut found = None;
    for suffix in ["", ".zip", ".ZIP"] {
        let mut name = archive.clone();
        name.extend_from_slice(suffix.as_bytes());
        if ctx
            .stat(&name, true)
            .await
            .is_ok_and(|m| m.kind != FileKind::Dir)
        {
            found = Some(name);
            break;
        }
    }
    let Some(path) = found else {
        ctx.error_raw(
            format!("unzip:  cannot find or open {shown}, {shown}.zip or {shown}.ZIP.\n")
                .as_bytes(),
        )
        .await;
        return 9;
    };
    let mut data = Vec::new();
    let read_in = match Input::open(ctx, &path).await {
        Ok(input) => input.read_to_end(ctx, &mut data).await,
        Err(e) => Err(e),
    };
    if read_in.is_err() {
        ctx.error_raw(
            format!("unzip:  cannot find or open {shown}, {shown}.zip or {shown}.ZIP.\n")
                .as_bytes(),
        )
        .await;
        return 9;
    }
    let members = match read(&data) {
        Ok(members) => members,
        Err(what @ ("zip64" | "encryption")) => return lacks(ctx, argv, what).await,
        Err(what) => {
            let text = format!(
                "  {what}.\nunzip:  cannot find zipfile directory in one of {shown} or\n        {shown}.zip, and cannot find {shown}.ZIP, period.\n"
            );
            ctx.error_raw(text.as_bytes()).await;
            return 9;
        }
    };
    let chosen: Vec<&Member> = members
        .iter()
        .filter(|m| {
            (patterns.is_empty() || matches_any(patterns, &m.name))
                && !matches_any(&excludes, &m.name)
        })
        .collect();
    let mut status = 0;
    let mut out = Vec::new();
    let header = format!("Archive:  {}\n", String::from_utf8_lossy(&path));
    if names_only {
        for m in &chosen {
            out.extend_from_slice(&m.name);
            out.push(b'\n');
        }
        let _ = ctx.write(&out).await;
        return 0;
    }
    if list {
        if quiet == 0 {
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(b"  Length      Date    Time    Name\n");
            out.extend_from_slice(b"---------  ---------- -----   ----\n");
        }
        let mut total = 0u64;
        for m in &chosen {
            let tm = if m.has_time {
                timefmt::local(ctx, m.mtime * 1_000_000_000)
            } else {
                Tm {
                    year: i64::from(m.dos.1 >> 9) + 1980,
                    mon: u32::from(m.dos.1 >> 5 & 15),
                    day: u32::from(m.dos.1 & 31),
                    hour: u32::from(m.dos.0 >> 11),
                    min: u32::from(m.dos.0 >> 5 & 63),
                    sec: 0,
                    nsec: 0,
                    wday: 0,
                    yday: 0,
                    offset: 0,
                }
            };
            total += u64::from(m.size);
            out.extend_from_slice(
                format!(
                    "{:>9}  {:02}-{:02}-{:04} {:02}:{:02}   ",
                    m.size, tm.mon, tm.day, tm.year, tm.hour, tm.min
                )
                .as_bytes(),
            );
            out.extend_from_slice(&m.name);
            out.push(b'\n');
        }
        if quiet == 0 {
            out.extend_from_slice(b"---------                     -------\n");
            out.extend_from_slice(
                format!(
                    "{total:>9}                     {} file{}\n",
                    chosen.len(),
                    if chosen.len() == 1 { "" } else { "s" }
                )
                .as_bytes(),
            );
        }
        let _ = ctx.write(&out).await;
        return 0;
    }
    if !pipe && quiet == 0 {
        let _ = ctx.write(header.as_bytes()).await;
    }
    let mut none = never;
    let mut all = overwrite;
    let mut errors = 0;
    for m in &chosen {
        ctx.check().await;
        let content = match m.method {
            0 => Some(m.data.clone()),
            8 => inflate(&m.data).map(|(plain, _)| plain),
            _ => None,
        };
        let name_text = String::from_utf8_lossy(&m.name).into_owned();
        let Some(content) = content.filter(|c| crc32(c) == m.crc) else {
            ctx.error_raw(
                format!(
                    "   skipping: {name_text:<22}  unsupported compression method or bad CRC\n"
                )
                .as_bytes(),
            )
            .await;
            errors += 1;
            continue;
        };
        if test {
            if quiet == 0 {
                let _ = ctx
                    .write(format!("    testing: {name_text:<22}   OK\n").as_bytes())
                    .await;
            }
            continue;
        }
        if pipe {
            if let Err(e) = ctx.write(&content).await {
                return crate::common::write_failed(ctx, &e).await;
            }
            continue;
        }
        let is_dir = m.name.ends_with(b"/");
        let mut rel = m.name.clone();
        if junk {
            if is_dir {
                continue;
            }
            rel = rel
                .rsplit(|&b| b == b'/')
                .next()
                .unwrap_or_default()
                .to_vec();
        }
        while rel.first() == Some(&b'/') {
            rel.remove(0);
        }
        if rel.split(|&b| b == b'/').any(|part| part == b"..") {
            ctx.error_raw(
                format!("warning:  skipped \"../\" path component(s) in {name_text}\n").as_bytes(),
            )
            .await;
            let kept: Vec<&[u8]> = rel.split(|&b| b == b'/').filter(|p| *p != b"..").collect();
            rel = kept.join(&b'/');
        }
        let target = match &dir {
            Some(dir) => join(dir, &rel),
            None => rel.clone(),
        };
        let target_text = String::from_utf8_lossy(&target).into_owned();
        // The directories above it.
        let mut at = Vec::new();
        let parts: Vec<&[u8]> = target.split(|&b| b == b'/').collect();
        let last = if is_dir { parts.len() } else { parts.len() - 1 };
        for (n, part) in parts[..last].iter().enumerate() {
            if n > 0 {
                at.push(b'/');
            }
            at.extend_from_slice(part);
            if !part.is_empty() && ctx.stat(&at, true).await.is_err() {
                let _ = ctx.mkdir(&at, 0o777 & !ctx.umask()).await;
            }
        }
        if is_dir {
            if quiet == 0 {
                let _ = ctx
                    .write(format!("   creating: {target_text}\n").as_bytes())
                    .await;
            }
            continue;
        }
        if ctx.stat(&target, false).await.is_ok() {
            if none {
                continue;
            }
            if !all {
                ctx.error_raw(
                    format!("replace {target_text}? [y]es, [n]o, [A]ll, [N]one, [r]ename: ")
                        .as_bytes(),
                )
                .await;
                let mut answer = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    match ctx.read(&mut byte).await {
                        Ok(1) if byte[0] != b'\n' => answer.push(byte[0]),
                        _ => break,
                    }
                }
                match answer.first() {
                    Some(b'y' | b'Y') if answer[0] == b'y' => {}
                    Some(b'A') => all = true,
                    Some(b'n') => continue,
                    Some(b'N') => {
                        none = true;
                        continue;
                    }
                    Some(_) => continue,
                    None => {
                        ctx.error_raw(b" NULL\n(EOF or read error, treating as \"[N]one\" ...)\n")
                            .await;
                        none = true;
                        status = 1;
                        continue;
                    }
                }
            }
            let _ = ctx.remove_file(&target).await;
        }
        let kind = m.mode & 0o170000;
        let result = if kind == 0o120000 {
            ctx.symlink(&content, &target).await
        } else {
            let options = OpenOptions {
                mode: Some(m.mode & 0o777 & !ctx.umask()),
                ..OpenOptions::write()
            };
            match ctx.open(&target, options).await {
                Ok(file) => file.write_all(&content).await,
                Err(e) => Err(e),
            }
        };
        match result {
            Ok(()) => {
                if kind != 0o120000 {
                    let stamp = if m.has_time {
                        m.mtime * 1_000_000_000
                    } else {
                        let tm = Tm {
                            year: i64::from(m.dos.1 >> 9) + 1980,
                            mon: u32::from(m.dos.1 >> 5 & 15).max(1),
                            day: u32::from(m.dos.1 & 31).max(1),
                            hour: u32::from(m.dos.0 >> 11),
                            min: u32::from(m.dos.0 >> 5 & 63),
                            sec: u32::from(m.dos.0 & 31) * 2,
                            nsec: 0,
                            wday: 0,
                            yday: 0,
                            offset: 0,
                        };
                        let wall =
                            timefmt::wall_secs(tm.year, tm.mon, tm.day, tm.hour, tm.min, tm.sec);
                        timefmt::epoch_of(ctx, wall, None) * 1_000_000_000
                    };
                    let _ = ctx.set_times(&target, Some(stamp), Some(stamp), true).await;
                }
                if quiet == 0 {
                    let verb = match (kind == 0o120000, m.method) {
                        (true, _) => "    linking",
                        (false, 8) => "  inflating",
                        _ => " extracting",
                    };
                    let mut line = format!("{verb}: {target_text:<22}  ");
                    if kind == 0o120000 {
                        line = format!(
                            "{verb}: {target_text:<22}  -> {} ",
                            String::from_utf8_lossy(&content)
                        );
                    }
                    line.push('\n');
                    let _ = ctx.write(line.as_bytes()).await;
                }
            }
            Err(e) => {
                ctx.error_raw(
                    format!(
                        "error:  cannot create {target_text}\n        {}\n",
                        error_text(&e)
                    )
                    .as_bytes(),
                )
                .await;
                errors += 1;
            }
        }
    }
    for pattern in patterns {
        if !members.iter().any(|m| wildmatch(pattern, &m.name, false)) {
            ctx.error_raw(
                format!(
                    "caution: filename not matched:  {}\n",
                    String::from_utf8_lossy(pattern)
                )
                .as_bytes(),
            )
            .await;
            status = 11;
        }
    }
    if test && quiet < 2 {
        let line = if errors == 0 {
            format!(
                "No errors detected in compressed data of {}.\n",
                String::from_utf8_lossy(&path)
            )
        } else {
            format!(
                "At least one error was detected in {}.\n",
                String::from_utf8_lossy(&path)
            )
        };
        let _ = ctx.write(line.as_bytes()).await;
    }
    if errors > 0 {
        return 2;
    }
    status
}
