#![allow(
    clippy::disallowed_methods,
    reason = "build scripts cannot use rustpython-host_env"
)]

use nanocode_platform::{civil_from_days, utc_offset_seconds};
use std::{
    env, io,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const UNIX_SECONDS_MIN: i64 = -377_705_023_201;
const UNIX_SECONDS_MAX: i64 = 253_402_207_200;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_DAY: i64 = 86_400;
const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

struct LocalTime {
    year: i64,
    month: i64,
    day: i64,
    second_of_day: i64,
}

fn main() {
    let frozen_libs: &[&str] = if cfg!(feature = "freeze-stdlib") {
        &["Lib"]
    } else {
        &["Lib", "python_builtins"]
    };
    for entry in python_files(frozen_libs) {
        let display = entry.display();
        println!("cargo:rerun-if-changed={display}");
    }
    println!("cargo:rerun-if-changed=../../Lib/importlib/_bootstrap.py");

    // = 3.14.0alpha
    python_version(3, 14, 0, "alpha", 0);

    println!("cargo:rustc-env=RUSTPYTHON_GIT_HASH={}", git_hash());
    println!(
        "cargo:rustc-env=RUSTPYTHON_GIT_TIMESTAMP={}",
        git_timestamp()
    );
    println!("cargo:rustc-env=RUSTPYTHON_GIT_TAG={}", git_tag());
    println!("cargo:rustc-env=RUSTPYTHON_GIT_BRANCH={}", git_branch());
    println!(
        "cargo:rustc-env=RUSTPYTHON_GIT_IDENTIFIER={}",
        git_identifier()
    );
    println!("cargo:rustc-env=RUSTPYTHON_BUILD_INFO={}", get_build_info());
    println!("cargo:rustc-env=RUSTC_VERSION={}", rustc_version());

    let release_level = option_env!("RUSTPYTHON_RELEASE_LEVEL").unwrap_or("alpha");
    println!("cargo:rustc-env=RUSTPYTHON_RELEASE_LEVEL={release_level}");
    let release_level_n = release_to_n(release_level);
    println!("cargo:rustc-env=RUSTPYTHON_RELEASE_LEVEL_N={release_level_n}");
    let release_serial = option_env!("RUSTPYTHON_RELEASE_SERIAL").unwrap_or("0");
    println!("cargo:rustc-env=RUSTPYTHON_RELEASE_SERIAL={release_serial}");

    println!(
        "cargo:rustc-env=RUSTPYTHON_TARGET_TRIPLE={}",
        env::var("TARGET").unwrap()
    );
}

fn git_hash() -> String {
    git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|_| "0000000".into())
}

fn git_timestamp() -> String {
    git(&["log", "-1", "--format=%ct"]).unwrap_or_else(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string()
    })
}

fn git_tag() -> String {
    git(&["describe", "--all", "--always", "--dirty"])
        .unwrap_or_else(|_| "heads/unknown-branch".into())
}

fn git_branch() -> String {
    git(&["name-rev", "--name-only", "HEAD"]).unwrap_or_else(|_| "unknown-branch".into())
}

fn git(args: &[&str]) -> io::Result<String> {
    command("git", args)
}

#[must_use]
fn get_build_info() -> String {
    // See: https://reproducible-builds.org/docs/timestamps/
    let revision = git_hash();
    let separator = if revision.is_empty() { "" } else { ":" };
    let identifier = git_identifier();

    format!(
        "{id}{sep}{revision}, {date:.20}, {time:.9}",
        id = if identifier.is_empty() {
            "default"
        } else {
            &identifier
        },
        sep = separator,
        revision = revision,
        date = get_git_date(),
        time = get_git_time(),
    )
}

fn git_identifier() -> String {
    let tag = git_tag();
    if tag.is_empty() || tag.eq_ignore_ascii_case("undefined") {
        git_branch()
    } else {
        tag
    }
}

fn get_git_timestamp_raw() -> Option<i64> {
    let git_timestamp = git_timestamp().parse::<i64>().ok()?;
    (UNIX_SECONDS_MIN..=UNIX_SECONDS_MAX)
        .contains(&git_timestamp)
        .then_some(git_timestamp)
}

fn now_seconds() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).expect("the clock reads a year before 10000"),
        Err(before) => {
            let before = before.duration();
            -i64::try_from(before.as_secs()).expect("the clock reads a year after -10000")
                - i64::from(before.subsec_nanos() > 0)
        }
    }
}

fn local_time(seconds: i64) -> LocalTime {
    let magnitude = Duration::from_secs(seconds.unsigned_abs());
    let at = if seconds < 0 {
        UNIX_EPOCH - magnitude
    } else {
        UNIX_EPOCH + magnitude
    };
    let local = seconds + i64::from(utc_offset_seconds(at));
    let (year, month, day) = civil_from_days(local.div_euclid(SECONDS_PER_DAY));
    LocalTime {
        year,
        month,
        day,
        second_of_day: local.rem_euclid(SECONDS_PER_DAY),
    }
}

fn get_git_timestamp_datetime() -> LocalTime {
    local_time(get_git_timestamp_raw().unwrap_or_else(now_seconds))
}

#[must_use]
fn get_git_date() -> String {
    let datetime = get_git_timestamp_datetime();
    let month =
        MONTH_ABBREVIATIONS[usize::try_from(datetime.month - 1).expect("a month from 1 to 12")];
    let year = if datetime.year < 0 {
        format!("-{:03}", datetime.year.unsigned_abs())
    } else {
        format!("{:04}", datetime.year)
    };
    format!("{month} {:>2} {year}", datetime.day)
}

#[must_use]
fn get_git_time() -> String {
    let datetime = get_git_timestamp_datetime();
    let second = datetime.second_of_day;
    format!(
        "{:02}:{:02}:{:02}",
        second / SECONDS_PER_HOUR,
        second % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        second % SECONDS_PER_MINUTE
    )
}

fn rustc_version() -> String {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    command(rustc, &["-V"]).unwrap_or_else(|_| "rustc [unknown]".into())
}

fn python_version(major: usize, minor: usize, micro: usize, release: &str, serial: usize) {
    println!("cargo:rustc-env=MAJOR_CPY={major}");
    println!("cargo:rustc-env=MINOR_CPY={minor}");
    println!("cargo:rustc-env=MICRO_CPY={micro}");
    println!("cargo:rustc-env=RELEASE_LEVEL_CPY={release}");
    println!(
        "cargo:rustc-env=RELEASE_LEVEL_N_CPY={}",
        release_to_n(release)
    );
    println!("cargo:rustc-env=SERIAL_CPY={serial}");

    println!("cargo:rustc-env=WINVER_CPY={major}.{minor}",);

    let cpy_version = format!("{major}.{minor}.{micro}.{release}");

    let (left, right) = get_version(&cpy_version);
    println!("cargo:rustc-env=RUSTPYTHON_VERSION_LEFT={left}");
    println!("cargo:rustc-env=RUSTPYTHON_VERSION_RIGHT={right}");
}

#[must_use]
fn get_version(cpy_version: &str) -> (String, String) {
    // Windows: include MSC v. for compatibility with ctypes.util.find_library
    // MSC v.1929 = VS 2019, version 14+ makes find_msvcrt() return None
    let msc_info = cfg_select! {
        windows => {{
            // Include both RustPython identifier and MSC v. for compatibility
            if cfg!(target_pointer_width = "64") {
                " MSC v.1929 64 bit (AMD64)"
            } else {
                " MSC v.1929 32 bit (Intel)"
            }
        }},
        _ => "",
    };

    // `left` and `right` are split by \n like PyPy. Passing a string with a newline to rustc
    // truncates everything from the newline onward, so we have to manually combine them later.
    let left = format!("{:.80} ({:.80})", cpy_version, get_build_info());
    let right = format!(
        "[RustPython {} with {:.80}{}]",
        env!("CARGO_PKG_VERSION"),
        rustc_version(),
        msc_info,
    );
    (left, right)
}

fn release_to_n(release: &str) -> usize {
    match release {
        "alpha" => 0xA,
        "beta" => 0xB,
        "candidate" => 0xC,
        "final" => 0xD,
        _ => panic!("`release` must be one of: 'alpha', 'beta', 'candidate', 'final'"),
    }
}

fn command(cmd: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> io::Result<String> {
    Command::new(&cmd).args(args).output().and_then(|output| {
        // TODO: Switch to exit_ok()? when stable.
        if !output.status.success() {
            Err(io::Error::other(format!(
                "command '{}' exited with status {}",
                cmd.as_ref().to_string_lossy(),
                output.status
            )))?
        }

        str::from_utf8(&output.stdout)
            .map(|s| s.trim().to_owned())
            .map_err(io::Error::other)
    })
}

fn python_files(prefix: &[&str]) -> Vec<std::path::PathBuf> {
    let root: std::path::PathBuf = prefix.iter().collect();
    let directories: Vec<std::path::PathBuf> = if prefix.len() == 1 {
        std::fs::read_dir(&root)
            .expect("Lib/ exists?")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    } else {
        vec![root]
    };
    let mut files = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "py") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
