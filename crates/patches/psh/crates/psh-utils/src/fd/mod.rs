//! `fd` (D1138, ARCH "Search"): fd 10.5.1's command line, filters and output on the
//! walker seam shared with `rg` and `grep`. The command line is psh's ([`cli`], after
//! fd's clap definition); `filter/`, `fmt/`, `exec.rs`, `output.rs`, `patterns.rs`,
//! `sanitize.rs` and the filter and receiver logic in `walk.rs` are fd's at 193d0df
//! (`exec` and `output` adapted to psh's byte paths and command runner), `colors.rs`
//! what fd uses of lscolors and nu-ansi-term.
//!
//! The front end here parses, checks the paths and patterns as fd's `run` does, and
//! runs the walk as a blocking utility (D1130): [`walk::run_sync`] prints the results,
//! or collects them for `-x`/`-X`/`-l`, whose commands then run here through the
//! command runner (a blocking body cannot spawn). What psh leaves to the real fd
//! (D1082): `--changed-within`/`--changed-before` with a duration or a date (only
//! `@timestamp` is parsed until the shared date parser exists), `--owner` with names,
//! `--hyperlink`, and options psh does not know.
//!
//! Differences on purpose: results are in path order (fd's when its search ends within
//! its buffering time), `-x` commands run one after another in that order, `--threads`
//! changes nothing, and on Windows paths print with `/` unless `--path-separator`
//! says otherwise.

mod cli;
mod colors;
mod exec;
mod filter;
mod fmt;
mod output;
mod patterns;
mod sanitize;
mod walk;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use psh_engine::host::{FileKind, OpenOptions};
use psh_engine::util::{BlockingOptions, UtilCtx};
use psh_pattern::bytes::RegexSetBuilder;

pub use cli::SPEC;
use cli::{CliError, Opts, Parsed, When};
use exec::{CommandSet, ExecOptions};
use filter::{OwnerError, OwnerFilter, TimeFilter};
use output::PrintConfig;
use patterns::{PatternType, build_patterns, pattern_has_uppercase_char};
use walk::{Config, FileTypes};

use crate::search::walk::{WalkOptions, is_absolute, join};

const HELP_LONG: &str = include_str!("help-long.txt");
const HELP_SHORT: &str = include_str!("help-short.txt");

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let opts = match cli::parse(&argv[1..]) {
        Ok(Parsed::Run(o)) => o,
        Ok(Parsed::Help { long }) => {
            let text = help(if long { HELP_LONG } else { HELP_SHORT });
            let _ = ctx.write(text.as_bytes()).await;
            let _ = ctx.flush().await;
            return 0;
        }
        Ok(Parsed::Version) => {
            let _ = ctx.write(b"fd 10.5.1\n").await;
            let _ = ctx.flush().await;
            return 0;
        }
        Err(CliError::Usage(msg)) => {
            ctx.error_raw(format!("{msg}\n").as_bytes()).await;
            return 2;
        }
        Err(CliError::Unknown(opt)) => {
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            ctx.error(&format!("unsupported option '{opt}' (psh built-in fd)"))
                .await;
            return 2;
        }
    };
    // The command templates are checked while clap parses.
    let command = match command_set(ctx, &opts).await {
        Ok(c) => c,
        Err(Failure::Usage(msg)) => {
            ctx.error_raw(
                format!(
                    "error: {msg}\n\nUsage: fd [OPTIONS] [pattern] [path]...\n\nFor more \
                     information, try '--help'.\n"
                )
                .as_bytes(),
            )
            .await;
            return 2;
        }
        Err(Failure::Fd(msg)) => return fail(ctx, &msg).await,
    };
    if let Some(what) = left_to_fd(&opts) {
        return crate::textio::fall_through(ctx, argv, what).await;
    }
    match prepare(ctx, *opts, command).await {
        Ok((config, command, exec_opts)) => {
            let results = config.results.clone();
            let status = ctx
                .run_blocking(BlockingOptions::default(), Box::new(config), walk::run_sync)
                .await;
            let (Some(command), Some(results)) = (command, results) else {
                return status;
            };
            let paths = std::mem::take(&mut *results.lock().unwrap_or_else(|e| e.into_inner()));
            let code = if command.in_batch_mode() {
                command.execute_batch(ctx, &paths, &exec_opts).await
            } else {
                command.execute(ctx, &paths, &exec_opts).await
            };
            let _ = ctx.flush().await;
            if status != 0 { status } else { code }
        }
        Err(msg) => fail(ctx, &msg).await,
    }
}

/// The help text for this platform (fd has no `--owner` on Windows).
fn help(text: &str) -> String {
    if !cfg!(windows) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        if line.starts_with("  -o, --owner") {
            // The long help's paragraph runs to the next blank line.
            skipping = text.len() > 4000;
            continue;
        }
        if skipping {
            if line.trim().is_empty() && !line.starts_with("          ") {
                skipping = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

enum Failure {
    /// clap's error (status 2).
    Usage(String),
    /// fd's own error (status 1).
    Fd(String),
}

/// `[fd error]: MSG`, status 1.
async fn fail(ctx: &mut UtilCtx<'_>, msg: &str) -> i32 {
    ctx.error_raw(format!("[fd error]: {msg}\n").as_bytes())
        .await;
    1
}

/// fd's `SanitizeErr` for errors that name paths: control characters escaped.
pub(crate) fn sanitize_error(msg: &str) -> String {
    struct S<'a>(&'a str);
    impl std::fmt::Display for S<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            sanitize::write_sanitized(f, self.0)
        }
    }
    S(msg).to_string()
}

/// What psh leaves to the real fd (D1082), if `o` asks for it.
fn left_to_fd(o: &Opts) -> Option<&'static str> {
    if o.changed_within
        .as_deref()
        .is_some_and(|t| TimeFilter::after(t).is_none())
        || o.changed_before
            .as_deref()
            .is_some_and(|t| TimeFilter::before(t).is_none())
    {
        return Some("--changed-within/--changed-before value (a date or duration)");
    }
    if let Some(owner) = &o.owner {
        if cfg!(windows) {
            return Some("option '--owner'");
        }
        if OwnerFilter::from_string(owner) == Err(OwnerError::NeedsNames) {
            return Some("--owner with a user or group name");
        }
    }
    if matches!(o.hyperlink, Some(When::Always | When::Auto)) {
        return Some("--hyperlink");
    }
    None
}

/// `-x`/`-X`, or `-l`'s `ls`: fd's `extract_command`. The colors decide `ls`'s.
async fn command_set(ctx: &mut UtilCtx<'_>, o: &Opts) -> Result<Option<CommandSet>, Failure> {
    if !o.exec.is_empty() {
        let set = if o.exec_batch {
            CommandSet::new_batch(o.exec.clone())
        } else {
            CommandSet::new(o.exec.clone())
        };
        return set.map(Some).map_err(Failure::Usage);
    }
    if !o.list_details {
        return Ok(None);
    }
    let colored = colored_output(ctx, o);
    let ls = determine_ls_command(ctx, colored)
        .await
        .map_err(Failure::Fd)?;
    let cmd = ls.iter().map(|a| a.as_bytes().to_vec()).collect();
    CommandSet::new_batch(vec![cmd])
        .map(Some)
        .map_err(Failure::Usage)
}

/// fd's `determine_ls_command`, with psh's lookups for `gls` and GNU `ls`.
async fn determine_ls_command(
    ctx: &mut UtilCtx<'_>,
    colored: bool,
) -> Result<Vec<&'static str>, String> {
    let gnu_ls = |command_name| {
        let color_arg = if colored {
            "--color=always"
        } else {
            "--color=never"
        };
        // Note: we use short options here (instead of --long-options) to support more
        // platforms (like BusyBox).
        vec![
            command_name,
            "-l", // long listing format
            "-h", // human readable file sizes
            "-d", // list directories themselves, not their contents
            color_arg,
        ]
    };
    let path = ctx.var(b"PATH").unwrap_or_default().to_vec();
    let bsd = cfg!(any(
        target_os = "macos",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ));
    if cfg!(unix) && !bsd {
        // Assume ls is GNU ls
        Ok(gnu_ls("ls"))
    } else if bsd {
        // Use GNU ls, if available (support for --color=auto, better LS_COLORS support)
        if !ctx.search_path(b"gls", &path).await.is_empty() {
            Ok(gnu_ls("gls"))
        } else {
            let mut cmd = vec![
                "ls", // BSD version of ls
                "-l", // long listing format
                "-h", // '--human-readable' is not available, '-h' is
                "-d", // '--directory' is not available, but '-d' is
            ];
            if !cfg!(any(target_os = "netbsd", target_os = "openbsd")) && colored {
                // -G is not available in NetBSD's and OpenBSD's ls
                cmd.push("-G");
            }
            Ok(cmd)
        }
    } else if cfg!(windows) {
        // Use GNU ls, if available (psh: an `ls` on PATH)
        if !ctx.search_path(b"ls", &path).await.is_empty() {
            Ok(gnu_ls("ls"))
        } else {
            Err(
                "'fd --list-details' is not supported on Windows unless GNU 'ls' is installed."
                    .to_string(),
            )
        }
    } else {
        Err("'fd --list-details' is not supported on this platform.".to_string())
    }
}

fn colored_output(ctx: &UtilCtx<'_>, o: &Opts) -> bool {
    match o.color.unwrap_or(When::Auto) {
        When::Always => true,
        When::Never => false,
        When::Auto => {
            let no_color = ctx.var(b"NO_COLOR").is_some_and(|x| !x.is_empty());
            !no_color && ctx.is_terminal(1)
        }
    }
}

/// An I/O error as Rust displays it: `No such file or directory (os error 2)`.
fn io_error_text(e: &std::io::Error) -> String {
    let code = match e.kind() {
        std::io::ErrorKind::NotFound => Some(2),
        std::io::ErrorKind::PermissionDenied => Some(13),
        _ => e.raw_os_error(),
    };
    let text = psh_engine::host::error_text(e);
    match code {
        Some(c) if !cfg!(windows) => format!("{text} (os error {c})"),
        _ => text,
    }
}

/// A path for fd's messages: Rust's `{:?}` of the lossy text.
fn debug(p: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(p))
}

/// fd's `run` up to the walk: the base directory, the search paths, the patterns and
/// the configuration (errors are fd's messages).
async fn prepare(
    ctx: &mut UtilCtx<'_>,
    mut opts: Opts,
    command: Option<CommandSet>,
) -> Result<(Config, Option<CommandSet>, ExecOptions), String> {
    // set_working_dir: the walk's cwd only.
    let cwd = match &opts.base_directory {
        Some(base) => {
            let abs = ctx.path(base);
            if !is_dir(ctx, &abs).await {
                return Err(format!(
                    "The '--base-directory' path {} is not a directory.",
                    debug(base)
                ));
            }
            abs
        }
        None => ctx.cwd().to_vec(),
    };
    let against = |p: &[u8]| {
        if is_absolute(p) {
            p.to_vec()
        } else {
            join(&cwd, p)
        }
    };

    // search_paths
    let given = if !opts.path.is_empty() {
        opts.path.clone()
    } else {
        opts.search_path.clone()
    };
    let mut roots = Vec::new();
    if given.is_empty() {
        roots.push(normalize_path(ctx, &opts, b"./", &cwd).await);
    } else {
        for p in &given {
            if is_dir(ctx, &against(p)).await {
                roots.push(normalize_path(ctx, &opts, p, &cwd).await);
            } else {
                // We use debug for the path to make it more readable if it has special characters
                let msg = format!("Search path {} is not a directory.", debug(p));
                fail(ctx, &sanitize_error(&msg)).await;
            }
        }
    }
    if roots.is_empty() {
        return Err("No valid search paths given.".to_string());
    }

    ensure_search_pattern_is_not_a_path(ctx, &opts, &cwd).await?;
    let lossy = |b: &Vec<u8>| String::from_utf8_lossy(b).into_owned();
    let mut patterns: Vec<String> = opts.exprs.iter().map(lossy).collect();
    if let Some(pattern) = opts.pattern.take() {
        patterns.push(lossy(&pattern));
    }

    let pattern_type = if opts.glob {
        PatternType::Glob
    } else if opts.exact {
        PatternType::Exact
    } else if opts.fixed_strings {
        PatternType::Fixed
    } else {
        PatternType::Regex
    };
    // The search will be case-sensitive if the command line flag is set or
    // if any of the patterns has an uppercase character (smart case).
    let ignore_case = opts.ignore_case
        || !(opts.case_sensitive
            || patterns
                .iter()
                .any(|pat| pattern_has_uppercase_char(pattern_type, pat)));

    // construct_config
    let path_separator = opts.path_separator.take().or_else(|| {
        // psh: `/` on Windows by default (fd: only under MSYS).
        cfg!(windows).then(|| "/".to_string())
    });
    let actual_path_separator = path_separator
        .clone()
        .unwrap_or_else(|| std::path::MAIN_SEPARATOR.to_string());
    if cfg!(windows)
        && let Some(sep) = &path_separator
        && sep.len() > 1
    {
        return Err(format!(
            "A path separator must be exactly one byte, but the given separator is {} bytes: \
             '{}'.\nIn some shells on Windows, '/' is automatically expanded. Try to use '//' \
             instead.",
            sep.len(),
            sep
        ));
    }
    let mut time_constraints = Vec::new();
    for (t, after) in [(&opts.changed_within, true), (&opts.changed_before, false)] {
        if let Some(t) = t {
            // Anything else ran the real fd.
            let f = if after {
                TimeFilter::after(t)
            } else {
                TimeFilter::before(t)
            };
            time_constraints.extend(f);
        }
    }
    // Checked by `cli::parse`; names ran the real fd.
    let owner_constraint = opts
        .owner
        .as_deref()
        .and_then(|o| OwnerFilter::from_string(o).ok())
        .and_then(OwnerFilter::filter_ignore);
    let interactive_terminal = ctx.is_terminal(1);
    let colored = colored_output(ctx, &opts);
    let ls_colors = colored.then(|| {
        let v = ctx
            .var(b"LS_COLORS")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(str::to_string);
        colors::LsColors::for_fd(v.as_deref())
    });
    let has_command = command.is_some();
    let full_path_base = if opts.full_path {
        Some(ctx.canonicalize(&cwd).await.unwrap_or_else(|_| cwd.clone()))
    } else {
        None
    };
    let ignore_hidden = !opts.hidden;
    let read_fdignore = !opts.no_ignore;
    let read_vcsignore = !(opts.no_ignore || opts.no_ignore_vcs);
    let read_global_ignore = !(opts.no_ignore || opts.no_global_ignore_file);

    // psh: on every platform (fd: Unix only), so one command behaves the same
    // everywhere; dot files are hidden on Windows too.
    if ignore_hidden && patterns::patterns_match_strings_with_leading_dots(&patterns, pattern_type)
    {
        return Err(
            "The pattern(s) seems to only match files with a leading dot, but hidden \
                    files are filtered by default. Consider adding -H/--hidden to search \
                    hidden files as well or adjust your search pattern(s)."
                .to_string(),
        );
    }
    let matcher = build_patterns(patterns, pattern_type, ignore_case)?;

    let extensions = if opts.extensions.is_empty() {
        None
    } else {
        let patterns = opts
            .extensions
            .iter()
            .map(|e| e.trim_start_matches('.'))
            .map(|e| format!(r".\.{}$", psh_pattern::escape(e)));
        Some(
            RegexSetBuilder::new(patterns)
                .case_insensitive(true)
                .build()
                .map_err(|e| e.to_string())?,
        )
    };

    let mut ignore_files = Vec::new();
    if read_global_ignore
        && let Some(f) = global_ignore_file(ctx)
        && ctx
            .stat(&f, true)
            .await
            .is_ok_and(|m| m.kind == FileKind::File)
    {
        ignore_files.push(f);
    }
    // `WalkBuilder::add_ignore` reads them now; malformed lines are skipped quietly.
    for f in &opts.ignore_file {
        match ctx.open(&against(f), OpenOptions::read()).await {
            Ok(_) => ignore_files.push(f.clone()),
            Err(e) => {
                let msg = format!(
                    "Malformed pattern in custom ignore file. {}: {}.",
                    String::from_utf8_lossy(f),
                    io_error_text(&e)
                );
                fail(ctx, &sanitize_error(&msg)).await;
            }
        }
    }
    let walk = WalkOptions {
        hidden: ignore_hidden,
        ignore: read_fdignore,
        parents: !opts.no_ignore_parent && (read_fdignore || read_vcsignore),
        git_ignore: read_vcsignore,
        git_global: read_vcsignore,
        git_exclude: read_vcsignore,
        require_git: !opts.no_require_git,
        custom_ignore_filenames: if read_fdignore {
            vec![".fdignore".to_string()]
        } else {
            Vec::new()
        },
        ignore_files,
        globs: opts
            .exclude
            .iter()
            .map(|p| (format!("!{p}"), false))
            .collect(),
        glob_root: Some(roots[0].clone()),
        max_depth: opts.max_depth(),
        follow_links: opts.follow,
        same_file_system: opts.one_file_system,
        // fd walks in parallel: `ignore`'s parallel walker's wording of errors.
        parallel_errors: true,
        ..WalkOptions::default()
    };
    let strip_cwd_prefix = opts.strip_cwd_prefix(!(opts.null_separator || has_command));
    let config = Config {
        roots,
        cwd: cwd.clone(),
        patterns: matcher,
        full_path_base,
        walk,
        min_depth: opts.min_depth(),
        prune: opts.prune,
        quiet: opts.quiet,
        max_results: opts.max_results(),
        file_types: (!opts.filetype.is_empty()).then(|| FileTypes::from_types(&opts.filetype)),
        extensions,
        size_constraints: std::mem::take(&mut opts.size),
        time_constraints,
        owner_constraint,
        ignore_contain: std::mem::take(&mut opts.ignore_contain),
        show_filesystem_errors: opts.show_errors,
        print: PrintConfig {
            format: opts.format.as_deref().map(fmt::FormatTemplate::parse),
            ls_colors,
            null_separator: opts.null_separator,
            path_separator: path_separator.clone(),
            actual_path_separator,
            interactive_terminal,
            strip_cwd_prefix,
        },
        results: has_command.then(|| Arc::new(Mutex::new(Vec::new()))),
    };
    let exec_opts = ExecOptions {
        path_separator,
        null_separator: opts.null_separator,
        buffer_output: opts.threads.is_none_or(|t| t > 1),
        batch_size: opts.batch_size,
        cwd: opts.base_directory.is_some().then_some(cwd),
    };
    Ok((config, command, exec_opts))
}

async fn is_dir(ctx: &UtilCtx<'_>, abs: &[u8]) -> bool {
    ctx.stat(abs, true)
        .await
        .is_ok_and(|m| m.kind == FileKind::Dir)
}

/// fd's `normalize_path`: absolute with `-a` (canonical, as `normpath` makes it), `./`
/// for `.`, `./-` for `-`.
async fn normalize_path(ctx: &UtilCtx<'_>, opts: &Opts, path: &[u8], cwd: &[u8]) -> Vec<u8> {
    if opts.absolute_path {
        let abs = if is_absolute(path) {
            path.to_vec()
        } else {
            join(cwd, path)
        };
        ctx.canonicalize(&abs).await.unwrap_or(abs)
    } else if path == b"." {
        // Change "." to "./" as a workaround for https://github.com/BurntSushi/ripgrep/pull/2711
        b"./".to_vec()
    } else if path == b"-" {
        // Prefix "-" so the underlying walker treats it as a path.
        b"./-".to_vec()
    } else {
        path.to_vec()
    }
}

/// fd's global ignore file: `fd/ignore` in etcetera's config directory.
fn global_ignore_file(ctx: &UtilCtx<'_>) -> Option<Vec<u8>> {
    let base = if cfg!(windows) {
        ctx.var(b"APPDATA").filter(|v| !v.is_empty())?.to_vec()
    } else {
        match ctx.var(b"XDG_CONFIG_HOME") {
            Some(x) if is_absolute(x) => x.to_vec(),
            _ => join(ctx.var(b"HOME").filter(|v| !v.is_empty())?, b".config"),
        }
    };
    Some(join(&join(&base, b"fd"), b"ignore"))
}

/// Detect if the user accidentally supplied a path instead of a search pattern.
async fn ensure_search_pattern_is_not_a_path(
    ctx: &UtilCtx<'_>,
    opts: &Opts,
    cwd: &[u8],
) -> Result<(), String> {
    if opts.full_path {
        return Ok(());
    }
    let Some(pattern) = &opts.pattern else {
        return Ok(());
    };
    for pattern in std::iter::once(pattern).chain(opts.exprs.iter()) {
        let mut should_warn = pattern.contains(&b'/');
        if cfg!(windows) && !should_warn && pattern.contains(&b'\\') {
            let abs = if is_absolute(pattern) {
                pattern.clone()
            } else {
                join(cwd, pattern)
            };
            should_warn = is_dir(ctx, &abs).await;
        }
        if should_warn {
            let pattern = String::from_utf8_lossy(pattern);
            return Err(format!(
                "The search pattern '{pattern}' contains a path-separation character and will \
                 not lead to any search results.\n\nIf you want to search for all files inside \
                 the '{pattern}' directory, use a match-all pattern:\n\n  fd . \
                 '{pattern}'\n\nInstead, if you want your pattern to match the full file path, \
                 use:\n\n  fd --full-path '{pattern}'"
            ));
        }
    }
    Ok(())
}
