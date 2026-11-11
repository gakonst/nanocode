//! How a commit is printed (git-log(1), PRETTY FORMATS): the built-in formats, format
//! strings, dates and decorations.

use std::collections::HashMap;

use super::repo::{Commit, Ident, Repo};
use super::store::Oid;
use super::{Error, Res, option};
use crate::timefmt::{strftime, tm_at};

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, PartialEq, Eq)]
pub enum DateMode {
    Default,
    Iso,
    IsoStrict,
    Short,
    Rfc,
    Relative,
    Unix,
    Raw,
    Format(String),
}

impl DateMode {
    pub fn parse(s: &str) -> Res<DateMode> {
        Ok(match s {
            "default" => DateMode::Default,
            "iso" | "iso8601" => DateMode::Iso,
            "iso-strict" | "iso8601-strict" => DateMode::IsoStrict,
            "short" => DateMode::Short,
            "rfc" | "rfc2822" => DateMode::Rfc,
            "relative" => DateMode::Relative,
            "unix" => DateMode::Unix,
            "raw" => DateMode::Raw,
            _ => match s.strip_prefix("format:") {
                Some(format) => DateMode::Format(format.to_string()),
                None => return Err(option("--date", s)),
            },
        })
    }
}

fn offset_secs(zone: i32) -> i32 {
    (zone / 100) * 3600 + (zone % 100) * 60
}

fn zone_text(zone: i32) -> String {
    format!("{}{:04}", if zone < 0 { '-' } else { '+' }, zone.abs())
}

pub fn date_default(time: i64, zone: i32) -> String {
    let tm = tm_at(time * 1_000_000_000, offset_secs(zone));
    format!(
        "{} {} {} {:02}:{:02}:{:02} {} {}",
        DAYS[tm.wday as usize],
        MONTHS[tm.mon as usize - 1],
        tm.day,
        tm.hour,
        tm.min,
        tm.sec,
        tm.year,
        zone_text(zone)
    )
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("{n} {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

pub fn date_relative(time: i64, now: i64) -> String {
    if now < time {
        return "in the future".to_string();
    }
    let mut diff = now - time;
    if diff < 90 {
        return format!("{} ago", plural(diff, "second"));
    }
    diff = (diff + 30) / 60;
    if diff < 90 {
        return format!("{} ago", plural(diff, "minute"));
    }
    diff = (diff + 30) / 60;
    if diff < 36 {
        return format!("{} ago", plural(diff, "hour"));
    }
    diff = (diff + 12) / 24;
    if diff < 14 {
        return format!("{} ago", plural(diff, "day"));
    }
    if diff < 70 {
        return format!("{} ago", plural((diff + 3) / 7, "week"));
    }
    if diff < 365 {
        return format!("{} ago", plural((diff + 15) / 30, "month"));
    }
    if diff < 1825 {
        let total = (diff * 12 * 2 + 365) / (365 * 2);
        let (years, months) = (total / 12, total % 12);
        if months != 0 {
            return format!("{}, {} ago", plural(years, "year"), plural(months, "month"));
        }
        return format!("{} ago", plural(years, "year"));
    }
    format!("{} ago", plural((diff + 183) / 365, "year"))
}

pub fn date(mode: &DateMode, time: i64, zone: i32, now: i64) -> String {
    let tm = tm_at(time * 1_000_000_000, offset_secs(zone));
    match mode {
        DateMode::Default => date_default(time, zone),
        DateMode::Iso => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
            tm.year,
            tm.mon,
            tm.day,
            tm.hour,
            tm.min,
            tm.sec,
            zone_text(zone)
        ),
        DateMode::IsoStrict => {
            let z = if zone == 0 {
                "Z".to_string()
            } else {
                let text = zone_text(zone);
                format!("{}:{}", &text[..3], &text[3..])
            };
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{z}",
                tm.year, tm.mon, tm.day, tm.hour, tm.min, tm.sec
            )
        }
        DateMode::Short => format!("{:04}-{:02}-{:02}", tm.year, tm.mon, tm.day),
        DateMode::Rfc => format!(
            "{}, {} {} {} {:02}:{:02}:{:02} {}",
            DAYS[tm.wday as usize],
            tm.day,
            MONTHS[tm.mon as usize - 1],
            tm.year,
            tm.hour,
            tm.min,
            tm.sec,
            zone_text(zone)
        ),
        DateMode::Relative => date_relative(time, now),
        DateMode::Unix => time.to_string(),
        DateMode::Raw => format!("{time} {}", zone_text(zone)),
        DateMode::Format(format) => {
            let format = format.replace("%z", &zone_text(zone));
            String::from_utf8_lossy(&strftime(format.as_bytes(), &tm)).into_owned()
        }
    }
}

fn blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

/// Where the subject's paragraph starts and ends, and where the body starts.
fn parts(message: &[u8]) -> (usize, usize, usize) {
    let mut pos = 0;
    let mut start = None;
    let mut end = message.len();
    let mut body = message.len();
    while pos < message.len() {
        let stop = message[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(message.len(), |n| pos + n);
        let line = &message[pos..stop];
        let next = (stop + 1).min(message.len());
        match start {
            None if !blank(line) => start = Some(pos),
            None => {}
            Some(_) if blank(line) => {
                end = pos;
                body = next;
                break;
            }
            Some(_) => {}
        }
        pos = next;
    }
    let start = start.unwrap_or(message.len());
    // The body starts after the blank lines that follow the subject.
    let mut at = body;
    while at < message.len() {
        let stop = message[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(message.len(), |n| at + n);
        if !blank(&message[at..stop]) {
            break;
        }
        at = (stop + 1).min(message.len());
    }
    (start, end.max(start), at)
}

/// The subject: the first paragraph on one line.
pub fn subject(message: &[u8]) -> Vec<u8> {
    let (start, end, _) = parts(message);
    let mut out = Vec::new();
    for line in message[start..end].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(b' ');
        }
        out.extend_from_slice(line);
    }
    while out.last().is_some_and(u8::is_ascii_whitespace) {
        out.pop();
    }
    out
}

fn body(message: &[u8]) -> &[u8] {
    let (_, _, at) = parts(message);
    &message[at..]
}

/// The names that point at each commit, as git log decorates with them.
pub struct Decorations {
    names: HashMap<Oid, Vec<String>>,
}

impl Decorations {
    pub fn load(repo: &Repo<'_>) -> Res<Decorations> {
        let mut names: HashMap<Oid, Vec<String>> = HashMap::new();
        let head_ref = repo.head_ref()?;
        for (name, oid) in repo.refs("refs/")? {
            let (label, target) = if let Some(tag) = name.strip_prefix("refs/tags/") {
                let (peeled, _) = repo.peel(&oid)?;
                (format!("tag: {tag}"), peeled)
            } else if let Some(branch) = name.strip_prefix("refs/heads/") {
                if head_ref.as_deref() == Some(name.as_str()) {
                    continue;
                }
                (branch.to_string(), oid)
            } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
                (remote.to_string(), oid)
            } else {
                (name.clone(), oid)
            };
            names.entry(target).or_default().insert(0, label);
        }
        if let Some(head) = repo.head()? {
            let label = match &head_ref {
                Some(branch) => format!(
                    "HEAD -> {}",
                    branch.strip_prefix("refs/heads/").unwrap_or(branch)
                ),
                None => "HEAD".to_string(),
            };
            names.entry(head).or_default().insert(0, label);
        }
        Ok(Decorations { names })
    }

    pub fn of(&self, oid: &Oid) -> String {
        self.names
            .get(oid)
            .map(|n| n.join(", "))
            .unwrap_or_default()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Format {
    Oneline,
    Short,
    Medium,
    Full,
    Fuller,
    Raw,
    Reference,
    /// A format string; the flag says that each commit ends with a newline (tformat)
    /// rather than commits being separated by one (format).
    User(String, bool),
}

impl Format {
    pub fn parse(s: &str) -> Res<Format> {
        Ok(match s {
            "oneline" => Format::Oneline,
            "short" => Format::Short,
            "medium" => Format::Medium,
            "full" => Format::Full,
            "fuller" => Format::Fuller,
            "raw" => Format::Raw,
            "reference" => Format::Reference,
            _ => {
                if let Some(format) = s.strip_prefix("format:") {
                    Format::User(format.to_string(), false)
                } else if let Some(format) = s.strip_prefix("tformat:") {
                    Format::User(format.to_string(), true)
                } else if s.contains('%') || s.is_empty() {
                    Format::User(s.to_string(), true)
                } else {
                    return Err(Error::Fatal(format!("invalid --pretty format: {s}")));
                }
            }
        })
    }
}

pub struct Pretty<'r, 'a> {
    pub repo: &'r Repo<'a>,
    pub format: Format,
    pub date: DateMode,
    pub abbrev_commit: bool,
    pub decorate: Option<Decorations>,
    pub now: i64,
    /// The digits of an abbreviated name, when --abbrev gave them.
    pub abbrev: Option<usize>,
}

impl Pretty<'_, '_> {
    fn short(&self, oid: &Oid) -> Res<String> {
        match self.abbrev {
            Some(len) => self.repo.short_at(oid, len),
            None => self.repo.short(oid),
        }
    }

    fn decoration(&self, oid: &Oid) -> String {
        self.decorate
            .as_ref()
            .map(|d| d.of(oid))
            .unwrap_or_default()
    }

    fn who(
        &self,
        out: &mut Vec<u8>,
        line: &[u8],
        what: u8,
        relative_default: &DateMode,
    ) -> Res<bool> {
        let who = Ident::parse(line);
        match what {
            b'n' | b'N' => out.extend_from_slice(who.name),
            b'e' | b'E' => out.extend_from_slice(who.email),
            b'l' | b'L' => {
                let local = who.email.split(|&b| b == b'@').next().unwrap_or_default();
                out.extend_from_slice(local);
            }
            b'd' => out
                .extend_from_slice(date(relative_default, who.time, who.zone, self.now).as_bytes()),
            b'D' => {
                out.extend_from_slice(date(&DateMode::Rfc, who.time, who.zone, self.now).as_bytes())
            }
            b'r' => out.extend_from_slice(date_relative(who.time, self.now).as_bytes()),
            b't' => out.extend_from_slice(who.time.to_string().as_bytes()),
            b'i' => {
                out.extend_from_slice(date(&DateMode::Iso, who.time, who.zone, self.now).as_bytes())
            }
            b'I' => out.extend_from_slice(
                date(&DateMode::IsoStrict, who.time, who.zone, self.now).as_bytes(),
            ),
            b's' => out
                .extend_from_slice(date(&DateMode::Short, who.time, who.zone, self.now).as_bytes()),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// One placeholder at format[at..] (after the percent sign): its text and how many
    /// bytes of the format it took. None: not a placeholder, printed as it is.
    fn placeholder(&self, commit: &Commit, format: &[u8], out: &mut Vec<u8>) -> Res<Option<usize>> {
        let Some(&c) = format.first() else {
            return Ok(None);
        };
        let hexes = |oids: &[Oid], short: bool| -> Res<Vec<u8>> {
            let mut text = Vec::new();
            for (i, oid) in oids.iter().enumerate() {
                if i > 0 {
                    text.push(b' ');
                }
                if short {
                    text.extend_from_slice(self.short(oid)?.as_bytes());
                } else {
                    text.extend_from_slice(oid.hex().as_bytes());
                }
            }
            Ok(text)
        };
        let used = match c {
            b'H' => {
                out.extend_from_slice(commit.id.hex().as_bytes());
                1
            }
            b'h' => {
                out.extend_from_slice(self.short(&commit.id)?.as_bytes());
                1
            }
            b'T' => {
                out.extend_from_slice(commit.tree.hex().as_bytes());
                1
            }
            b't' => {
                out.extend_from_slice(self.short(&commit.tree)?.as_bytes());
                1
            }
            b'P' => {
                out.extend_from_slice(&hexes(&commit.parents, false)?);
                1
            }
            b'p' => {
                out.extend_from_slice(&hexes(&commit.parents, true)?);
                1
            }
            b'a' | b'c' => {
                let line = if c == b'a' {
                    &commit.author
                } else {
                    &commit.committer
                };
                let Some(&what) = format.get(1) else {
                    return Ok(None);
                };
                if !self.who(out, line, what, &self.date)? {
                    return Ok(None);
                }
                2
            }
            b's' => {
                out.extend_from_slice(&subject(commit.message()));
                1
            }
            b'f' => {
                let subject = subject(commit.message());
                let mut slug = Vec::new();
                for &b in &subject {
                    if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' {
                        slug.push(b);
                    } else if slug.last().is_some_and(|&l| l != b'-') {
                        slug.push(b'-');
                    }
                }
                while slug.last().is_some_and(|&l| l == b'-' || l == b'.') {
                    slug.pop();
                }
                out.extend_from_slice(&slug);
                1
            }
            b'b' => {
                out.extend_from_slice(body(commit.message()));
                1
            }
            b'B' => {
                out.extend_from_slice(commit.message());
                1
            }
            b'd' => {
                let names = self.decoration(&commit.id);
                if !names.is_empty() {
                    out.extend_from_slice(format!(" ({names})").as_bytes());
                }
                1
            }
            b'D' => {
                out.extend_from_slice(self.decoration(&commit.id).as_bytes());
                1
            }
            b'n' => {
                out.push(b'\n');
                1
            }
            b'%' => {
                out.push(b'%');
                1
            }
            b'x' => {
                let hex = format.get(1..3).and_then(|h| std::str::from_utf8(h).ok());
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        3
                    }
                    None => return Ok(None),
                }
            }
            b'C' => {
                // Colors are off where nothing is a terminal.
                if format.get(1) == Some(&b'(') {
                    match format.iter().position(|&b| b == b')') {
                        Some(close) => close + 1,
                        None => return Ok(None),
                    }
                } else {
                    let rest = &format[1..];
                    let names: [&[u8]; 4] = [b"red", b"green", b"blue", b"reset"];
                    match names.iter().find(|name| rest.starts_with(name)) {
                        Some(name) => 1 + name.len(),
                        None => return Ok(None),
                    }
                }
            }
            b'e' => 1,
            b'G' | b'N' | b'g' | b'w' | b'(' | b'm' | b'S' => {
                return Err(option(
                    "--format",
                    &format!(
                        "%{}",
                        String::from_utf8_lossy(&format[..format.len().min(2)])
                    ),
                ));
            }
            _ => return Ok(None),
        };
        Ok(Some(used))
    }

    fn user(&self, commit: &Commit, format: &str) -> Res<Vec<u8>> {
        let format = format.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        // A pending %<(N) or %>(N): the width, left-aligned or not, and how to cut.
        let mut pad: Option<(usize, bool, Option<u8>)> = None;
        while i < format.len() {
            if format[i] != b'%' {
                out.push(format[i]);
                i += 1;
                continue;
            }
            let rest = &format[i + 1..];
            if (rest.starts_with(b"<(") || rest.starts_with(b">("))
                && let Some(close) = rest.iter().position(|&b| b == b')')
            {
                let inside = String::from_utf8_lossy(&rest[2..close]).into_owned();
                let (width, cut) = match inside.split_once(',') {
                    Some((width, cut)) => (width, Some(cut.trim().to_string())),
                    None => (inside.as_str(), None),
                };
                let cut = match cut.as_deref() {
                    None => None,
                    Some("trunc") => Some(b't'),
                    Some("ltrunc") => Some(b'l'),
                    Some(_) => return Err(option("--format", "%<( , )")),
                };
                let width = width
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| option("--format", "%<(N)"))?;
                pad = Some((width, rest[0] == b'<', cut));
                i += close + 2;
                continue;
            }
            let (magic, body) = match rest.first() {
                Some(m @ (b'+' | b'-' | b' ')) => (Some(*m), &rest[1..]),
                _ => (None, rest),
            };
            let mut piece = Vec::new();
            match self.placeholder(commit, body, &mut piece)? {
                Some(used) => {
                    i += 1 + usize::from(magic.is_some()) + used;
                    match magic {
                        Some(b'+') if !piece.is_empty() => out.push(b'\n'),
                        Some(b' ') if !piece.is_empty() => out.push(b' '),
                        Some(b'-') if piece.is_empty() => {
                            while out.last() == Some(&b'\n') {
                                out.pop();
                            }
                        }
                        _ => {}
                    }
                    if let Some((width, left, cut)) = pad.take() {
                        let text = String::from_utf8_lossy(&piece).into_owned();
                        let len = text.chars().count();
                        let text = if len > width && cut.is_some() && width >= 2 {
                            if cut == Some(b't') {
                                let kept: String = text.chars().take(width - 2).collect();
                                format!("{kept}..")
                            } else {
                                let kept: String = text.chars().skip(len - (width - 2)).collect();
                                format!("..{kept}")
                            }
                        } else {
                            text
                        };
                        let len = text.chars().count();
                        let fill = " ".repeat(width.saturating_sub(len));
                        piece = if left {
                            format!("{text}{fill}").into_bytes()
                        } else {
                            format!("{fill}{text}").into_bytes()
                        };
                    }
                    out.extend_from_slice(&piece);
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    fn name_line(&self, oid: &Oid) -> Res<String> {
        if self.abbrev_commit {
            self.short(oid)
        } else {
            Ok(oid.hex())
        }
    }

    /// A commit as the format prints it, without what separates it from the next.
    pub fn commit(&self, commit: &Commit) -> Res<Vec<u8>> {
        let mut out = Vec::new();
        let decoration = {
            let names = self.decoration(&commit.id);
            if names.is_empty() {
                String::new()
            } else {
                format!(" ({names})")
            }
        };
        match &self.format {
            Format::User(format, _) => return self.user(commit, format),
            Format::Oneline => {
                out.extend_from_slice(self.name_line(&commit.id)?.as_bytes());
                out.extend_from_slice(decoration.as_bytes());
                out.push(b' ');
                out.extend_from_slice(&subject(commit.message()));
                return Ok(out);
            }
            Format::Reference => {
                let who = Ident::parse(&commit.committer);
                out.extend_from_slice(self.short(&commit.id)?.as_bytes());
                out.extend_from_slice(b" (");
                out.extend_from_slice(&subject(commit.message()));
                out.extend_from_slice(
                    format!(
                        ", {})",
                        date(&DateMode::Short, who.time, who.zone, self.now)
                    )
                    .as_bytes(),
                );
                return Ok(out);
            }
            Format::Raw => {
                out.extend_from_slice(
                    format!("commit {}{decoration}\n", commit.id.hex()).as_bytes(),
                );
                out.extend_from_slice(&commit.data[..commit.body]);
            }
            _ => {
                out.extend_from_slice(
                    format!("commit {}{decoration}\n", self.name_line(&commit.id)?).as_bytes(),
                );
                if commit.parents.len() > 1 {
                    out.extend_from_slice(b"Merge:");
                    for parent in &commit.parents {
                        out.push(b' ');
                        out.extend_from_slice(self.short(parent)?.as_bytes());
                    }
                    out.push(b'\n');
                }
                let author = Ident::parse(&commit.author);
                let committer = Ident::parse(&commit.committer);
                let person = |label: &str, who: &Ident<'_>| {
                    let mut line = label.as_bytes().to_vec();
                    line.extend_from_slice(who.name);
                    line.extend_from_slice(b" <");
                    line.extend_from_slice(who.email);
                    line.extend_from_slice(b">\n");
                    line
                };
                match self.format {
                    Format::Short => out.extend_from_slice(&person("Author: ", &author)),
                    Format::Medium => {
                        out.extend_from_slice(&person("Author: ", &author));
                        out.extend_from_slice(
                            format!(
                                "Date:   {}\n",
                                date(&self.date, author.time, author.zone, self.now)
                            )
                            .as_bytes(),
                        );
                    }
                    Format::Full => {
                        out.extend_from_slice(&person("Author: ", &author));
                        out.extend_from_slice(&person("Commit: ", &committer));
                    }
                    _ => {
                        out.extend_from_slice(&person("Author:     ", &author));
                        out.extend_from_slice(
                            format!(
                                "AuthorDate: {}\n",
                                date(&self.date, author.time, author.zone, self.now)
                            )
                            .as_bytes(),
                        );
                        out.extend_from_slice(&person("Commit:     ", &committer));
                        out.extend_from_slice(
                            format!(
                                "CommitDate: {}\n",
                                date(&self.date, committer.time, committer.zone, self.now)
                            )
                            .as_bytes(),
                        );
                    }
                }
                out.push(b'\n');
            }
        }
        let message = commit.message();
        if self.format == Format::Short {
            out.extend_from_slice(b"    ");
            out.extend_from_slice(&subject(message));
            out.push(b'\n');
            return Ok(out);
        }
        let mut lines: Vec<&[u8]> = message.split(|&b| b == b'\n').collect();
        while lines.last().is_some_and(|l| blank(l)) {
            lines.pop();
        }
        let skip = lines.iter().take_while(|l| blank(l)).count();
        for line in &lines[skip..] {
            out.extend_from_slice(b"    ");
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        Ok(out)
    }
}

/// A tag object as git show prints it before what it names.
pub fn tag_header(data: &[u8], mode: &DateMode, now: i64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut lines = data.split(|&b| b == b'\n');
    let mut name: &[u8] = b"";
    let mut tagger: Option<&[u8]> = None;
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix(b"tag ") {
            name = n;
        } else if let Some(t) = line.strip_prefix(b"tagger ") {
            tagger = Some(t);
        }
    }
    out.extend_from_slice(b"tag ");
    out.extend_from_slice(name);
    out.push(b'\n');
    if let Some(tagger) = tagger {
        let who = Ident::parse(tagger);
        out.extend_from_slice(b"Tagger: ");
        out.extend_from_slice(who.name);
        out.extend_from_slice(b" <");
        out.extend_from_slice(who.email);
        out.extend_from_slice(b">\n");
        out.extend_from_slice(
            format!("Date:   {}\n", date(mode, who.time, who.zone, now)).as_bytes(),
        );
    }
    out.push(b'\n');
    let rest: Vec<&[u8]> = lines.collect();
    let mut message = rest.join(&b'\n');
    if let Some(at) = message
        .windows(27)
        .position(|w| w == b"-----BEGIN PGP SIGNATURE---")
    {
        message.truncate(at);
    }
    while message.last() == Some(&b'\n') {
        message.pop();
    }
    out.extend_from_slice(&message);
    out.extend_from_slice(b"\n\n");
    out
}
