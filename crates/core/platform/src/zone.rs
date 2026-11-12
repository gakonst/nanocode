use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
mod tzif;

const SECONDS_PER_DAY: i128 = 86_400;
const DAYS_PER_ERA: i64 = 146_097;
const YEARS_PER_ERA: i64 = 400;
const DAYS_PER_YEAR: i64 = 365;
const YEARS_PER_LEAP: i64 = 4;
const YEARS_PER_CENTURY: i64 = 100;
const DAYS_BEFORE_EPOCH_SINCE_ERA: i64 = 719_468;
const MONTHS_PER_YEAR: i64 = 12;
const MONTHS_BEFORE_MARCH: i64 = 2;
const DAYS_PER_FIVE_MONTHS: i64 = 153;
const MONTHS_PER_FIVE: i64 = 5;

fn unix_seconds(at: SystemTime) -> i128 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::from(after.as_secs()),
        Err(before) => {
            let before = before.duration();
            -i128::from(before.as_secs()) - i128::from(before.subsec_nanos() > 0)
        }
    }
}

pub(crate) const fn civil(days: i64) -> (i64, i64, i64) {
    let shifted = days + DAYS_BEFORE_EPOCH_SINCE_ERA;
    let era = shifted.div_euclid(DAYS_PER_ERA);
    let day_of_era = shifted.rem_euclid(DAYS_PER_ERA);
    let year_of_era = (day_of_era - day_of_era / (DAYS_PER_YEAR * YEARS_PER_LEAP)
        + day_of_era
            / (DAYS_PER_YEAR * YEARS_PER_CENTURY + YEARS_PER_CENTURY / YEARS_PER_LEAP - 1)
        - day_of_era / (DAYS_PER_ERA - 1))
        / DAYS_PER_YEAR;
    let day_of_year = day_of_era
        - (DAYS_PER_YEAR * year_of_era + year_of_era / YEARS_PER_LEAP
            - year_of_era / YEARS_PER_CENTURY);
    let month_from_march =
        (MONTHS_PER_FIVE * day_of_year + MONTHS_BEFORE_MARCH) / DAYS_PER_FIVE_MONTHS;
    let day = day_of_year
        - (DAYS_PER_FIVE_MONTHS * month_from_march + MONTHS_BEFORE_MARCH) / MONTHS_PER_FIVE
        + 1;
    let month = if month_from_march < MONTHS_PER_YEAR - MONTHS_BEFORE_MARCH {
        month_from_march + MONTHS_BEFORE_MARCH + 1
    } else {
        month_from_march + MONTHS_BEFORE_MARCH + 1 - MONTHS_PER_YEAR
    };
    let year = year_of_era + era * YEARS_PER_ERA + (month <= MONTHS_BEFORE_MARCH) as i64;
    (year, month, day)
}

#[cfg(unix)]
pub(crate) const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= MONTHS_BEFORE_MARCH {
        year - 1
    } else {
        year
    };
    let era = year.div_euclid(YEARS_PER_ERA);
    let year_of_era = year.rem_euclid(YEARS_PER_ERA);
    let month_from_march = (month + MONTHS_PER_YEAR - MONTHS_BEFORE_MARCH - 1) % MONTHS_PER_YEAR;
    let day_of_year =
        (DAYS_PER_FIVE_MONTHS * month_from_march + MONTHS_BEFORE_MARCH) / MONTHS_PER_FIVE + day - 1;
    let day_of_era = year_of_era * DAYS_PER_YEAR + year_of_era / YEARS_PER_LEAP
        - year_of_era / YEARS_PER_CENTURY
        + day_of_year;
    era * DAYS_PER_ERA + day_of_era - DAYS_BEFORE_EPOCH_SINCE_ERA
}

fn date(seconds: i128) -> String {
    let days = i64::try_from(seconds.div_euclid(SECONDS_PER_DAY))
        .expect("a SystemTime's day count fits in i64");
    let (year, month, day) = civil(days);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn utc_date(at: SystemTime) -> String {
    date(unix_seconds(at))
}

pub fn local_date(at: SystemTime) -> String {
    date(unix_seconds(at) + i128::from(utc_offset_seconds(at)))
}

#[cfg(not(unix))]
pub const fn utc_offset_seconds(_at: SystemTime) -> i32 {
    0
}

#[cfg(not(unix))]
pub const fn time_zone_name() -> Option<String> {
    None
}

#[cfg(unix)]
pub use native::{time_zone_name, utc_offset_seconds};

#[cfg(unix)]
mod native {
    use super::tzif::{self, Zone};
    use std::cell::RefCell;
    use std::time::{Duration, Instant, SystemTime};

    const LOCALTIME: &str = "/etc/localtime";
    const TZDB: &str = "/usr/share/zoneinfo";
    const ZONE_DIRECTORIES: [&str; 4] = [
        TZDB,
        "/share/zoneinfo",
        "/etc/zoneinfo",
        "/usr/share/lib/zoneinfo",
    ];
    const RECHECK: Duration = Duration::from_secs(1);

    #[derive(PartialEq, Eq)]
    enum Source {
        Environment(String),
        LocalTime(Option<SystemTime>),
    }

    impl Source {
        fn read(environment: Option<&str>) -> Self {
            match environment {
                Some(value) => Self::Environment(value.to_owned()),
                None => Self::LocalTime(
                    std::fs::symlink_metadata(LOCALTIME)
                        .and_then(|data| data.modified())
                        .ok(),
                ),
            }
        }

        fn same(&self, other: &Self) -> bool {
            !matches!(
                (self, other),
                (Self::LocalTime(None), _) | (_, Self::LocalTime(None))
            ) && self == other
        }
    }

    struct Loaded {
        zone: Zone,
        source: Source,
        checked: Instant,
    }

    thread_local! {
        static LOADED: RefCell<Option<Loaded>> = const { RefCell::new(None) };
    }

    fn current(environment: Option<&str>) -> Zone {
        let local = match environment {
            Some(value) => tzif::from_environment(value, LOCALTIME, &ZONE_DIRECTORIES),
            None => tzif::from_path(LOCALTIME),
        };
        local
            .or_else(|| tzif::from_path(&format!("{TZDB}/{}", time_zone_name()?)))
            .unwrap_or_else(Zone::utc)
    }

    fn load() -> Loaded {
        let environment = std::env::var("TZ").ok();
        Loaded {
            zone: current(environment.as_deref()),
            source: Source::read(environment.as_deref()),
            checked: Instant::now(),
        }
    }

    pub fn utc_offset_seconds(at: SystemTime) -> i32 {
        let unix =
            i64::try_from(super::unix_seconds(at)).expect("a SystemTime on Unix holds i64 seconds");
        LOADED.with(|loaded| {
            let mut loaded = loaded.borrow_mut();
            let loaded = loaded.get_or_insert_with(load);
            if loaded.checked.elapsed() >= RECHECK {
                let environment = std::env::var("TZ").ok();
                let source = Source::read(environment.as_deref());
                if !loaded.source.same(&source) {
                    loaded.zone = current(environment.as_deref());
                }
                loaded.source = source;
                loaded.checked = Instant::now();
            }
            loaded
                .zone
                .offset(unix)
                .expect("a clock reading within the years a time zone rule covers")
        })
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn time_zone_name() -> Option<String> {
        const PREFIXES: [&str; 4] = [
            "/usr/share/zoneinfo/",
            "../usr/share/zoneinfo/",
            "/etc/zoneinfo/",
            "../etc/zoneinfo/",
        ];
        if let Ok(link) = std::fs::read_link(LOCALTIME)
            && let Some(link) = link.to_str()
            && let Some(name) = PREFIXES.iter().find_map(|prefix| link.strip_prefix(prefix))
        {
            return Some(name.to_owned());
        }
        if let Ok(name) = std::fs::read_to_string("/etc/timezone") {
            return Some(name.trim_end().to_owned());
        }
        openwrt(&std::fs::read_to_string("/etc/config/system").ok()?)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn word(line: &str) -> Result<Option<(&str, &str)>, ()> {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }
        for quote in ['\'', '"'] {
            if let Some(rest) = line.strip_prefix(quote) {
                return rest.split_once(quote).map(Some).ok_or(());
            }
        }
        Ok(Some(
            line.split_once(char::is_whitespace).unwrap_or((line, "")),
        ))
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn openwrt(config: &str) -> Option<String> {
        let mut system = false;
        let mut timezone = None;
        for line in config.split_inclusive('\n') {
            let mut rest = line;
            let mut next = || -> Result<Option<&str>, ()> {
                Ok(word(rest)?.map(|(item, tail)| {
                    rest = tail;
                    item
                }))
            };
            let Some(keyword) = next().ok()? else {
                continue;
            };
            if keyword == "config" {
                system = next().ok()? == Some("system") && next().ok()?.is_none();
            } else if system && keyword == "option" {
                let key = next().ok()?;
                if key == Some("zonename") {
                    if let (Some(name), None) = (next().ok()?, next().ok()?) {
                        return Some(name.to_owned());
                    }
                } else if key == Some("timezone")
                    && let (Some(value), None) = (next().ok()?, next().ok()?)
                {
                    timezone = Some(value.to_owned());
                }
            }
        }
        timezone
    }

    #[cfg(target_os = "macos")]
    pub fn time_zone_name() -> Option<String> {
        let link = std::fs::read_link(LOCALTIME).ok()?;
        link.to_str()?
            .split_once("/zoneinfo/")
            .map(|(_, name)| name.to_owned())
    }

    #[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
    pub use super::apple::time_zone_name;

    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    pub const fn time_zone_name() -> Option<String> {
        None
    }
}

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
#[allow(unsafe_code)]
mod apple {
    use std::ffi::c_void;

    const UTF8: u32 = 0x0800_0100;
    const UTF8_BYTES_PER_UNIT_MAX: isize = 3;
    const NAME_BYTES_MAX: usize = 64;

    #[repr(C)]
    struct Range {
        location: isize,
        length: isize,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFTimeZoneResetSystem();
        fn CFTimeZoneCopySystem() -> *const c_void;
        fn CFTimeZoneGetName(zone: *const c_void) -> *const c_void;
        fn CFStringGetLength(string: *const c_void) -> isize;
        fn CFStringGetBytes(
            string: *const c_void,
            range: Range,
            encoding: u32,
            loss: u8,
            external: u8,
            buffer: *mut u8,
            capacity: isize,
            used: *mut isize,
        ) -> isize;
        fn CFRelease(object: *const c_void);
    }

    pub fn time_zone_name() -> Option<String> {
        let zone = unsafe {
            CFTimeZoneResetSystem();
            CFTimeZoneCopySystem()
        };
        if zone.is_null() {
            return None;
        }
        let name = unsafe { CFTimeZoneGetName(zone) };
        let text = (!name.is_null()).then(|| {
            let length = unsafe { CFStringGetLength(name) };
            let mut buffer = vec![0_u8; usize::try_from(length * UTF8_BYTES_PER_UNIT_MAX).ok()?];
            let mut used = 0;
            let converted = unsafe {
                CFStringGetBytes(
                    name,
                    Range {
                        location: 0,
                        length,
                    },
                    UTF8,
                    0,
                    0,
                    buffer.as_mut_ptr(),
                    length * UTF8_BYTES_PER_UNIT_MAX,
                    &mut used,
                )
            };
            buffer.truncate(usize::try_from(used).ok()?);
            (converted == length)
                .then(|| String::from_utf8(buffer).ok())
                .flatten()
        });
        unsafe { CFRelease(zone) };
        text.flatten()
            .filter(|name| !name.is_empty() && name.len() <= NAME_BYTES_MAX)
    }
}
