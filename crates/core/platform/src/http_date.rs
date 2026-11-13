use core::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const IMF_FIXDATE_BYTES: usize = 29;
const RFC850_BYTES_MIN: usize = 23;
const RFC850_AFTER_WEEKDAY_BYTES: usize = 22;
const ASCTIME_BYTES: usize = 24;
const YEAR_MIN: u16 = 1970;
const YEAR_MAX: u16 = 9999;
const RFC850_CENTURY_PIVOT_YEARS: u16 = 70;
const RFC850_EARLY_CENTURY_YEARS: u16 = 2000;
const RFC850_LATE_CENTURY_YEARS: u16 = 1900;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
const DAYS_PER_YEAR: u64 = 365;
const MONTHS_PER_YEAR: u8 = 12;
const DAYS_PER_WEEK: u64 = 7;
const EPOCH_WEEKDAY: u64 = 3;
const FEBRUARY: u8 = 2;
const MONTH_DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAYS_BEFORE_MONTH: [u64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const WEEKDAYS: [&[u8; 3]; 7] = [b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat", b"Sun"];
const WEEKDAY_NAMES: [&[u8]; 7] = [
    b"Monday, ",
    b"Tuesday, ",
    b"Wednesday, ",
    b"Thursday, ",
    b"Friday, ",
    b"Saturday, ",
    b"Sunday, ",
];
const MONTHS: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpDateError;

impl fmt::Display for HttpDateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("string contains no or an invalid date")
    }
}

impl std::error::Error for HttpDateError {}

struct Fields {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u16,
    weekday: u8,
}

pub fn parse_http_date(input: &str) -> Result<SystemTime, HttpDateError> {
    if !input.is_ascii() {
        return Err(HttpDateError);
    }
    let bytes = input.trim().as_bytes();
    imf_fixdate(bytes)
        .or_else(|| rfc850(bytes))
        .or_else(|| asctime(bytes))
        .and_then(|fields| fields.system_time())
        .ok_or(HttpDateError)
}

fn imf_fixdate(bytes: &[u8]) -> Option<Fields> {
    let text: &[u8; IMF_FIXDATE_BYTES] = bytes.try_into().ok()?;
    if &text[25..] != b" GMT" || text[16] != b' ' || text[19] != b':' || text[22] != b':' {
        return None;
    }
    if &text[3..5] != b", " || text[7] != b' ' || text[11] != b' ' {
        return None;
    }
    Some(Fields {
        second: two_digits(&text[23..25])?,
        minute: two_digits(&text[20..22])?,
        hour: two_digits(&text[17..19])?,
        day: two_digits(&text[5..7])?,
        month: month(&text[8..11])?,
        year: four_digits(&text[12..16])?,
        weekday: weekday(&text[..3])?,
    })
}

fn rfc850(bytes: &[u8]) -> Option<Fields> {
    if bytes.len() < RFC850_BYTES_MIN {
        return None;
    }
    let (weekday, rest) = WEEKDAY_NAMES
        .iter()
        .zip(0u8..)
        .find_map(|(name, index)| bytes.strip_prefix(*name).map(|rest| (index, rest)))?;
    if rest.len() != RFC850_AFTER_WEEKDAY_BYTES
        || rest[12] != b':'
        || rest[15] != b':'
        || &rest[18..22] != b" GMT"
    {
        return None;
    }
    if rest[2] != b'-' || rest[6] != b'-' {
        return None;
    }
    let short_year = u16::from(two_digits(&rest[7..9])?);
    let century = if short_year < RFC850_CENTURY_PIVOT_YEARS {
        RFC850_EARLY_CENTURY_YEARS
    } else {
        RFC850_LATE_CENTURY_YEARS
    };
    Some(Fields {
        second: two_digits(&rest[16..18])?,
        minute: two_digits(&rest[13..15])?,
        hour: two_digits(&rest[10..12])?,
        day: two_digits(&rest[0..2])?,
        month: month(&rest[3..6])?,
        year: century + short_year,
        weekday,
    })
}

fn asctime(bytes: &[u8]) -> Option<Fields> {
    let text: &[u8; ASCTIME_BYTES] = bytes.try_into().ok()?;
    if text[10] != b' ' || text[13] != b':' || text[16] != b':' || text[19] != b' ' {
        return None;
    }
    if text[3] != b' ' || text[7] != b' ' {
        return None;
    }
    let day = if text[8] == b' ' {
        digit(text[9])?
    } else {
        two_digits(&text[8..10])?
    };
    Some(Fields {
        second: two_digits(&text[17..19])?,
        minute: two_digits(&text[14..16])?,
        hour: two_digits(&text[11..13])?,
        day,
        month: month(&text[4..7])?,
        year: four_digits(&text[20..24])?,
        weekday: weekday(&text[..3])?,
    })
}

fn weekday(name: &[u8]) -> Option<u8> {
    WEEKDAYS
        .iter()
        .zip(0u8..)
        .find_map(|(candidate, index)| (candidate.as_slice() == name).then_some(index))
}

fn month(name: &[u8]) -> Option<u8> {
    MONTHS
        .iter()
        .zip(1u8..)
        .find_map(|(candidate, number)| (candidate.as_slice() == name).then_some(number))
}

fn digit(byte: u8) -> Option<u8> {
    byte.is_ascii_digit().then(|| byte - b'0')
}

fn two_digits(bytes: &[u8]) -> Option<u8> {
    Some(digit(bytes[0])? * 10 + digit(bytes[1])?)
}

fn four_digits(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0u16, |value, &byte| {
        Some(value * 10 + u16::from(digit(byte)?))
    })
}

const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn leap_years_before(year: u16) -> u64 {
    let previous = year as u64 - 1;
    previous / 4 - previous / 100 + previous / 400
}

impl Fields {
    fn system_time(&self) -> Option<SystemTime> {
        if u64::from(self.second) >= SECONDS_PER_MINUTE
            || u64::from(self.minute) >= MINUTES_PER_HOUR
            || u64::from(self.hour) >= HOURS_PER_DAY
            || !(YEAR_MIN..=YEAR_MAX).contains(&self.year)
        {
            return None;
        }
        assert!((1..=MONTHS_PER_YEAR).contains(&self.month));
        let month_index = usize::from(self.month - 1);
        let leap_day = u8::from(is_leap_year(self.year) && self.month == FEBRUARY);
        if self.day == 0 || self.day > MONTH_DAYS[month_index] + leap_day {
            return None;
        }
        let later_leap_day = u64::from(is_leap_year(self.year) && self.month > FEBRUARY);
        let days = u64::from(self.year - YEAR_MIN) * DAYS_PER_YEAR + leap_years_before(self.year)
            - leap_years_before(YEAR_MIN)
            + DAYS_BEFORE_MONTH[month_index]
            + later_leap_day
            + u64::from(self.day)
            - 1;
        if (days + EPOCH_WEEKDAY) % DAYS_PER_WEEK != u64::from(self.weekday) {
            return None;
        }
        let seconds = ((days * HOURS_PER_DAY + u64::from(self.hour)) * MINUTES_PER_HOUR
            + u64::from(self.minute))
            * SECONDS_PER_MINUTE
            + u64::from(self.second);
        Some(UNIX_EPOCH + Duration::from_secs(seconds))
    }
}

pub fn format_http_date(time: SystemTime) -> Option<String> {
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let seconds_per_day = SECONDS_PER_MINUTE * MINUTES_PER_HOUR * HOURS_PER_DAY;
    let mut days = seconds / seconds_per_day;
    let second_of_day = seconds % seconds_per_day;
    let weekday = WEEKDAYS[usize::try_from((days + EPOCH_WEEKDAY) % DAYS_PER_WEEK).ok()?];
    let mut year = YEAR_MIN;
    loop {
        let year_days = DAYS_PER_YEAR + u64::from(is_leap_year(year));
        if days < year_days {
            break;
        }
        days -= year_days;
        year += 1;
        if year > YEAR_MAX {
            return None;
        }
    }
    let mut month = 0;
    loop {
        let leap_day = u64::from(is_leap_year(year) && month + 1 == usize::from(FEBRUARY));
        let month_days = u64::from(MONTH_DAYS[month]) + leap_day;
        if days < month_days {
            break;
        }
        days -= month_days;
        month += 1;
    }
    let hour = second_of_day / (SECONDS_PER_MINUTE * MINUTES_PER_HOUR);
    let minute = second_of_day / SECONDS_PER_MINUTE % MINUTES_PER_HOUR;
    let second = second_of_day % SECONDS_PER_MINUTE;
    Some(format!(
        "{}, {:02} {} {year} {hour:02}:{minute:02}:{second:02} GMT",
        std::str::from_utf8(weekday).ok()?,
        days + 1,
        std::str::from_utf8(MONTHS[month]).ok()?,
    ))
}
