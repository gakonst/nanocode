//! `rg`: ripgrep 15.2.0's command line on the ripgrep crates (D020, D1133, ARCH
//! "Search"). `flags/`, `main.rs`, `search.rs`, `haystack.rs` and `messages.rs` are
//! ripgrep's `crates/core` at 91f5d22 with the seams patched (each patch is marked
//! `psh:`), `lexopt.rs` is lexopt 0.3.2; `anyhow.rs`, `textwrap.rs` and `cli.rs` stand in
//! for the crates ripgrep uses there.
//!
//! The front end here parses (with `RIPGREP_CONFIG_PATH` read through the utility's file
//! system), prints `--help`/`--version`, runs the real rg for what psh leaves to it
//! (D1082: `--pre`, `-z`, PCRE2, `--debug`/`--trace`), and runs the search as a
//! blocking utility (D1130): `main::run_sync`.

// The copied code keeps ripgrep's style.
#![allow(clippy::all)]

#[macro_use]
mod messages;

mod anyhow;
mod bytes;
mod cli;
mod flags;
mod haystack;
mod json;
#[rustfmt::skip]
mod lexopt;
mod main;
mod search;
mod textwrap;

use std::ffi::OsString;

use psh_engine::host::OpenOptions;
use psh_engine::util::{BlockingOptions, UtilCtx};

use flags::{EngineChoice, LowArgs, Mode, ParseResult, PatternSource};

/// The options with values, short and long, for the classifier check (D1090): from
/// ripgrep's own flag table.
#[cfg(test)]
pub(crate) fn value_flags() -> (String, Vec<String>) {
    flags::value_flags()
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let config = match ctx.var(b"RIPGREP_CONFIG_PATH") {
        Some(p) if !p.is_empty() => {
            let p = p.to_vec();
            let contents = read_file(ctx, &p).await;
            Some(flags::ConfigSource {
                path: crate::search::walk::to_path(&p),
                contents,
            })
        }
        _ => None,
    };
    let args: Vec<OsString> = argv[1..].iter().map(|a| os_string(a)).collect();
    let (result, mut msgs) =
        messages::scoped(messages::State::buffered(), || {
            flags::parse_low(args, config)
        });
    let low = match result {
        ParseResult::Special(mode) => {
            let (text, status) = main::special(mode);
            ctx.error_raw(&msgs.take_buffered()).await;
            if let Err(e) = ctx.write(text.as_bytes()).await
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                return 2;
            }
            return status;
        }
        ParseResult::Err(err) => {
            let mut text = msgs.take_buffered();
            text.extend_from_slice(format!("rg: {err:#}\n").as_bytes());
            ctx.error_raw(&text).await;
            return 2;
        }
        ParseResult::Ok(low) => low,
    };
    if let Some(what) = left_to_rg(&low) {
        return crate::textio::fall_through(ctx, argv, what).await;
    }
    let stdin = reads_stdin(&low, ctx.stdin_readable());
    ctx.run_blocking(BlockingOptions { stdin }, (low, msgs), main::run_sync)
        .await
}

/// What psh leaves to the real rg (D1082), if `low` asks for it.
fn left_to_rg(low: &LowArgs) -> Option<&'static str> {
    if low.pre.is_some() {
        return Some("--pre");
    }
    if low.search_zip {
        return Some("-z/--search-zip");
    }
    match low.engine {
        EngineChoice::Default => {}
        EngineChoice::PCRE2 => return Some("-P/--pcre2"),
        EngineChoice::Auto => return Some("--engine auto"),
    }
    if low.logging.is_some() {
        return Some("--debug/--trace");
    }
    None
}

/// Whether the search reads stdin: `-f -`, a `-` path, or no path with stdin readable
/// (ripgrep's heuristic). The inline backend reads it first only then.
fn reads_stdin(low: &LowArgs, stdin_readable: bool) -> bool {
    if low
        .patterns
        .iter()
        .any(|p| matches!(p, PatternSource::File(f) if f.as_os_str() == "-"))
    {
        return true;
    }
    // Without -e/-f, the first positional argument is the pattern.
    let skip = usize::from(low.patterns.is_empty());
    let paths = low.positional.get(skip..).unwrap_or(&[]);
    if paths.iter().any(|p| p == "-") {
        return true;
    }
    paths.is_empty() && matches!(low.mode, Mode::Search(_)) && stdin_readable
}

async fn read_file(
    ctx: &mut UtilCtx<'_>,
    path: &[u8],
) -> std::io::Result<Vec<u8>> {
    let f = ctx.open(path, OpenOptions::read()).await?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).await?;
    Ok(out)
}

/// An argument as the `OsString` lexopt parses (lossy where the platform's strings
/// are not bytes).
fn os_string(a: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(a.to_vec())
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(a).into_owned())
    }
}

#[cfg(test)]
mod tests;
