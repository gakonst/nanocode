use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAGIC: &[u8] = b"TZif";
const HEADER_RESERVED_BYTES: usize = 15;
const COUNT_BYTES: usize = 4;
const V1_TIME_BYTES: usize = 4;
const V2_TIME_BYTES: usize = 8;
const LOCAL_TYPE_BYTES: usize = 6;
const LEAP_CORRECTION_BYTES: usize = 4;
const VERSION_1: u8 = 0;
const VERSION_3: u8 = b'3';
const VERSIONS: [u8; 3] = [VERSION_1, b'2', VERSION_3];
const NAME_BYTES_MIN: usize = 3;
const NAME_BYTES_MAX: usize = 7;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_DAY: i64 = 86_400;
const DAYS_PER_WEEK: i64 = 7;
const SECONDS_PER_WEEK: i64 = SECONDS_PER_DAY * DAYS_PER_WEEK;
const LEAP_SECOND_INTERVAL_MIN: i64 = 28 * SECONDS_PER_DAY - 1;
const OFFSET_HOURS_MAX: i64 = 23;
const RULE_HOURS_MAX: i64 = 24;
const RULE_EXTENDED_HOURS_MAX: i64 = 167;
const MINUTES_MAX: i64 = 59;
const RULE_TIME_DEFAULT: i64 = 2 * SECONDS_PER_HOUR;
const DST_SHIFT_DEFAULT: i64 = SECONDS_PER_HOUR;
const JULIAN_DAYS_MAX: i64 = 365;
const MONTHS: i64 = 12;
const WEEKS_MAX: i64 = 5;
const WEEKDAY_MAX: i64 = 6;
const EPOCH_WEEKDAY: i64 = 4;
const LEAP_DAY_JULIAN: i64 = 60;
const DAYS_IN_MONTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

#[derive(Clone, Copy, PartialEq, Eq)]
struct LocalType {
    offset: i32,
    dst: bool,
    name: Option<([u8; NAME_BYTES_MAX], usize)>,
}

impl LocalType {
    fn new(offset: i64, dst: bool, name: Option<&[u8]>) -> Option<Self> {
        let offset = i32::try_from(offset)
            .ok()
            .filter(|offset| *offset != i32::MIN)?;
        let name = match name {
            None => None,
            Some(name) => {
                if !(NAME_BYTES_MIN..=NAME_BYTES_MAX).contains(&name.len())
                    || !name
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'+' || *byte == b'-')
                {
                    return None;
                }
                let mut bytes = [0; NAME_BYTES_MAX];
                bytes[..name.len()].copy_from_slice(name);
                Some((bytes, name.len()))
            }
        };
        Some(Self { offset, dst, name })
    }
}

#[derive(Clone, Copy)]
enum RuleDay {
    Julian1(i64),
    Julian0(i64),
    MonthWeekday { month: i64, week: i64, weekday: i64 },
}

#[derive(Clone, Copy)]
enum Rule {
    Fixed(LocalType),
    Alternate {
        std: LocalType,
        dst: LocalType,
        start: RuleDay,
        start_time: i64,
        end: RuleDay,
        end_time: i64,
    },
}

pub(crate) struct Zone {
    transitions: Vec<(i64, usize)>,
    types: Vec<LocalType>,
    leaps: Vec<(i64, i64)>,
    rule: Option<Rule>,
}

impl Zone {
    pub(crate) fn utc() -> Self {
        Self {
            transitions: Vec::new(),
            types: vec![LocalType {
                offset: 0,
                dst: false,
                name: None,
            }],
            leaps: Vec::new(),
            rule: None,
        }
    }

    pub(crate) fn offset(&self, unix: i64) -> Option<i32> {
        let Some(last) = self.transitions.last() else {
            return Some(
                self.rule
                    .map_or(Some(self.types[0]), |rule| rule.find(unix))?
                    .offset,
            );
        };
        let leap = self.unix_to_leap(unix)?;
        if leap >= last.0 {
            return Some(match self.rule {
                Some(rule) => rule.find(unix)?.offset,
                None => self.types[last.1].offset,
            });
        }
        let index = match self
            .transitions
            .binary_search_by_key(&leap, |transition| transition.0)
        {
            Ok(index) => index + 1,
            Err(index) => index,
        };
        let kind = if index > 0 {
            self.transitions[index - 1].1
        } else {
            0
        };
        Some(self.types[kind].offset)
    }

    fn unix_to_leap(&self, unix: i64) -> Option<i64> {
        let mut leap = unix;
        for (time, correction) in &self.leaps {
            if leap < *time {
                break;
            }
            leap = unix.checked_add(*correction)?;
        }
        Some(leap)
    }

    fn leap_to_unix(&self, leap: i64) -> Option<i64> {
        if leap == i64::MIN {
            return None;
        }
        let index = match self
            .leaps
            .binary_search_by_key(&(leap - 1), |entry| entry.0)
        {
            Ok(index) => index + 1,
            Err(index) => index,
        };
        let correction = if index > 0 {
            self.leaps[index - 1].1
        } else {
            0
        };
        leap.checked_sub(correction)
    }

    fn checked(self) -> Option<Self> {
        if self.types.is_empty() {
            return None;
        }
        for (index, transition) in self.transitions.iter().enumerate() {
            if transition.1 >= self.types.len() {
                return None;
            }
            if self
                .transitions
                .get(index + 1)
                .is_some_and(|next| transition.0 >= next.0)
            {
                return None;
            }
        }
        if let Some(first) = self.leaps.first()
            && !(first.0 >= 0 && first.1.saturating_abs() == 1)
        {
            return None;
        }
        for pair in self.leaps.windows(2) {
            if pair[1].0.saturating_sub(pair[0].0) < LEAP_SECOND_INTERVAL_MIN
                || pair[1].1.saturating_sub(pair[0].1).saturating_abs() != 1
            {
                return None;
            }
        }
        if let (Some(rule), Some(last)) = (self.rule, self.transitions.last()) {
            let kind = self.types[last.1];
            let found = rule.find(self.leap_to_unix(last.0)?)?;
            if kind != found {
                return None;
            }
        }
        Some(self)
    }
}

impl Rule {
    fn find(&self, unix: i64) -> Option<LocalType> {
        let Self::Alternate {
            std,
            dst,
            start,
            start_time,
            end,
            end_time,
        } = *self
        else {
            let Self::Fixed(kind) = *self else {
                unreachable!()
            };
            return Some(kind);
        };
        let start_utc = start_time - i64::from(std.offset);
        let end_utc = end_time - i64::from(dst.offset);
        let year = super::civil_from_days(unix.div_euclid(SECONDS_PER_DAY)).0;
        if !(i64::from(i32::MIN) + 2..=i64::from(i32::MAX) - 2).contains(&year) {
            return None;
        }
        let starts = |year: i64| start.unix(year, start_utc);
        let ends = |year: i64| end.unix(year, end_utc);
        let dst_now = if starts(year) <= ends(year) {
            if unix < starts(year) {
                unix < ends(year - 1) && starts(year - 1) <= unix
            } else if unix < ends(year) {
                true
            } else {
                starts(year + 1) <= unix && unix < ends(year + 1)
            }
        } else if unix < ends(year) {
            unix >= starts(year - 1) || unix < ends(year - 1)
        } else if unix < starts(year) {
            false
        } else {
            !(ends(year + 1) <= unix) || starts(year + 1) <= unix
        };
        Some(if dst_now { dst } else { std })
    }
}

const fn leap_year(year: i64) -> bool {
    year % 400 == 0 || (year % 4 == 0 && year % 100 != 0)
}

impl RuleDay {
    fn unix(self, year: i64, time: i64) -> i64 {
        let first = super::days_from_civil(year, 1, 1);
        let day = match self {
            Self::Julian1(day) => {
                first + day - 1 + i64::from(leap_year(year) && day >= LEAP_DAY_JULIAN)
            }
            Self::Julian0(day) => first + day,
            Self::MonthWeekday {
                month,
                week,
                weekday,
            } => {
                let month_first = super::days_from_civil(year, month, 1);
                let mut length = DAYS_IN_MONTHS[(month - 1) as usize];
                if month == 2 && leap_year(year) {
                    length += 1;
                }
                let first_weekday = (EPOCH_WEEKDAY + month_first).rem_euclid(DAYS_PER_WEEK);
                let mut day = 1
                    + (weekday - first_weekday).rem_euclid(DAYS_PER_WEEK)
                    + (week - 1) * DAYS_PER_WEEK;
                if day > length {
                    day -= DAYS_PER_WEEK;
                }
                month_first + day - 1
            }
        };
        day * SECONDS_PER_DAY + time
    }
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    const fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if count > self.0.len() {
            return None;
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Some(head)
    }

    fn count(&mut self) -> Option<usize> {
        let bytes = self.take(COUNT_BYTES)?;
        usize::try_from(u32::from_be_bytes(bytes.try_into().ok()?)).ok()
    }

    const fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn tag(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.0 = &self.0[1..];
        }
        found
    }

    fn take_while(&mut self, keep: impl Fn(u8) -> bool) -> &'a [u8] {
        let end = self
            .0
            .iter()
            .position(|byte| !keep(*byte))
            .unwrap_or(self.0.len());
        let (head, tail) = self.0.split_at(end);
        self.0 = tail;
        head
    }

    fn number(&mut self) -> Option<i64> {
        let digits = self.take_while(|byte| byte.is_ascii_digit());
        if digits.is_empty() {
            return None;
        }
        digits.iter().try_fold(0_i64, |value, digit| {
            value.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
        })
    }
}

struct Block<'a> {
    version: u8,
    times: &'a [u8],
    kinds: &'a [u8],
    types: &'a [u8],
    names: &'a [u8],
    leaps: &'a [u8],
    standard: &'a [u8],
    universal: &'a [u8],
    time_bytes: usize,
}

impl<'a> Block<'a> {
    fn read(cursor: &mut Cursor<'a>, time_bytes: usize) -> Option<Self> {
        if cursor.take(MAGIC.len())? != MAGIC {
            return None;
        }
        let version = cursor.take(1)?[0];
        if !VERSIONS.contains(&version) {
            return None;
        }
        cursor.take(HEADER_RESERVED_BYTES)?;
        let universal = cursor.count()?;
        let standard = cursor.count()?;
        let leaps = cursor.count()?;
        let transitions = cursor.count()?;
        let types = cursor.count()?;
        let names = cursor.count()?;
        if types == 0
            || names == 0
            || (universal != 0 && universal != types)
            || (standard != 0 && standard != types)
        {
            return None;
        }
        Some(Self {
            version,
            times: cursor.take(transitions.checked_mul(time_bytes)?)?,
            kinds: cursor.take(transitions)?,
            types: cursor.take(types.checked_mul(LOCAL_TYPE_BYTES)?)?,
            names: cursor.take(names)?,
            leaps: cursor.take(leaps.checked_mul(time_bytes + LEAP_CORRECTION_BYTES)?)?,
            standard: cursor.take(standard)?,
            universal: cursor.take(universal)?,
            time_bytes,
        })
    }

    fn time(&self, bytes: &[u8]) -> Option<i64> {
        if self.version == VERSION_1 {
            return Some(i64::from(i32::from_be_bytes(
                bytes.get(..V1_TIME_BYTES)?.try_into().ok()?,
            )));
        }
        Some(i64::from_be_bytes(bytes.try_into().ok()?))
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Option<Zone> {
    let mut cursor = Cursor(bytes);
    let first = Block::read(&mut cursor, V1_TIME_BYTES)?;
    let (block, footer) = if first.version == VERSION_1 {
        if !cursor.0.is_empty() {
            return None;
        }
        (first, None)
    } else {
        (Block::read(&mut cursor, V2_TIME_BYTES)?, Some(cursor.0))
    };
    let mut transitions = Vec::with_capacity(block.kinds.len());
    for (time, kind) in block.times.chunks_exact(block.time_bytes).zip(block.kinds) {
        transitions.push((block.time(time)?, usize::from(*kind)));
    }
    let mut types = Vec::with_capacity(block.types.len() / LOCAL_TYPE_BYTES);
    for entry in block.types.as_chunks::<LOCAL_TYPE_BYTES>().0 {
        let offset = i32::from_be_bytes(entry[..COUNT_BYTES].try_into().ok()?);
        let dst = match entry[COUNT_BYTES] {
            0 => false,
            1 => true,
            _ => return None,
        };
        let start = usize::from(entry[COUNT_BYTES + 1]);
        let rest = block.names.get(start..)?;
        let name = &rest[..rest.iter().position(|byte| *byte == 0)?];
        types.push(LocalType::new(
            i64::from(offset),
            dst,
            (!name.is_empty()).then_some(name),
        )?);
    }
    let mut leaps =
        Vec::with_capacity(block.leaps.len() / (block.time_bytes + LEAP_CORRECTION_BYTES));
    for entry in block
        .leaps
        .chunks_exact(block.time_bytes + LEAP_CORRECTION_BYTES)
    {
        let correction = i32::from_be_bytes(entry[block.time_bytes..].try_into().ok()?);
        leaps.push((
            block.time(&entry[..block.time_bytes])?,
            i64::from(correction),
        ));
    }
    let wall = |index: usize| block.standard.get(index).copied().unwrap_or(0);
    let local = |index: usize| block.universal.get(index).copied().unwrap_or(0);
    if (0..types.len()).any(|index| wall(index) == 0 && local(index) == 1) {
        return None;
    }
    let rule = match footer {
        None => None,
        Some(footer) => {
            let footer = std::str::from_utf8(footer).ok()?;
            if !(footer.starts_with('\n') && footer.ends_with('\n')) {
                return None;
            }
            let rule = footer.trim_matches(|character: char| character.is_ascii_whitespace());
            if rule.starts_with(':') || rule.contains('\0') {
                return None;
            }
            if rule.is_empty() {
                None
            } else {
                Some(parse_rule(rule.as_bytes(), block.version == VERSION_3)?)
            }
        }
    };
    Zone {
        transitions,
        types,
        leaps,
        rule,
    }
    .checked()
}

fn read_file(path: &Path) -> Option<Zone> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    parse(&bytes)
}

fn find_file(path: &str, directories: &[&str]) -> Option<File> {
    let path = Path::new(path);
    if path.is_absolute() {
        return File::open(path).ok();
    }
    directories
        .iter()
        .find_map(|directory| File::open(Path::new(directory).join(path)).ok())
}

fn read_found(file: Option<File>) -> Option<Option<Zone>> {
    let mut bytes = Vec::new();
    file?.read_to_end(&mut bytes).ok().map(|_| parse(&bytes))
}

pub(crate) fn from_environment(value: &str, localtime: &str, directories: &[&str]) -> Option<Zone> {
    if value.is_empty() {
        return Some(Zone::utc());
    }
    if value == "localtime" {
        return read_file(Path::new(localtime));
    }
    if let Some(name) = value.strip_prefix(':') {
        return read_found(find_file(name, directories)).flatten();
    }
    if let Some(found) = find_file(value, directories) {
        return read_found(Some(found)).flatten();
    }
    let rule = parse_rule(
        value
            .trim_matches(|character: char| character.is_ascii_whitespace())
            .as_bytes(),
        true,
    )?;
    let types = match rule {
        Rule::Fixed(kind) => vec![kind],
        Rule::Alternate { std, dst, .. } => vec![std, dst],
    };
    Zone {
        transitions: Vec::new(),
        types,
        leaps: Vec::new(),
        rule: Some(rule),
    }
    .checked()
}

pub(crate) fn from_path(path: &str) -> Option<Zone> {
    read_file(Path::new(path))
}

fn parse_name<'a>(cursor: &mut Cursor<'a>) -> Option<&'a [u8]> {
    if !cursor.tag(b'<') {
        return Some(cursor.take_while(|byte| byte.is_ascii_alphabetic()));
    }
    let name = cursor.take_while(|byte| byte != b'>');
    cursor.tag(b'>').then_some(name)
}

fn parse_clock(cursor: &mut Cursor) -> Option<(i64, i64, i64)> {
    let hours = cursor.number()?;
    let mut minutes = 0;
    let mut seconds = 0;
    if cursor.tag(b':') {
        minutes = cursor.number()?;
        if cursor.tag(b':') {
            seconds = cursor.number()?;
        }
    }
    Some((hours, minutes, seconds))
}

fn parse_signed(cursor: &mut Cursor, hours_max: i64) -> Option<i64> {
    let sign = if cursor.tag(b'-') {
        -1
    } else {
        cursor.tag(b'+');
        1
    };
    let (hours, minutes, seconds) = parse_clock(cursor)?;
    if hours > hours_max || minutes > MINUTES_MAX || seconds > MINUTES_MAX {
        return None;
    }
    Some(sign * (hours * SECONDS_PER_HOUR + minutes * SECONDS_PER_MINUTE + seconds))
}

fn parse_day(cursor: &mut Cursor, extended: bool) -> Option<(RuleDay, i64)> {
    let day = if cursor.tag(b'M') {
        let month = cursor.number()?;
        if !cursor.tag(b'.') {
            return None;
        }
        let week = cursor.number()?;
        if !cursor.tag(b'.') {
            return None;
        }
        let weekday = cursor.number()?;
        if !(1..=MONTHS).contains(&month)
            || !(1..=WEEKS_MAX).contains(&week)
            || weekday > WEEKDAY_MAX
        {
            return None;
        }
        RuleDay::MonthWeekday {
            month,
            week,
            weekday,
        }
    } else if cursor.tag(b'J') {
        let day = cursor.number()?;
        if !(1..=JULIAN_DAYS_MAX).contains(&day) {
            return None;
        }
        RuleDay::Julian1(day)
    } else {
        let day = cursor.number()?;
        if day > JULIAN_DAYS_MAX {
            return None;
        }
        RuleDay::Julian0(day)
    };
    let time = if !cursor.tag(b'/') {
        RULE_TIME_DEFAULT
    } else if extended {
        parse_signed(cursor, RULE_EXTENDED_HOURS_MAX)?
    } else {
        if cursor.peek() == Some(b'-') || cursor.peek() == Some(b'+') {
            return None;
        }
        parse_signed(cursor, RULE_HOURS_MAX)?
    };
    Some((day, time))
}

fn parse_rule(text: &[u8], extended: bool) -> Option<Rule> {
    let mut cursor = Cursor(text);
    let std_name = parse_name(&mut cursor)?;
    let std_offset = parse_signed(&mut cursor, OFFSET_HOURS_MAX)?;
    let std = LocalType::new(-std_offset, false, Some(std_name))?;
    if cursor.0.is_empty() {
        return Some(Rule::Fixed(std));
    }
    let dst_name = parse_name(&mut cursor)?;
    let dst_offset = match cursor.peek()? {
        b',' => std_offset - DST_SHIFT_DEFAULT,
        _ => parse_signed(&mut cursor, OFFSET_HOURS_MAX)?,
    };
    if !cursor.tag(b',') {
        return None;
    }
    let (start, start_time) = parse_day(&mut cursor, extended)?;
    if !cursor.tag(b',') {
        return None;
    }
    let (end, end_time) = parse_day(&mut cursor, extended)?;
    if !cursor.0.is_empty() {
        return None;
    }
    let dst = LocalType::new(-dst_offset, true, Some(dst_name))?;
    if start_time.abs() >= SECONDS_PER_WEEK || end_time.abs() >= SECONDS_PER_WEEK {
        return None;
    }
    Some(Rule::Alternate {
        std,
        dst,
        start,
        start_time,
        end,
        end_time,
    })
}
