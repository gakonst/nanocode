// Copied from ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22),
// crates/core/search.rs, with the psh patches marked `psh:` (D1133):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

/*!
Defines a very high level "search worker" abstraction.

A search worker manages the high level interaction points between the matcher
(i.e., which regex engine is used), the searcher (i.e., how data is actually
read and matched using the regex engine) and the printer. For example, the
search worker is where things like preprocessors or decompression happens.
*/

// psh: `--pre` and `-z/--search-zip` run the real rg (D1082), so the preprocessor and
// decompression paths are gone; PCRE2 likewise. A haystack is searched natively when
// the walk found it on the process's file system, else through the utility's file
// system (`SyncCtx::open`), and stdin is the utility's (D1133). Memory maps are off
// (`MmapChoice::auto` is unsafe).

use std::{io, path::Path};

use psh_engine::util::SyncCtx;

use {grep_matcher::Matcher, termcolor::WriteColor};

/// The configuration for the search worker.
///
/// Among a few other things, the configuration primarily controls the way we
/// show search results to users at a very high level.
#[derive(Clone, Debug)]
struct Config {
    binary_implicit: grep_searcher::BinaryDetection,
    binary_explicit: grep_searcher::BinaryDetection,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            binary_implicit: grep_searcher::BinaryDetection::none(),
            binary_explicit: grep_searcher::BinaryDetection::none(),
        }
    }
}

/// A builder for configuring and constructing a search worker.
#[derive(Clone, Debug)]
pub(crate) struct SearchWorkerBuilder {
    config: Config,
}

impl Default for SearchWorkerBuilder {
    fn default() -> SearchWorkerBuilder {
        SearchWorkerBuilder::new()
    }
}

impl SearchWorkerBuilder {
    /// Create a new builder for configuring and constructing a search worker.
    pub(crate) fn new() -> SearchWorkerBuilder {
        SearchWorkerBuilder { config: Config::default() }
    }

    /// Create a new search worker using the given searcher, matcher and
    /// printer.
    pub(crate) fn build<W: WriteColor>(
        &self,
        matcher: PatternMatcher,
        searcher: grep_searcher::Searcher,
        printer: Printer<W>,
    ) -> SearchWorker<W> {
        let config = self.config.clone();
        SearchWorker { config, matcher, searcher, printer }
    }

    /// Set the binary detection that should be used when searching files
    /// found via a recursive directory search.
    ///
    /// Generally, this binary detection may be
    /// `grep_searcher::BinaryDetection::quit` if we want to skip binary files
    /// completely.
    ///
    /// By default, no binary detection is performed.
    pub(crate) fn binary_detection_implicit(
        &mut self,
        detection: grep_searcher::BinaryDetection,
    ) -> &mut SearchWorkerBuilder {
        self.config.binary_implicit = detection;
        self
    }

    /// Set the binary detection that should be used when searching files
    /// explicitly supplied by an end user.
    ///
    /// Generally, this binary detection should NOT be
    /// `grep_searcher::BinaryDetection::quit`, since we never want to
    /// automatically filter files supplied by the end user.
    ///
    /// By default, no binary detection is performed.
    pub(crate) fn binary_detection_explicit(
        &mut self,
        detection: grep_searcher::BinaryDetection,
    ) -> &mut SearchWorkerBuilder {
        self.config.binary_explicit = detection;
        self
    }
}

/// The result of executing a search.
///
/// Generally speaking, the "result" of a search is sent to a printer, which
/// writes results to an underlying writer such as stdout or a file. However,
/// every search also has some aggregate statistics or meta data that may be
/// useful to higher level routines.
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchResult {
    has_match: bool,
    stats: Option<grep_printer::Stats>,
}

impl SearchResult {
    /// Whether the search found a match or not.
    pub(crate) fn has_match(&self) -> bool {
        self.has_match
    }

    /// Return aggregate search statistics for a single search, if available.
    ///
    /// It can be expensive to compute statistics, so these are only present
    /// if explicitly enabled in the printer provided by the caller.
    pub(crate) fn stats(&self) -> Option<&grep_printer::Stats> {
        self.stats.as_ref()
    }
}

/// The pattern matcher used by a search worker.
#[derive(Clone, Debug)]
pub(crate) enum PatternMatcher {
    RustRegex(grep_regex::RegexMatcher),
}

/// The printer used by a search worker.
///
/// The `W` type parameter refers to the type of the underlying writer.
#[derive(Clone, Debug)]
pub(crate) enum Printer<W> {
    /// Use the standard printer, which supports the classic grep-like format.
    Standard(grep_printer::Standard<W>),
    /// Use the summary printer, which supports aggregate displays of search
    /// results.
    Summary(grep_printer::Summary<W>),
    /// A JSON printer, which emits results in the JSON Lines format.
    ///
    /// psh: crate::rg::json's, since grep-printer's needs serde.
    JSON(crate::rg::json::JSON<W>),
}

impl<W: WriteColor> Printer<W> {
    /// Return a mutable reference to the underlying printer's writer.
    pub(crate) fn get_mut(&mut self) -> &mut W {
        match *self {
            Printer::Standard(ref mut p) => p.get_mut(),
            Printer::Summary(ref mut p) => p.get_mut(),
            Printer::JSON(ref mut p) => p.get_mut(),
        }
    }
}

/// A worker for executing searches.
///
/// It is intended for a single worker to execute many searches, and is
/// generally intended to be used from a single thread. When searching using
/// multiple threads, it is better to create a new worker for each thread.
#[derive(Clone, Debug)]
pub(crate) struct SearchWorker<W> {
    config: Config,
    matcher: PatternMatcher,
    searcher: grep_searcher::Searcher,
    printer: Printer<W>,
}

impl<W: WriteColor> SearchWorker<W> {
    /// Execute a search over the given haystack.
    ///
    /// psh: `io` is how to read it: stdin for the stdin haystack, the utility's file
    /// system for one the walk did not find natively (`None` on search threads, which
    /// only get native haystacks).
    pub(crate) fn search(
        &mut self,
        haystack: &crate::rg::haystack::Haystack,
        io: HaystackIo<'_>,
    ) -> io::Result<SearchResult> {
        let bin = if haystack.is_explicit() {
            self.config.binary_explicit.clone()
        } else {
            self.config.binary_implicit.clone()
        };
        let path = haystack.path();
        trace!("{}: binary detection: {:?}", path.display(), bin);

        self.searcher.set_binary_detection(bin);
        match (haystack.entry(), io) {
            (None, HaystackIo::Stdin(rdr)) => self.search_reader(path, rdr),
            (Some(e), _) if e.native.is_some() => {
                let native = e.native.clone().unwrap();
                self.search_path(path, &native)
            }
            (Some(e), HaystackIo::Fs(ctx)) => {
                let mut rdr = ctx.open(&e.abs)?;
                self.search_reader(path, &mut rdr)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot read this haystack here",
            )),
        }
    }

    /// Return a mutable reference to the underlying printer.
    pub(crate) fn printer(&mut self) -> &mut Printer<W> {
        &mut self.printer
    }

    /// Search the contents of the given file path.
    fn search_path(
        &mut self,
        path: &Path,
        file: &Path,
    ) -> io::Result<SearchResult> {
        use self::PatternMatcher::*;

        let (searcher, printer) = (&mut self.searcher, &mut self.printer);
        match self.matcher {
            RustRegex(ref m) => search_path(m, searcher, printer, path, file),
        }
    }

    /// Executes a search on the given reader, which may or may not correspond
    /// directly to the contents of the given file path. Instead, the reader
    /// may actually cause something else to be searched (for example, when
    /// a preprocessor is set or when decompression is enabled). In those
    /// cases, the file path is used for visual purposes only.
    ///
    /// Generally speaking, this method should only be used when there is no
    /// other choice. Searching via `search_path` provides more opportunities
    /// for optimizations (such as memory maps).
    fn search_reader<R: io::Read + ?Sized>(
        &mut self,
        path: &Path,
        rdr: &mut R,
    ) -> io::Result<SearchResult> {
        use self::PatternMatcher::*;

        let (searcher, printer) = (&mut self.searcher, &mut self.printer);
        match self.matcher {
            RustRegex(ref m) => search_reader(m, searcher, printer, path, rdr),
        }
    }
}

/// psh: how a haystack is read (see [`SearchWorker::search`]).
pub(crate) enum HaystackIo<'a> {
    Stdin(&'a mut dyn io::Read),
    Fs(&'a SyncCtx),
    None,
}

/// Search the contents of the given file path using the given matcher,
/// searcher and printer.
fn search_path<M: Matcher, W: WriteColor>(
    matcher: M,
    searcher: &mut grep_searcher::Searcher,
    printer: &mut Printer<W>,
    path: &Path,
    file: &Path,
) -> io::Result<SearchResult> {
    match *printer {
        Printer::Standard(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_path(&matcher, file, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: sink.stats().cloned(),
            })
        }
        Printer::Summary(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_path(&matcher, file, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: sink.stats().cloned(),
            })
        }
        Printer::JSON(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_path(&matcher, file, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: Some(sink.stats().clone()),
            })
        }
    }
}

/// Search the contents of the given reader using the given matcher, searcher
/// and printer.
fn search_reader<M: Matcher, R: io::Read + ?Sized, W: WriteColor>(
    matcher: M,
    searcher: &mut grep_searcher::Searcher,
    printer: &mut Printer<W>,
    path: &Path,
    rdr: &mut R,
) -> io::Result<SearchResult> {
    match *printer {
        Printer::Standard(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_reader(&matcher, &mut *rdr, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: sink.stats().cloned(),
            })
        }
        Printer::Summary(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_reader(&matcher, &mut *rdr, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: sink.stats().cloned(),
            })
        }
        Printer::JSON(ref mut p) => {
            let mut sink = p.sink_with_path(&matcher, path);
            searcher.search_reader(&matcher, &mut *rdr, &mut sink)?;
            Ok(SearchResult {
                has_match: sink.has_match(),
                stats: Some(sink.stats().clone()),
            })
        }
    }
}
