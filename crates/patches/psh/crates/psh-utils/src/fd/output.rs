// Adapted from fd (https://github.com/sharkdp/fd, commit 193d0df), src/output.rs and
// the path helpers of src/dir_entry.rs, with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: entries are the walk's (shell paths as bytes, `/`-joined; `\` separates too on
// Windows), printed into a byte buffer; styles come with the entry. No hyperlinks
// (`--hyperlink` runs the real fd).

use super::colors::{Indicator, LsColors, Style};
use super::fmt::{FormatTemplate, strip_current_dir};
use super::sanitize::sanitize_for_term;

/// What printing needs of fd's `Config`.
pub struct PrintConfig {
    pub format: Option<FormatTemplate>,
    pub ls_colors: Option<LsColors>,
    pub null_separator: bool,
    pub path_separator: Option<String>,
    pub actual_path_separator: String,
    pub interactive_terminal: bool,
    pub strip_cwd_prefix: bool,
}

/// One result: its path as walked and whether it is a directory (for the trailing
/// separator), with its style when colored.
pub struct Printed<'a> {
    pub path: &'a [u8],
    pub is_dir: bool,
    pub style: Option<Style>,
}

/// A path separator of the platform.
pub fn is_separator(b: u8) -> bool {
    b == b'/' || (cfg!(windows) && b == b'\\')
}

/// Returns the path as it should be presented to the user.
/// When stripping `./` would leave the path starting with `-`, keep the original
/// (with `./`) so downstream tools don't interpret it as an option.
pub fn stripped_path<'a>(path: &'a [u8], config: &PrintConfig) -> &'a [u8] {
    if config.strip_cwd_prefix {
        let stripped = strip_current_dir(path);
        if stripped.first() == Some(&b'-') {
            path
        } else {
            stripped
        }
    } else {
        path
    }
}

fn replace_path_separator(path: &str, new_path_separator: &str) -> String {
    // psh: every separator of the platform (fd: `MAIN_SEPARATOR`; psh joins with `/`).
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii() && is_separator(c as u8) {
            out.push_str(new_path_separator);
        } else {
            out.push(c);
        }
    }
    out
}

// TODO: this function is performance critical and can probably be optimized
pub fn print_entry(stdout: &mut Vec<u8>, entry: &Printed<'_>, config: &PrintConfig) {
    if let Some(ref format) = config.format {
        print_entry_format(stdout, entry, config, format);
    } else if let Some(ref ls_colors) = config.ls_colors {
        print_entry_colorized(stdout, entry, config, ls_colors);
    } else {
        print_entry_uncolorized(stdout, entry, config);
    };

    if config.null_separator {
        stdout.push(0);
    } else {
        stdout.push(b'\n');
    }
}

// Display a trailing slash if the path is a directory and the config option is enabled.
// If the path_separator option is set, display that instead.
// The trailing slash will not be colored.
#[inline]
fn print_trailing_slash(
    stdout: &mut Vec<u8>,
    entry: &Printed<'_>,
    config: &PrintConfig,
    style: Option<&Style>,
) {
    if entry.is_dir {
        let s = style
            .copied()
            .unwrap_or_default()
            .paint(&config.actual_path_separator);
        stdout.extend_from_slice(s.as_bytes());
    }
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_format(
    stdout: &mut Vec<u8>,
    entry: &Printed<'_>,
    config: &PrintConfig,
    format: &FormatTemplate,
) {
    let output = format.generate(
        stripped_path(entry.path, config),
        config.path_separator.as_deref(),
    );
    // TODO: support writing raw bytes on unix?
    let s = String::from_utf8_lossy(&output);
    stdout.extend_from_slice(
        sanitize_for_term(&s, config.interactive_terminal)
            .to_string()
            .as_bytes(),
    );
}

/// The length of a path's parent, `Path::parent`'s (psh: on bytes).
fn parent_len(path: &[u8]) -> Option<usize> {
    let end = path.len() - path.iter().rev().take_while(|&&b| is_separator(b)).count();
    if end == 0 {
        return None;
    }
    let p = &path[..end];
    match p.iter().rposition(|&b| is_separator(b)) {
        None => Some(0),
        Some(i) => {
            let keep = i + 1
                - p[..=i]
                    .iter()
                    .rev()
                    .take_while(|&&b| is_separator(b))
                    .count();
            Some(if keep == 0 { 1 } else { keep })
        }
    }
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_colorized(
    stdout: &mut Vec<u8>,
    entry: &Printed<'_>,
    config: &PrintConfig,
    ls_colors: &LsColors,
) {
    let mut offset = 0;
    let path = stripped_path(entry.path, config);
    let path_str = String::from_utf8_lossy(path);

    if let Some(parent) = parent_len(path) {
        offset = String::from_utf8_lossy(&path[..parent]).len();
        for c in path_str[offset..].chars() {
            if c.is_ascii() && is_separator(c as u8) {
                offset += c.len_utf8();
            } else {
                break;
            }
        }
    }

    let dir_style = ls_colors
        .style_for_indicator(Indicator::Directory)
        .copied()
        .unwrap_or_default();
    if offset > 0 {
        let mut parent_str = path_str[..offset].to_string();
        if let Some(ref separator) = config.path_separator {
            parent_str = replace_path_separator(&parent_str, separator);
        }

        let safe_parent = sanitize_for_term(&parent_str, config.interactive_terminal);
        // We explicitly use prefix/suffix instead of paint so that we can use Sanitized
        let s = format!(
            "{}{}{}",
            dir_style.prefix(),
            safe_parent,
            dir_style.suffix()
        );
        stdout.extend_from_slice(s.as_bytes());
    }

    let style = entry.style.unwrap_or_default();
    let safe_basename = sanitize_for_term(&path_str[offset..], config.interactive_terminal);
    // We explicitly use prefix/suffix instead of paint so that we can use Sanitized
    let s = format!("{}{}{}", style.prefix(), safe_basename, style.suffix());
    stdout.extend_from_slice(s.as_bytes());

    print_trailing_slash(
        stdout,
        entry,
        config,
        ls_colors.style_for_indicator(Indicator::Directory),
    );
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_uncolorized_base(stdout: &mut Vec<u8>, entry: &Printed<'_>, config: &PrintConfig) {
    let path = stripped_path(entry.path, config);

    let mut path_string = String::from_utf8_lossy(path).into_owned();
    if let Some(ref separator) = config.path_separator {
        path_string = replace_path_separator(&path_string, separator);
    }
    let safe = sanitize_for_term(&path_string, config.interactive_terminal);
    stdout.extend_from_slice(safe.to_string().as_bytes());
    print_trailing_slash(stdout, entry, config, None)
}

fn print_entry_uncolorized(stdout: &mut Vec<u8>, entry: &Printed<'_>, config: &PrintConfig) {
    // psh: fd's Unix branch on every platform: shell paths are bytes.
    if config.interactive_terminal || config.path_separator.is_some() {
        print_entry_uncolorized_base(stdout, entry, config)
    } else {
        // Piped output: raw bytes so invalid UTF-8 filenames reach downstream tools intact.
        stdout.extend_from_slice(stripped_path(entry.path, config));
        print_trailing_slash(stdout, entry, config, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PrintConfig {
        PrintConfig {
            format: None,
            ls_colors: None,
            null_separator: false,
            path_separator: None,
            actual_path_separator: "/".into(),
            interactive_terminal: false,
            strip_cwd_prefix: true,
        }
    }

    fn print(path: &str, is_dir: bool, config: &PrintConfig) -> String {
        let mut out = Vec::new();
        let style = config.ls_colors.as_ref().and_then(|c| {
            let facts = super::super::colors::Facts {
                name: path.rsplit('/').next().unwrap().as_bytes(),
                kind: Some(if is_dir {
                    psh_engine::host::FileKind::Dir
                } else {
                    psh_engine::host::FileKind::File
                }),
                mode: Some(0o644),
                target_exists: true,
            };
            c.style_for(&facts).copied()
        });
        print_entry(
            &mut out,
            &Printed {
                path: path.as_bytes(),
                is_dir,
                style,
            },
            config,
        );
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn dash_prefixed_paths_keep_their_dot() {
        let c = config();
        assert_eq!(print("./-rf", false, &c), "./-rf\n");
        assert_eq!(print("./a/-rf", false, &c), "a/-rf\n");
        assert_eq!(print("./dir", true, &c), "dir/\n");
    }

    #[test]
    fn separators_and_colors() {
        let mut c = config();
        c.path_separator = Some("#".into());
        c.actual_path_separator = "#".into();
        assert_eq!(print("./a/b", true, &c), "a#b#\n");
        let mut c = config();
        c.ls_colors = Some(LsColors::from_string("di=01;34:*.rs=91"));
        assert_eq!(
            print("./src/a.rs", false, &c),
            "\x1b[1;34msrc/\x1b[0m\x1b[38;5;9ma.rs\x1b[0m\n"
        );
        assert_eq!(print("./-x", false, &c), "\x1b[1;34m./\x1b[0m-x\n");
    }

    #[test]
    fn sanitized_on_a_terminal() {
        let mut c = config();
        c.interactive_terminal = true;
        assert_eq!(print("./a\x1bb", false, &c), "a\\x1Bb\n");
        c.interactive_terminal = false;
        assert_eq!(print("./a\x1bb", false, &c), "a\x1bb\n");
    }
}
