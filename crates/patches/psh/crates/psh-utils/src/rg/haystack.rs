// Copied from ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22),
// crates/core/haystack.rs, with the psh patches marked `psh:` (D1133):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

/*!
Defines a builder for haystacks.

A "haystack" represents something we want to search. It encapsulates the logic
for whether a haystack ought to be searched or not, separate from the standard
ignore rules and other filtering logic.

Effectively, a haystack wraps a directory entry and adds some light application
level logic around it.
*/

// psh: haystacks are entries of psh's walker seam (`search::walk::Entry`, D1132)
// instead of `ignore::DirEntry`, and stdin is a haystack of its own (the walker has no
// stdin entry). Whether a root that is a link names a directory is asked of the
// utility's file system.

use std::path::{Path, PathBuf};

use crate::search::walk::{Entry, WalkHost, to_path};
use psh_engine::host::FileKind;

/// A builder for constructing things to search over.
#[derive(Clone, Debug)]
pub(crate) struct HaystackBuilder {
    strip_dot_prefix: bool,
}

impl HaystackBuilder {
    /// Return a new haystack builder with a default configuration.
    pub(crate) fn new() -> HaystackBuilder {
        HaystackBuilder { strip_dot_prefix: false }
    }

    /// Create a new haystack from a possibly missing directory entry.
    ///
    /// If the directory entry isn't present, then the corresponding error is
    /// logged if messages have been configured. Otherwise, if the directory
    /// entry is deemed searchable, then it is returned as a haystack.
    pub(crate) fn build_from_result(
        &self,
        host: &dyn WalkHost,
        result: Result<Entry, crate::search::ignore::Error>,
    ) -> Option<Haystack> {
        match result {
            Ok(dent) => self.build(host, dent),
            Err(err) => {
                err_message!("{err}");
                None
            }
        }
    }

    /// Create a new haystack using this builder's configuration.
    ///
    /// If a directory entry could not be created or should otherwise not be
    /// searched, then this returns `None` after emitting any relevant log
    /// messages.
    fn build(&self, host: &dyn WalkHost, dent: Entry) -> Option<Haystack> {
        let display = to_path(&dent.path);
        // If this is a symlink, then we want to follow it to determine
        // whether it's a directory or not.
        let is_dir = dent.is_dir()
            || (dent.path_is_symlink
                && host
                    .stat(&dent.abs, true)
                    .is_ok_and(|m| m.kind == FileKind::Dir));
        let hay = Haystack {
            display: Self::strip(self.strip_dot_prefix, display),
            dent: Some(dent),
            is_dir,
        };
        if let Some(err) = hay.dent.as_ref().and_then(|d| d.err.as_ref()) {
            ignore_message!("{err}");
        }
        // If this entry was explicitly provided by an end user, then we always
        // want to search it.
        if hay.is_explicit() {
            return Some(hay);
        }
        // At this point, we only want to search something if it's explicitly a
        // file. This omits symlinks. (If ripgrep was configured to follow
        // symlinks, then they have already been followed by the directory
        // traversal.)
        if hay.is_file() {
            return Some(hay);
        }
        // We got nothing. Emit a debug message, but only if this isn't a
        // directory. Otherwise, emitting messages for directories is just
        // noisy.
        if !hay.is_dir() {
            debug!(
                "ignoring {}: failed to pass haystack filter: \
                 file type: {:?}",
                hay.display.display(),
                hay.dent.as_ref().map(|d| d.kind),
            );
        }
        None
    }

    /// The haystack for stdin.
    pub(crate) fn stdin(&self) -> Haystack {
        Haystack {
            display: PathBuf::from("<stdin>"),
            dent: None,
            is_dir: false,
        }
    }

    fn strip(yes: bool, p: PathBuf) -> PathBuf {
        if yes && p.starts_with("./") {
            p.strip_prefix("./").unwrap().to_path_buf()
        } else {
            p
        }
    }

    /// When enabled, if the haystack's file path starts with `./` then it is
    /// stripped.
    ///
    /// This is useful when implicitly searching the current working directory.
    pub(crate) fn strip_dot_prefix(
        &mut self,
        yes: bool,
    ) -> &mut HaystackBuilder {
        self.strip_dot_prefix = yes;
        self
    }
}

/// A haystack is a thing we want to search.
///
/// Generally, a haystack is either a file or stdin.
#[derive(Debug)]
pub(crate) struct Haystack {
    /// The path to show: the entry's, `./` stripped when asked, or `<stdin>`.
    display: PathBuf,
    /// `None` for stdin.
    dent: Option<Entry>,
    is_dir: bool,
}

impl Haystack {
    /// Return the file path corresponding to this haystack.
    ///
    /// If this haystack corresponds to stdin, then a special `<stdin>` path
    /// is returned instead.
    pub(crate) fn path(&self) -> &Path {
        &self.display
    }

    /// The walk's entry (`None` for stdin).
    pub(crate) fn entry(&self) -> Option<&Entry> {
        self.dent.as_ref()
    }

    /// Returns true if and only if this entry corresponds to stdin.
    pub(crate) fn is_stdin(&self) -> bool {
        self.dent.is_none()
    }

    /// Returns true if and only if this entry corresponds to a haystack to
    /// search that was explicitly supplied by an end user.
    ///
    /// Generally, this corresponds to either stdin or an explicit file path
    /// argument. e.g., in `rg foo some-file ./some-dir/`, `some-file` is
    /// an explicit haystack, but, e.g., `./some-dir/some-other-file` is not.
    ///
    /// However, note that ripgrep does not see through shell globbing. e.g.,
    /// in `rg foo ./some-dir/*`, `./some-dir/some-other-file` will be treated
    /// as an explicit haystack.
    pub(crate) fn is_explicit(&self) -> bool {
        // stdin is obvious. When an entry has a depth of 0, that means it
        // was explicitly provided to our directory iterator, which means it
        // was in turn explicitly provided by the end user. The !is_dir check
        // means that we want to search files even if their symlinks, again,
        // because they were explicitly provided. (And we never want to try
        // to search a directory.)
        match &self.dent {
            None => true,
            Some(d) => d.depth == 0 && !self.is_dir(),
        }
    }

    /// Returns true if and only if this haystack points to a directory after
    /// following symbolic links.
    fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns true if and only if this haystack points to a file.
    fn is_file(&self) -> bool {
        self.dent.as_ref().is_some_and(|d| d.is_file())
    }
}
