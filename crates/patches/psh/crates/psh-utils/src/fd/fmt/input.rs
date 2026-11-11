// Adapted from fd (https://github.com/sharkdp/fd, commit 193d0df), src/fmt/input.rs
// (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: paths are the shell's bytes, so `Path`'s `file_name`, `parent` and `file_stem`
// are spelled out on bytes with the same results for the paths fd prints.

use super::super::output::is_separator;

/// The last component without trailing separators, if it is a name (`Path::file_name`).
fn file_name(path: &[u8]) -> Option<&[u8]> {
    let end = path.len() - path.iter().rev().take_while(|&&b| is_separator(b)).count();
    let p = &path[..end];
    let start = p
        .iter()
        .rposition(|&b| is_separator(b))
        .map_or(0, |i| i + 1);
    let name = &p[start..];
    (!name.is_empty() && name != b"." && name != b"..").then_some(name)
}

/// Removes the parent component of the path
pub fn basename(path: &[u8]) -> &[u8] {
    file_name(path).unwrap_or(path)
}

/// `Path::file_stem` of a file name.
fn file_stem(name: &[u8]) -> &[u8] {
    if name == b".." {
        return name;
    }
    match name.iter().rposition(|&b| b == b'.') {
        Some(0) | None => name,
        Some(i) => &name[..i],
    }
}

/// Removes the extension from the path
pub fn remove_extension(path: &[u8]) -> Vec<u8> {
    let dirname = dirname(path);
    let stem = file_name(path).map_or(path, file_stem);

    let mut joined = dirname;
    if !joined.is_empty() && !joined.last().copied().is_some_and(is_separator) {
        joined.push(b'/');
    }
    joined.extend_from_slice(stem);

    strip_current_dir(&joined).to_vec()
}

/// Removes the basename from the path.
pub fn dirname(path: &[u8]) -> Vec<u8> {
    match parent(path) {
        Some([]) => b".".to_vec(),
        Some(p) => p.to_vec(),
        None => path.to_vec(),
    }
}

/// `Path::parent`: the path without its last component; `None` for a root or "".
fn parent(path: &[u8]) -> Option<&[u8]> {
    let end = path.len() - path.iter().rev().take_while(|&&b| is_separator(b)).count();
    if end == 0 {
        return None;
    }
    let p = &path[..end];
    match p.iter().rposition(|&b| is_separator(b)) {
        None => Some(&[]),
        Some(i) => {
            let keep = i + 1
                - p[..=i]
                    .iter()
                    .rev()
                    .take_while(|&&b| is_separator(b))
                    .count();
            if keep == 0 {
                // The root itself.
                Some(&path[..1])
            } else {
                Some(&p[..keep])
            }
        }
    }
}

/// Remove the `./` prefix from a path.
pub fn strip_current_dir(path: &[u8]) -> &[u8] {
    if path == b"." {
        return &[];
    }
    match path.strip_prefix(b".") {
        Some(rest) if rest.first().copied().is_some_and(is_separator) => {
            let n = rest.iter().take_while(|&&b| is_separator(b)).count();
            &rest[n..]
        }
        _ => path,
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    macro_rules! func_tests {
        ($($name:ident: $func:ident for $input:expr => $output:expr)+) => {
            $(
                #[test]
                fn $name() {
                    assert_eq!(
                        String::from_utf8_lossy(&$func($input.as_bytes())),
                        $output
                    );
                }
            )+
        }
    }

    func_tests! {
        remove_ext_simple:  remove_extension  for  "foo.txt"      =>  "foo"
        remove_ext_dir:     remove_extension  for  "dir/foo.txt"  =>  "dir/foo"
        hidden:             remove_extension  for  ".foo"         =>  ".foo"
        remove_ext_utf8:    remove_extension  for  "💖.txt"       =>  "💖"
        remove_ext_empty:   remove_extension  for  ""             =>  ""
        // psh: the `./` of the result goes, as with `PathBuf::join` and strip_prefix.
        remove_ext_dot:     remove_extension  for  "./x/foo.txt"  =>  "x/foo"

        basename_simple:  basename  for  "foo.txt"      =>  "foo.txt"
        basename_dir:     basename  for  "dir/foo.txt"  =>  "foo.txt"
        basename_empty:   basename  for  ""             =>  ""
        basename_utf8_0:  basename  for  "💖/foo.txt"   =>  "foo.txt"
        basename_utf8_1:  basename  for  "dir/💖.txt"   =>  "💖.txt"

        dirname_simple:  dirname  for  "foo.txt"      =>  "."
        dirname_dir:     dirname  for  "dir/foo.txt"  =>  "dir"
        dirname_utf8_0:  dirname  for  "💖/foo.txt"   =>  "💖"
        dirname_utf8_1:  dirname  for  "dir/💖.txt"   =>  "dir"
        dirname_dot:     dirname  for  "./foo"        =>  "."
        dirname_abs:     dirname  for  "/foo"         =>  "/"
    }

    #[test]
    fn dirname_root() {
        assert_eq!(dirname(b"/"), b"/");
    }
}
