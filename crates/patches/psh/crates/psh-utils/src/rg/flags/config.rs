// Copied from ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22),
// crates/core/flags/config.rs, with the psh patches marked `psh:` (D1133):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

/*!
This module provides routines for reading ripgrep config "rc" files.

The primary output of these routines is a sequence of arguments, where each
argument corresponds precisely to one shell argument.
*/

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::rg::anyhow;

/// psh: `RIPGREP_CONFIG_PATH` as the utility's environment names it, and the file's
/// contents as read through the utility's file system (D1133).
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) contents: std::io::Result<Vec<u8>>,
}

/// Return a sequence of arguments derived from ripgrep rc configuration files.
///
/// psh: `None` when `RIPGREP_CONFIG_PATH` is unset or empty.
pub fn args(source: Option<Source>) -> Vec<OsString> {
    let Some(source) = source else {
        debug!(
            "RIPGREP_CONFIG_PATH environment variable is not set, \
             therefore not reading any config file"
        );
        return vec![];
    };
    let config_path = source.path;
    let (args, errs) = match parse(&config_path, source.contents) {
        Ok((args, errs)) => (args, errs),
        Err(err) => {
            message!(
                "failed to read the file specified in RIPGREP_CONFIG_PATH: {}",
                err
            );
            return vec![];
        }
    };
    if !errs.is_empty() {
        for err in errs {
            message!("{}:{}", config_path.display(), err);
        }
    }
    debug!(
        "{}: arguments loaded from config file: {:?}",
        config_path.display(),
        args
    );
    args
}

/// Parse a single ripgrep rc file from the given path.
///
/// On success, this returns a set of shell arguments, in order, that should
/// be pre-pended to the arguments given to ripgrep at the command line.
///
/// If the file could not be read, then an error is returned. If there was
/// a problem parsing one or more lines in the file, then errors are returned
/// for each line in addition to successfully parsed arguments.
fn parse(
    path: &Path,
    contents: std::io::Result<Vec<u8>>,
) -> anyhow::Result<(Vec<OsString>, Vec<anyhow::Error>)> {
    match contents {
        Ok(file) => parse_reader(&file[..]),
        Err(err) => anyhow::bail!("{}: {}", path.display(), err),
    }
}

/// Parse a single ripgrep rc file from the given reader.
///
/// Callers should not provided a buffered reader, as this routine will use its
/// own buffer internally.
///
/// On success, this returns a set of shell arguments, in order, that should
/// be pre-pended to the arguments given to ripgrep at the command line.
///
/// If the reader could not be read, then an error is returned. If there was a
/// problem parsing one or more lines, then errors are returned for each line
/// in addition to successfully parsed arguments.
fn parse_reader<R: std::io::Read>(
    rdr: R,
) -> anyhow::Result<(Vec<OsString>, Vec<anyhow::Error>)> {
    let bufrdr = std::io::BufReader::new(rdr);
    let (mut args, mut errs) = (vec![], vec![]);
    let mut line_number = 0;
    crate::rg::bytes::for_each_line(bufrdr, |line| {
        line_number += 1;

        let line = crate::rg::bytes::trim_whitespace(line);
        if line.is_empty() || line[0] == b'#' {
            return Ok(true);
        }
        match crate::rg::bytes::os_str(line) {
            Ok(osstr) => {
                args.push(osstr.to_os_string());
            }
            Err(err) => {
                errs.push(anyhow::anyhow!(
                    "{line_number}: invalid UTF-8 found at byte offset {}",
                    err.valid_up_to()
                ));
            }
        }
        Ok(true)
    })?;
    Ok((args, errs))
}

#[cfg(test)]
mod tests {
    use super::parse_reader;
    use std::ffi::OsString;

    #[test]
    fn basic() {
        let (args, errs) = parse_reader(
            &b"\
# Test
--context=0
   --smart-case
-u


   # --bar
--foo
"[..],
        )
        .unwrap();
        assert!(errs.is_empty());
        let args: Vec<String> =
            args.into_iter().map(|s| s.into_string().unwrap()).collect();
        assert_eq!(args, vec!["--context=0", "--smart-case", "-u", "--foo",]);
    }

    // We test that we can handle invalid UTF-8 on Unix-like systems.
    #[test]
    #[cfg(unix)]
    fn error() {
        use std::os::unix::ffi::OsStringExt;

        let (args, errs) = parse_reader(
            &b"\
quux
foo\xFFbar
baz
"[..],
        )
        .unwrap();
        assert!(errs.is_empty());
        assert_eq!(
            args,
            vec![
                OsString::from("quux"),
                OsString::from_vec(b"foo\xFFbar".to_vec()),
                OsString::from("baz"),
            ]
        );
    }

    // ... but test that invalid UTF-8 fails on Windows.
    #[test]
    #[cfg(not(unix))]
    fn error() {
        let (args, errs) = parse_reader(
            &b"\
quux
foo\xFFbar
baz
"[..],
        )
        .unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(args, vec![OsString::from("quux"), OsString::from("baz"),]);
    }
}
