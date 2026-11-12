use nanocode_platform::{civil_from_days, days_from_civil};

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_DAY: i64 = 86_400;
const DAYS_PER_WEEK: i64 = 7;
const EPOCH_DAYS_AFTER_SUNDAY: i64 = 4;
const EPOCH_DAYS_AFTER_MONDAY: i64 = 3;
const UNIX_SECONDS_MIN: i64 = -377_705_023_201;
const UNIX_SECONDS_MAX: i64 = 253_402_207_200;
const YEAR_MIN: i16 = -9_999;
const YEAR_MAX: i16 = 9_999;
const MONTHS_PER_YEAR: i8 = 12;
const HOURS_PER_DAY: i8 = 24;
const HOURS_PER_HALF_DAY: i8 = 12;
const MINUTES_PER_HOUR: i8 = 60;
const SECONDS_PER_MINUTE_FIELD: i8 = 60;
const NANOSECONDS_PER_SECOND: i32 = 1_000_000_000;
const ISO_WEEK_ANCHOR_DAY: i64 = 4;
const DECEMBER: i8 = 12;
const ISO_WEEK_YEAR_CHANGE_DAY_MIN: i8 = 29;
const DAYS_PER_FEBRUARY_LEAP: i8 = 29;
const DAYS_PER_MONTH: [i8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const FEBRUARY: i8 = 2;
const YEARS_PER_CENTURY: i16 = 100;
const YEARS_PER_LEAP: i16 = 4;
const YEARS_PER_LEAP_CYCLE: i16 = 400;
const WEEKDAYS_AFTER_SUNDAY: i16 = 6;
const DAYS_PER_WEEK_FIELD: i16 = 7;
const MONTHS_PER_QUARTER: i8 = 3;
const INTEGER_DIGITS_MAX: u8 = 20;
const FRACTION_DIGITS: usize = 9;
const COLONS_MAX: usize = 3;
const DECIMAL_BASE: u64 = 10;
const DECIMAL_BASE_DIGIT: u8 = 10;
const DECIMAL_BASE_FRACTION: u32 = 10;
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const WEEKDAY_ABBREVIATIONS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DATE_TIME_FORMAT: &str = "%Y M%m %-d, %a %H:%M:%S";
const DATE_TIME_UPPERCASE_FORMAT: &str = "%Y M%m %-d, %^a %H:%M:%S";
const DATE_FORMAT: &str = "%Y M%m %-d";
const TIME_FORMAT: &str = "%H:%M:%S";
const TWELVE_HOUR_FORMAT: &str = "%-I:%M:%S %p";
const TWELVE_HOUR_UPPERCASE_FORMAT: &str = "%-I:%M:%S %^p";

#[derive(Debug)]
pub(super) struct OutOfRange;

#[derive(Debug)]
struct Unformattable;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DateTime {
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    second: i8,
    nanosecond: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flag {
    PadSpace,
    PadZero,
    NoPad,
    Uppercase,
    Swapcase,
}

#[derive(Clone, Copy)]
struct Extension {
    flag: Option<Flag>,
    width: Option<u8>,
}

const fn leap_year(year: i16) -> bool {
    year % YEARS_PER_LEAP_CYCLE == 0
        || (year % YEARS_PER_LEAP == 0 && year % YEARS_PER_CENTURY != 0)
}

fn days_in_month(year: i16, month: i8) -> i8 {
    if month == FEBRUARY && leap_year(year) {
        return DAYS_PER_FEBRUARY_LEAP;
    }
    DAYS_PER_MONTH[usize::try_from(month - 1).expect("a month from 1 to 12")]
}

fn iso_week_start(year: i64) -> i64 {
    let anchor = days_from_civil(year, 1, ISO_WEEK_ANCHOR_DAY);
    anchor - (anchor + EPOCH_DAYS_AFTER_MONDAY).rem_euclid(DAYS_PER_WEEK)
}

impl DateTime {
    pub(super) fn new(
        year: i16,
        month: i8,
        day: i8,
        hour: i8,
        minute: i8,
        second: i8,
    ) -> Result<Self, OutOfRange> {
        if !(YEAR_MIN..=YEAR_MAX).contains(&year)
            || !(1..=MONTHS_PER_YEAR).contains(&month)
            || !(1..=days_in_month(year, month)).contains(&day)
            || !(0..HOURS_PER_DAY).contains(&hour)
            || !(0..MINUTES_PER_HOUR).contains(&minute)
            || !(0..SECONDS_PER_MINUTE_FIELD).contains(&second)
        {
            return Err(OutOfRange);
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond: 0,
        })
    }

    pub(super) fn from_unix(seconds: i64, nanosecond: i32) -> Result<Self, OutOfRange> {
        assert!((0..NANOSECONDS_PER_SECOND).contains(&nanosecond));
        if !(UNIX_SECONDS_MIN..=UNIX_SECONDS_MAX).contains(&seconds) {
            return Err(OutOfRange);
        }
        let (year, month, day) = civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
        let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let field = |value: i64| i8::try_from(value).expect("a clock field below 60");
        let date_time = Self {
            year: i16::try_from(year).expect("a year within the timestamp range"),
            month: field(month),
            day: field(day),
            hour: field(second_of_day / SECONDS_PER_HOUR),
            minute: field(second_of_day % SECONDS_PER_HOUR / SECONDS_PER_MINUTE),
            second: field(second_of_day % SECONDS_PER_MINUTE),
            nanosecond,
        };
        assert!((YEAR_MIN..=YEAR_MAX).contains(&date_time.year));
        Ok(date_time)
    }

    pub(super) fn to_unix(self) -> Result<i64, OutOfRange> {
        let seconds = self.days() * SECONDS_PER_DAY
            + i64::from(self.hour) * SECONDS_PER_HOUR
            + i64::from(self.minute) * SECONDS_PER_MINUTE
            + i64::from(self.second);
        if !(UNIX_SECONDS_MIN..=UNIX_SECONDS_MAX).contains(&seconds) {
            return Err(OutOfRange);
        }
        Ok(seconds)
    }

    pub(super) const fn year(self) -> i16 {
        self.year
    }

    pub(super) const fn month(self) -> i8 {
        self.month
    }

    pub(super) const fn day(self) -> i8 {
        self.day
    }

    pub(super) const fn hour(self) -> i8 {
        self.hour
    }

    pub(super) const fn minute(self) -> i8 {
        self.minute
    }

    pub(super) const fn second(self) -> i8 {
        self.second
    }

    fn days(self) -> i64 {
        days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        )
    }

    pub(super) fn weekday_from_sunday(self) -> i8 {
        let weekday = (self.days() + EPOCH_DAYS_AFTER_SUNDAY).rem_euclid(DAYS_PER_WEEK);
        i8::try_from(weekday).expect("a weekday below 7")
    }

    pub(super) fn day_of_year(self) -> i16 {
        let first = days_from_civil(i64::from(self.year), 1, 1);
        i16::try_from(self.days() - first + 1).expect("a day of the year below 367")
    }

    fn iso_week(self) -> (i16, i8) {
        let days = self.days();
        let mut year = self.year;
        let mut start = iso_week_start(i64::from(year));
        if days < start {
            year -= 1;
            start = iso_week_start(i64::from(year));
        } else if self.month == DECEMBER
            && self.day >= ISO_WEEK_YEAR_CHANGE_DAY_MIN
            && year < YEAR_MAX
        {
            let next = iso_week_start(i64::from(year) + 1);
            if days >= next {
                start = next;
                year += 1;
            }
        }
        let week = (days - start) / DAYS_PER_WEEK + 1;
        (year, i8::try_from(week).expect("an ISO week below 54"))
    }

    const fn hour_of_half_day(self) -> i8 {
        if self.hour == 0 {
            HOURS_PER_HALF_DAY
        } else if self.hour > HOURS_PER_HALF_DAY {
            self.hour - HOURS_PER_HALF_DAY
        } else {
            self.hour
        }
    }

    pub(super) fn format(self, format: &str) -> String {
        let mut output = String::with_capacity(format.len());
        self.write_format(format, &mut output);
        output
    }

    fn write_format(self, format: &str, output: &mut String) {
        let mut rest = format;
        while let Some(percent) = rest.find('%') {
            output.push_str(&rest[..percent]);
            rest = &rest[percent + 1..];
            if rest.is_empty() {
                output.push('%');
                return;
            }
            match self.write_directive(rest.as_bytes(), output) {
                Ok(consumed) => rest = &rest[consumed..],
                Err(Unformattable) => output.push('%'),
            }
        }
        output.push_str(rest);
    }

    fn write_directive(self, spec: &[u8], output: &mut String) -> Result<usize, Unformattable> {
        let (extension, index) = parse_extension(spec)?;
        let directive = spec[index];
        let integer = |output: &mut String, pad: u8, width: u8, number: i64| {
            write_integer(extension, pad, width, number, output);
        };
        let text = |output: &mut String, text: &str| write_text(extension, text, output);
        let uppercase = extension.flag == Some(Flag::Uppercase);
        let year = i64::from(self.year);
        match directive {
            b'%' => text(output, "%"),
            b'A' => text(output, WEEKDAY_NAMES[self.weekday_index()]),
            b'a' => text(output, WEEKDAY_ABBREVIATIONS[self.weekday_index()]),
            b'B' => text(output, MONTH_NAMES[self.month_index()]),
            b'b' | b'h' => text(output, MONTH_ABBREVIATIONS[self.month_index()]),
            b'C' => integer(output, b' ', 0, i64::from(self.year / YEARS_PER_CENTURY)),
            b'c' if uppercase => self.write_format(DATE_TIME_UPPERCASE_FORMAT, output),
            b'c' => self.write_format(DATE_TIME_FORMAT, output),
            b'D' => {
                let year = self.year % YEARS_PER_CENTURY;
                push_two_digits(output, i64::from(self.month));
                output.push('/');
                push_two_digits(output, i64::from(self.day));
                output.push('/');
                if year < 0 {
                    output.push('-');
                }
                push_two_digits(output, i64::from(year.unsigned_abs()));
            }
            b'd' => integer(output, b'0', 2, i64::from(self.day)),
            b'e' => integer(output, b' ', 2, i64::from(self.day)),
            b'F' => {
                if year < 0 {
                    output.push('-');
                }
                push_padded(output, year.unsigned_abs(), b'0', 4);
                output.push('-');
                push_two_digits(output, i64::from(self.month));
                output.push('-');
                push_two_digits(output, i64::from(self.day));
            }
            b'f' => {
                if extension.width == Some(0) {
                    return Err(Unformattable);
                }
                if self.nanosecond == 0 && extension.width.is_none() {
                    output.push('0');
                } else {
                    push_fraction(output, extension.width, self.nanosecond);
                }
            }
            b'G' => integer(output, b'0', 4, i64::from(self.iso_week().0)),
            b'g' => integer(
                output,
                b'0',
                2,
                i64::from(self.iso_week().0 % YEARS_PER_CENTURY),
            ),
            b'H' => integer(output, b'0', 2, i64::from(self.hour)),
            b'I' => integer(output, b'0', 2, i64::from(self.hour_of_half_day())),
            b'j' => integer(output, b'0', 3, i64::from(self.day_of_year())),
            b'k' => integer(output, b' ', 2, i64::from(self.hour)),
            b'l' => integer(output, b' ', 2, i64::from(self.hour_of_half_day())),
            b'M' => integer(output, b'0', 2, i64::from(self.minute)),
            b'm' => integer(output, b'0', 2, i64::from(self.month)),
            b'N' => {
                if extension.width == Some(0) {
                    return Err(Unformattable);
                }
                let width = extension.width.unwrap_or(FRACTION_DIGITS as u8);
                push_fraction(output, Some(width), self.nanosecond);
            }
            b'n' => text(output, "\n"),
            b'P' => text(output, self.meridiem_lowercase()),
            b'p' if extension.flag == Some(Flag::Swapcase) => {
                text(output, self.meridiem_lowercase())
            }
            b'p' if self.hour < HOURS_PER_HALF_DAY => text(output, "AM"),
            b'p' => text(output, "PM"),
            b'q' => integer(
                output,
                b'0',
                0,
                i64::from((self.month - 1) / MONTHS_PER_QUARTER + 1),
            ),
            b'R' => {
                push_two_digits(output, i64::from(self.hour));
                output.push(':');
                push_two_digits(output, i64::from(self.minute));
            }
            b'r' if uppercase => self.write_format(TWELVE_HOUR_UPPERCASE_FORMAT, output),
            b'r' => self.write_format(TWELVE_HOUR_FORMAT, output),
            b'S' => integer(output, b'0', 2, i64::from(self.second)),
            b'T' => self.write_format(TIME_FORMAT, output),
            b't' => text(output, "\t"),
            b'U' => {
                let week = (self.day_of_year() + WEEKDAYS_AFTER_SUNDAY
                    - i16::from(self.weekday_from_sunday()))
                    / DAYS_PER_WEEK_FIELD;
                integer(output, b'0', 2, i64::from(week));
            }
            b'u' => {
                let from_monday = (i16::from(self.weekday_from_sunday()) + WEEKDAYS_AFTER_SUNDAY)
                    % DAYS_PER_WEEK_FIELD;
                integer(output, b' ', 0, i64::from(from_monday + 1));
            }
            b'V' => integer(output, b'0', 2, i64::from(self.iso_week().1)),
            b'W' => {
                let from_monday = (i16::from(self.weekday_from_sunday()) + WEEKDAYS_AFTER_SUNDAY)
                    % DAYS_PER_WEEK_FIELD;
                let week = (self.day_of_year() + WEEKDAYS_AFTER_SUNDAY - from_monday)
                    / DAYS_PER_WEEK_FIELD;
                integer(output, b'0', 2, i64::from(week));
            }
            b'w' => integer(output, b' ', 0, i64::from(self.weekday_from_sunday())),
            b'X' => self.write_format(TIME_FORMAT, output),
            b'x' => self.write_format(DATE_FORMAT, output),
            b'Y' => integer(output, b'0', 4, year),
            b'y' => integer(output, b'0', 2, i64::from(self.year % YEARS_PER_CENTURY)),
            b'.' => return self.write_dot_fraction(spec, index + 1, output),
            _ => return Err(Unformattable),
        }
        Ok(index + 1)
    }

    fn write_dot_fraction(
        self,
        spec: &[u8],
        start: usize,
        output: &mut String,
    ) -> Result<usize, Unformattable> {
        if start == spec.len() {
            return Err(Unformattable);
        }
        let (width, index) = parse_width(spec, start)?;
        if spec[index] != b'f' {
            return Err(Unformattable);
        }
        if !(self.nanosecond == 0 && width.is_none() || width == Some(0)) {
            output.push('.');
            push_fraction(output, width, self.nanosecond);
        }
        Ok(index + 1)
    }

    fn weekday_index(self) -> usize {
        usize::try_from(self.weekday_from_sunday()).expect("a weekday below 7")
    }

    fn month_index(self) -> usize {
        usize::try_from(self.month - 1).expect("a month from 1 to 12")
    }

    const fn meridiem_lowercase(self) -> &'static str {
        if self.hour < HOURS_PER_HALF_DAY {
            "am"
        } else {
            "pm"
        }
    }
}

fn parse_extension(spec: &[u8]) -> Result<(Extension, usize), Unformattable> {
    let mut extension = Extension {
        flag: None,
        width: None,
    };
    if spec[0].is_ascii_alphabetic() {
        return Ok((extension, 0));
    }
    extension.flag = match spec[0] {
        b'_' => Some(Flag::PadSpace),
        b'0' => Some(Flag::PadZero),
        b'-' => Some(Flag::NoPad),
        b'^' => Some(Flag::Uppercase),
        b'#' => Some(Flag::Swapcase),
        _ => None,
    };
    let mut index = usize::from(extension.flag.is_some());
    if index == spec.len() {
        return Err(Unformattable);
    }
    (extension.width, index) = parse_width(spec, index)?;
    let colons = spec[index..]
        .iter()
        .take(COLONS_MAX)
        .take_while(|byte| **byte == b':')
        .count();
    index += colons;
    if colons > 0 && index == spec.len() {
        return Err(Unformattable);
    }
    Ok((extension, index))
}

fn parse_width(spec: &[u8], start: usize) -> Result<(Option<u8>, usize), Unformattable> {
    let digits = spec[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return Ok((None, start));
    }
    let end = start + digits;
    let width = spec[start..end]
        .iter()
        .try_fold(0_u8, |width, digit| {
            width
                .checked_mul(DECIMAL_BASE_DIGIT)?
                .checked_add(digit - b'0')
        })
        .ok_or(Unformattable)?;
    if end == spec.len() {
        return Err(Unformattable);
    }
    Ok((Some(width), end))
}

fn write_text(extension: Extension, text: &str, output: &mut String) {
    if extension.flag == Some(Flag::Uppercase) {
        output.extend(text.chars().map(|character| character.to_ascii_uppercase()));
    } else {
        output.push_str(text);
    }
}

fn write_integer(extension: Extension, pad: u8, width: u8, number: i64, output: &mut String) {
    let pad = match extension.flag {
        Some(Flag::PadZero) => b'0',
        Some(Flag::PadSpace) => b' ',
        _ => pad,
    };
    let width = if extension.flag == Some(Flag::NoPad) {
        0
    } else {
        extension.width.unwrap_or(width)
    };
    let magnitude = number.unsigned_abs();
    if number < 0 && width > 0 {
        let width = width - 1;
        if pad == b' ' {
            let digits = u8::try_from(magnitude.checked_ilog10().unwrap_or(0) + 1)
                .expect("an i64 has at most 19 digits");
            output.extend(core::iter::repeat_n(
                ' ',
                usize::from(width.saturating_sub(digits)),
            ));
            output.push('-');
            push_padded(output, magnitude, pad, 0);
        } else {
            output.push('-');
            push_padded(output, magnitude, pad, width);
        }
        return;
    }
    if number < 0 {
        output.push('-');
    }
    push_padded(output, magnitude, pad, width);
}

fn push_padded(output: &mut String, number: u64, pad: u8, width: u8) {
    assert!(pad.is_ascii());
    let mut digits = [0_u8; INTEGER_DIGITS_MAX as usize];
    let mut start = digits.len();
    let mut rest = number;
    loop {
        start -= 1;
        digits[start] = b'0' + u8::try_from(rest % DECIMAL_BASE).expect("a decimal digit");
        rest /= DECIMAL_BASE;
        if rest == 0 {
            break;
        }
    }
    let length = digits.len() - start;
    let width = usize::from(width.min(INTEGER_DIGITS_MAX));
    output.extend(core::iter::repeat_n(
        char::from(pad),
        width.saturating_sub(length),
    ));
    output.push_str(core::str::from_utf8(&digits[start..]).expect("ASCII digits"));
}

fn push_two_digits(output: &mut String, number: i64) {
    let number = u64::try_from(number).expect("a non-negative two-digit field");
    assert!(number < DECIMAL_BASE * DECIMAL_BASE);
    push_padded(output, number, b'0', 2);
}

fn push_fraction(output: &mut String, precision: Option<u8>, nanosecond: i32) {
    let mut digits = [b'0'; FRACTION_DIGITS];
    let mut rest = u32::try_from(nanosecond).expect("a non-negative nanosecond");
    for digit in digits.iter_mut().rev() {
        *digit = b'0' + u8::try_from(rest % DECIMAL_BASE_FRACTION).expect("a decimal digit");
        rest /= DECIMAL_BASE_FRACTION;
    }
    let end = match precision {
        Some(precision) => usize::from(precision).min(FRACTION_DIGITS),
        None => digits
            .iter()
            .rposition(|digit| *digit != b'0')
            .map_or(0, |last| last + 1),
    };
    output.push_str(core::str::from_utf8(&digits[..end]).expect("ASCII digits"));
}
