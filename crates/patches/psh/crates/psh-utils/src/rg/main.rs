// Copied from ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22),
// crates/core/main.rs, with the psh patches marked `psh:` (D1133):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

/*!
The main entry point into ripgrep.
*/

// psh (D1133): `main` is `run_sync`, the body of a blocking utility (D1130): its
// statuses are returned, never `process::exit`; stdout and stderr are the utility's.
// The walk is psh's walker seam, sorted by name, on this thread; with more than one
// thread (native walks only), haystacks are searched on scoped search threads and
// their output is printed in walk order (ARCH "Sorted output"), so the output is the
// same as single-threaded.

use std::{collections::BTreeMap, io::Write, sync::mpsc};

use ignore::WalkState;
use psh_engine::util::SyncCtx;

use crate::rg::anyhow;
use crate::rg::flags::{self, HiArgs, SearchMode};
use crate::rg::haystack::Haystack;
use crate::rg::messages;
use crate::rg::search::{HaystackIo, SearchResult};
use crate::search::walk::Walker;

/// Then, as it was, then again it will be.
///
/// psh: the blocking body; `low` comes from the front end's parse (with the
/// messages that parse collected).
pub(crate) fn run_sync(
    ctx: &mut SyncCtx,
    (low, mut msgs): (flags::LowArgs, messages::State),
) -> i32 {
    let mut stderr = ctx.writer(2);
    let early = msgs.write_to(stderr.clone());
    let _ = stderr.write_all(&early);
    let (code, _) = messages::scoped(msgs, || {
        match run(ctx, low) {
            Ok(code) => code,
            Err(err) => {
                // Look for a broken pipe error. In this case, we generally want
                // to exit "gracefully" with a success exit code. This matches
                // existing Unix convention. We need to handle this explicitly
                // since the Rust runtime doesn't ask for PIPE signals, and thus
                // we get an I/O error instead. Traditional C Unix applications
                // quit by getting a PIPE signal that they don't handle, and thus
                // the unhandled signal causes the process to unceremoniously
                // terminate.
                if err.io_kind() == Some(std::io::ErrorKind::BrokenPipe) {
                    return 0;
                }
                eprintln_locked!("{:#}", err);
                2
            }
        }
    });
    code
}

/// The main entry point for ripgrep.
///
/// The given parse result determines ripgrep's behavior. The parse
/// result should be the result of parsing CLI arguments in a low level
/// representation, and then followed by an attempt to convert them into a
/// higher level representation. The higher level representation has some nicer
/// abstractions, for example, instead of representing the `-g/--glob` flag
/// as a `Vec<String>` (as in the low level representation), the globs are
/// converted into a single matcher.
fn run(ctx: &mut SyncCtx, low: flags::LowArgs) -> anyhow::Result<i32> {
    use crate::rg::flags::Mode;

    let args = HiArgs::from_low_args(low, ctx)?;
    let matched = match args.mode() {
        Mode::Search(_) if !args.matches_possible() => false,
        Mode::Search(mode) if args.threads() == 1 => search(ctx, &args, mode)?,
        Mode::Search(mode) => search_parallel(ctx, &args, mode)?,
        Mode::Files => files(ctx, &args)?,
        Mode::Types => return types(ctx, &args),
        Mode::Generate(mode) => return generate(ctx, mode),
    };
    Ok(if matched && (args.quiet() || !messages::errored()) {
        0
    } else if messages::errored() {
        2
    } else {
        1
    })
}

/// psh: the haystacks of `args`' paths in order, walked with the walker seam (stdin
/// for `-`), given to `visit` until it says `Quit`. `visit` gets the context for
/// reading: shared during a walk, exclusive for stdin.
fn walk(
    ctx: &mut SyncCtx,
    args: &HiArgs,
    visit: &mut dyn FnMut(Haystack, HaystackIo<'_>) -> WalkState,
) -> anyhow::Result<()> {
    let haystack_builder = args.haystack_builder();
    let paths = args.paths();
    let mut i = 0;
    while i < paths.len() {
        if paths[i] == b"-" {
            let mut stdin = ctx.stdin();
            if visit(haystack_builder.stdin(), HaystackIo::Stdin(&mut stdin))
                == WalkState::Quit
            {
                return Ok(());
            }
            i += 1;
            continue;
        }
        let end = (i..paths.len())
            .find(|&j| paths[j] == b"-")
            .unwrap_or(paths.len());
        let host: &SyncCtx = ctx;
        let walker = Walker::new(host, args.walk_options().clone())?;
        let state =
            walker.run(&paths[i..end], &mut |result| match haystack_builder
                .build_from_result(host, result)
            {
                Some(haystack) => visit(haystack, HaystackIo::Fs(host)),
                None => WalkState::Continue,
            });
        if state == WalkState::Quit {
            return Ok(());
        }
        i = end;
    }
    Ok(())
}

/// psh: the haystacks to search in order: as walked, or collected and sorted when a
/// sort other than ascending path is asked for (`HiArgs::sort`).
fn haystacks(
    ctx: &mut SyncCtx,
    args: &HiArgs,
    visit: &mut dyn FnMut(Haystack, HaystackIo<'_>) -> WalkState,
) -> anyhow::Result<()> {
    if !args.sorts_after_walk() {
        return walk(ctx, args, visit);
    }
    let mut all = Vec::new();
    walk(ctx, args, &mut |h, _| {
        all.push(h);
        WalkState::Continue
    })?;
    let host: &SyncCtx = ctx;
    let sorted: Vec<Haystack> = args.sort(host, all.into_iter()).collect();
    for h in sorted {
        let state = if h.is_stdin() {
            let mut stdin = ctx.stdin();
            visit(h, HaystackIo::Stdin(&mut stdin))
        } else {
            visit(h, HaystackIo::Fs(ctx))
        };
        if state == WalkState::Quit {
            break;
        }
    }
    Ok(())
}

/// The top-level entry point for single-threaded search.
///
/// This recursively steps through the file list (current directory by default)
/// and searches each file sequentially.
fn search(
    ctx: &mut SyncCtx,
    args: &HiArgs,
    mode: SearchMode,
) -> anyhow::Result<bool> {
    let started_at = Started::now();
    let mut matched = false;
    let mut searched = false;
    let mut stats = args.stats();
    let mut searcher = args.search_worker(
        args.matcher()?,
        args.searcher()?,
        args.printer(mode, args.stdout(ctx.writer(1))),
    )?;
    haystacks(ctx, args, &mut |haystack, io| {
        searched = true;
        let search_result = match searcher.search(&haystack, io) {
            Ok(search_result) => search_result,
            // A broken pipe means graceful termination.
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                return WalkState::Quit;
            }
            Err(err) => {
                err_message!("{}: {}", haystack.path().display(), err);
                return WalkState::Continue;
            }
        };
        matched = matched || search_result.has_match();
        if let Some(ref mut stats) = stats {
            *stats += search_result.stats().unwrap();
        }
        if matched && args.quit_after_match() {
            return WalkState::Quit;
        }
        WalkState::Continue
    })?;
    if args.has_implicit_path() && !searched {
        eprint_nothing_searched();
    }
    if let Some(ref stats) = stats {
        let wtr = searcher.printer().get_mut();
        let _ = print_stats(mode, stats, started_at, wtr);
    }
    let _ = searcher.printer().get_mut().flush();
    Ok(matched)
}

/// The top-level entry point for multi-threaded search.
///
/// psh: the walk stays on this thread; each haystack goes to one of the search
/// threads, which print into a buffer of their own; the buffers are written to
/// stdout in walk order, at most `4 * threads` haystacks ahead (stdin is searched
/// here, in its turn). Requesting a sorted output (`--sort`) uses one thread.
fn search_parallel(
    ctx: &mut SyncCtx,
    args: &HiArgs,
    mode: SearchMode,
) -> anyhow::Result<bool> {
    use std::sync::{Arc, Mutex};

    let started_at = Started::now();
    let mut stats = args.stats();
    let mut matched = false;
    let mut searched = false;
    let mut out = args.stdout(ctx.writer(1));
    let worker = args.search_worker(
        args.matcher()?,
        args.searcher()?,
        args.printer(mode, args.buffer()),
    )?;
    let threads = args.threads();
    let (job_tx, job_rx) = mpsc::sync_channel::<(usize, Haystack)>(threads);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (done_tx, done_rx) = mpsc::channel::<Done>();

    std::thread::scope(|scope| -> anyhow::Result<()> {
        for _ in 0..threads {
            let job_rx = Arc::clone(&job_rx);
            let done_tx = done_tx.clone();
            let mut searcher = worker.clone();
            scope.spawn(move || {
                loop {
                    let job = job_rx
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .recv();
                    let Ok((seq, haystack)) = job else { break };
                    searcher.printer().get_mut().clear();
                    let result = searcher
                        .search(&haystack, HaystackIo::None)
                        .map(|r| {
                            (
                                searcher
                                    .printer()
                                    .get_mut()
                                    .as_slice()
                                    .to_vec(),
                                r,
                            )
                        });
                    if done_tx.send(Done { seq, haystack, result }).is_err() {
                        break;
                    }
                }
            });
        }
        drop(done_tx);
        let mut merge = Merge {
            separator: args.file_separator().map(<[u8]>::to_vec),
            printed: false,
            next: 0,
            assigned: 0,
            pending: BTreeMap::new(),
            quit: false,
            given: 0,
            held: 0,
        };
        let mut local = worker.clone();
        let mut on_done = |merge: &mut Merge,
                           out: &mut crate::rg::cli::StandardStream,
                           d: Done| {
            searched = true;
            match d.result {
                Err(err) => {
                    err_message!("{}: {}", d.haystack.path().display(), err);
                }
                Ok((buf, result)) => {
                    if result.has_match() {
                        matched = true;
                    }
                    if let Some(ref mut stats) = stats {
                        *stats += result.stats().unwrap();
                    }
                    if let Err(err) = merge.print(out, &buf) {
                        // A broken pipe means graceful termination.
                        if err.kind() == std::io::ErrorKind::BrokenPipe {
                            merge.quit = true;
                        } else {
                            // Otherwise, we continue on our merry way.
                            err_message!(
                                "{}: {}",
                                d.haystack.path().display(),
                                err
                            );
                        }
                    }
                }
            }
            if matched && args.quit_after_match() {
                merge.quit = true;
            }
        };
        let mut job_tx = Some(job_tx);
        let r = haystacks(ctx, args, &mut |haystack, io| {
            let seq = merge.assigned;
            merge.assigned += 1;
            if let HaystackIo::Stdin(_) = io {
                // In its turn: everything before it is printed first.
                while merge.next < seq && !merge.quit {
                    let Ok(d) = done_rx.recv() else { break };
                    merge.pending.insert(d.seq, d);
                    while let Some(d) = merge.pending.remove(&merge.next) {
                        merge.next += 1;
                        on_done(&mut merge, &mut out, d);
                    }
                }
                local.printer().get_mut().clear();
                let result = local.search(&haystack, io).map(|r| {
                    (local.printer().get_mut().as_slice().to_vec(), r)
                });
                merge.next += 1;
                on_done(&mut merge, &mut out, Done { seq, haystack, result });
            } else if let Some(tx) = &job_tx {
                if tx.send((seq, haystack)).is_err() {
                    return WalkState::Quit;
                }
            }
            // Print what is done, in order; wait while too far ahead.
            loop {
                while let Ok(d) = done_rx.try_recv() {
                    merge.pending.insert(d.seq, d);
                }
                while let Some(d) = merge.pending.remove(&merge.next) {
                    merge.next += 1;
                    on_done(&mut merge, &mut out, d);
                }
                if merge.quit {
                    return WalkState::Quit;
                }
                if merge.assigned - merge.next <= 4 * threads {
                    return WalkState::Continue;
                }
                match done_rx.recv() {
                    Ok(d) => {
                        merge.pending.insert(d.seq, d);
                    }
                    Err(_) => return WalkState::Quit,
                }
            }
        });
        // No more jobs: the threads end when the queue is empty.
        job_tx.take();
        if !merge.quit {
            for d in done_rx.iter() {
                merge.pending.insert(d.seq, d);
                while let Some(d) = merge.pending.remove(&merge.next) {
                    merge.next += 1;
                    on_done(&mut merge, &mut out, d);
                }
                if merge.quit {
                    break;
                }
            }
        }
        // Dropping the receiver (when the scope ends) stops threads still searching.
        drop(done_rx);
        r
    })?;
    if args.has_implicit_path() && !searched {
        eprint_nothing_searched();
    }
    if let Some(ref stats) = stats {
        let _ = print_stats(mode, stats, started_at, &mut out);
    }
    let _ = out.flush();
    Ok(matched)
}

/// psh: when the search started, for `--stats`: wasm32-unknown-unknown has no clock
/// (`Instant::now` panics there), so the time is zero there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Started(Option<std::time::Instant>);

impl Started {
    pub(crate) fn now() -> Started {
        if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
            Started(None)
        } else {
            Started(Some(std::time::Instant::now()))
        }
    }

    pub(crate) fn elapsed(&self) -> std::time::Duration {
        self.0.map_or(std::time::Duration::ZERO, |t| t.elapsed())
    }
}

/// psh: one haystack searched on a search thread: its output and result.
struct Done {
    seq: usize,
    haystack: Haystack,
    result: std::io::Result<(Vec<u8>, SearchResult)>,
}

/// psh: the in-order merge of search threads' output.
struct Merge {
    /// What is printed between two haystacks' non-empty outputs.
    separator: Option<Vec<u8>>,
    printed: bool,
    /// The sequence number to print next.
    next: usize,
    /// The sequence number the next haystack gets.
    assigned: usize,
    pending: BTreeMap<usize, Done>,
    quit: bool,
    /// Bytes of output the reader has been given, and bytes written since.
    given: usize,
    held: usize,
}

impl Merge {
    /// Prints one haystack's output, after the separator when something was printed.
    fn print(
        &mut self,
        out: &mut impl Write,
        buf: &[u8],
    ) -> std::io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        if let Some(ref sep) = self.separator {
            if self.printed {
                out.write_all(sep)?;
                out.write_all(b"\n")?;
            }
        }
        out.write_all(buf)?;
        self.printed = true;
        // The reader gets the first file's output at once and then each time as much
        // again as it has been given: one that is gone (`| head`) is seen before the
        // search is over, and one that reads it all is woken a few times, not per file.
        self.held += buf.len();
        if self.held >= self.given {
            out.flush()?;
            self.given += self.held;
            self.held = 0;
        }
        Ok(())
    }
}

/// The top-level entry point for file listing without searching.
///
/// This recursively steps through the file list (current directory by default)
/// and prints each path sequentially using a single thread.
///
/// psh: also with more threads (listing is the walk itself).
fn files(ctx: &mut SyncCtx, args: &HiArgs) -> anyhow::Result<bool> {
    let mut matched = false;
    let mut path_printer =
        args.path_printer_builder().build(args.stdout(ctx.writer(1)));
    let mut error = None;
    haystacks(ctx, args, &mut |haystack, _| {
        matched = true;
        if args.quit_after_match() {
            return WalkState::Quit;
        }
        if let Err(err) = path_printer.write(haystack.path()) {
            // A broken pipe means graceful termination.
            if err.kind() != std::io::ErrorKind::BrokenPipe {
                // Otherwise, we have some other error that's preventing us from
                // writing to stdout, so we should bubble it up.
                error = Some(err);
            }
            return WalkState::Quit;
        }
        WalkState::Continue
    })?;
    if let Some(err) = error {
        return Err(err.into());
    }
    // The stream flushes when it is dropped (as ripgrep's stdout).
    drop(path_printer);
    Ok(matched)
}

/// The top-level entry point for `--type-list`.
fn types(ctx: &mut SyncCtx, args: &HiArgs) -> anyhow::Result<i32> {
    let mut count = 0;
    let mut stdout = args.stdout(ctx.writer(1));
    for def in args.types().definitions() {
        count += 1;
        stdout.write_all(def.name().as_bytes())?;
        stdout.write_all(b": ")?;

        let mut first = true;
        for glob in def.globs() {
            if !first {
                stdout.write_all(b", ")?;
            }
            stdout.write_all(glob.as_bytes())?;
            first = false;
        }
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;
    Ok(if count == 0 { 1 } else { 0 })
}

/// Implements ripgrep's "generate" modes.
///
/// These modes correspond to generating some kind of ancillary data related
/// to ripgrep. At present, this includes ripgrep's man page (in roff format)
/// and supported shell completions.
fn generate(
    ctx: &mut SyncCtx,
    mode: crate::rg::flags::GenerateMode,
) -> anyhow::Result<i32> {
    use crate::rg::flags::GenerateMode;

    let output = match mode {
        GenerateMode::Man => flags::generate_man_page(),
        GenerateMode::CompleteBash => flags::generate_complete_bash(),
        GenerateMode::CompleteZsh => flags::generate_complete_zsh(),
        GenerateMode::CompleteFish => flags::generate_complete_fish(),
        GenerateMode::CompletePowerShell => {
            flags::generate_complete_powershell()
        }
    };
    writeln!(ctx.writer(1), "{}", output.trim_end())?;
    Ok(0)
}

/// Implements ripgrep's "special" modes.
///
/// A special mode is one that generally short-circuits most (not all) of
/// ripgrep's initialization logic and skips right to this routine. The
/// special modes essentially consist of printing help and version output. The
/// idea behind the short circuiting is to ensure there is as little as possible
/// (within reason) that would prevent ripgrep from emitting help output.
///
/// For example, part of the initialization logic that is skipped (among
/// other things) is accessing the current working directory. If that fails,
/// ripgrep emits an error. We don't want to emit an error if it fails and
/// the user requested version or help information.
///
/// psh: the text to print and the status (the front end prints it).
pub(crate) fn special(mode: crate::rg::flags::SpecialMode) -> (String, i32) {
    use crate::rg::flags::SpecialMode;

    let mut exit = 0;
    let output = match mode {
        SpecialMode::HelpShort => flags::generate_help_short(),
        SpecialMode::HelpLong => flags::generate_help_long(),
        SpecialMode::VersionShort => flags::generate_version_short(),
        SpecialMode::VersionLong => flags::generate_version_long(),
        // --pcre2-version is a little special because it emits an error
        // exit code if this build of ripgrep doesn't support PCRE2.
        SpecialMode::VersionPCRE2 => {
            let (output, available) = flags::generate_version_pcre2();
            if !available {
                exit = 1;
            }
            output
        }
    };
    (format!("{}\n", output.trim_end()), exit)
}

/// Prints a heuristic error messages when nothing is searched.
///
/// This can happen if an applicable ignore file has one or more rules that
/// are too broad and cause ripgrep to ignore everything.
///
/// We only show this error message when the user does *not* provide an
/// explicit path to search. This is because the message can otherwise be
/// noisy, e.g., when it is intended that there is nothing to search.
fn eprint_nothing_searched() {
    err_message!(
        "No files were searched, which means ripgrep probably \
         applied a filter you didn't expect.\n\
         Running with --debug will show why files are being skipped."
    );
}

/// Prints the statistics given to the writer given.
///
/// The search mode given determines whether the stats should be printed in
/// a plain text format or in a JSON format.
///
/// The `started` time should be the time at which ripgrep started working.
///
/// If an error occurs while writing, then writing stops and the error is
/// returned. Note that callers should probably ignore this error, since
/// whether stats fail to print or not generally shouldn't cause ripgrep to
/// enter into an "error" state. And usually the only way for this to fail is
/// if writing to stdout itself fails.
fn print_stats<W: Write>(
    mode: SearchMode,
    stats: &crate::printer::Stats,
    started: Started,
    mut wtr: W,
) -> std::io::Result<()> {
    let elapsed = started.elapsed();
    if matches!(mode, SearchMode::JSON) {
        // We specifically match the format laid out by the JSON printer in
        // the grep-printer crate. We simply "extend" it with the 'summary'
        // message type.
        // psh: built and written by crate::rg::json (D029).
        crate::rg::json::summary(stats, elapsed).write_line(&mut wtr)
    } else {
        write!(
            wtr,
            "
{matches} matches
{lines} matched lines
{searches_with_match} files contained matches
{searches} files searched
{bytes_printed} bytes printed
{bytes_searched} bytes searched
{search_time:0.6} seconds spent searching
{process_time:0.6} seconds total
",
            matches = stats.matches(),
            lines = stats.matched_lines(),
            searches_with_match = stats.searches_with_match(),
            searches = stats.searches(),
            bytes_printed = stats.bytes_printed(),
            bytes_searched = stats.bytes_searched(),
            search_time = stats.elapsed().as_secs_f64(),
            process_time = elapsed.as_secs_f64(),
        )
    }
}
