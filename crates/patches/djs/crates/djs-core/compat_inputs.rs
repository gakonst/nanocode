// The build-time inputs of an image's dependency descriptor (D2265): the cargo features
// that change images (D2267) and the source regions of `vm::RECORDS_HASH` (D2266).
// build.rs computes both (`include!`d there, with std); `vm::compat_tests` checks them
// (`include!`d there too).
//
// A region is the text of an item, a whole file, or every top-level `const` of a file or a
// directory, with comments and whitespace removed: editing a doc comment or reformatting
// keeps the hash, changing a number or adding an entry changes it. The regions are the id
// tables and slot conventions that images store numbers of and that the layout, opcode and
// intrinsic hashes do not cover. Images get refused when any of them change, though not
// every change would break one: the price of reading the sources instead of the meaning.

/// The cargo features an image depends on, by bit of `vm::FEATURES` (`Roots::FEATURES`):
/// they change which builtins a VM has or what its images can hold. Append only.
pub const IMAGE_FEATURES: &[&str] = &[
    "full",
    "regexp-unicode-props",
    "unicode-normalization",
    "embed",
];

/// The features that change no image: umbrellas that only name others, and ones that
/// change how the heap is accessed, not what it holds. A feature in neither list fails the
/// build, so that each new one is classified.
pub const IMAGE_NEUTRAL_FEATURES: &[&str] =
    &["compile-cache", "default", "dump", "lazy", "prelude", "unicode", "unsafe-heap"];

/// The `IMAGE_FEATURES` mask of the enabled features (`enabled` as cargo names them).
pub fn feature_mask(enabled: &[String]) -> Result<u32, String> {
    let mut mask = 0;
    for f in enabled {
        if let Some(bit) = IMAGE_FEATURES.iter().position(|n| n == f) {
            mask |= 1 << bit;
        } else if !IMAGE_NEUTRAL_FEATURES.contains(&f.as_str()) {
            return Err(String::from("cargo feature `")
                + f
                + "` is not classified: add it to IMAGE_FEATURES (images depend on it) or \
                   IMAGE_NEUTRAL_FEATURES in crates/djs-core/compat_inputs.rs (D2267)");
        }
    }
    Ok(mask)
}

/// How a region is cut out of its file.
#[derive(Clone, Copy, Debug)]
pub enum Cut {
    /// The item that starts with this text (e.g. `pub enum ObjectClass`) through its
    /// closing brace, or its `;`.
    Item(&'static str),
    /// Every top-level `const` item of the file, or of each non-test `.rs` file under the
    /// directory (a path ending in `/`), but those that name an intrinsic (`= id("Name")`):
    /// the intrinsic table's hash covers them.
    TopConsts,
    /// The whole file.
    All,
}

/// The regions, by path relative to the crate root.
pub const REGIONS: &[(&str, Cut)] = &[
    // Object classes (`Object::CLASS`) and property attribute bits (`PropEntry::FLAGS`).
    ("src/heap/object.rs", Cut::Item("pub enum ObjectClass")),
    ("src/heap/object.rs", Cut::Item("impl PropFlags")),
    // Cell header flags.
    ("src/heap/cells.rs", Cut::Item("pub mod flags")),
    // Typed array element kinds (the builtins use the numbers) and frame continuations
    // (a suspended frame's flags).
    ("src/heap/typed.rs", Cut::Item("pub enum Kind")),
    ("src/interp/frame.rs", Cut::Item("pub enum Continuation")),
    ("src/interp/frame.rs", Cut::Item("impl Continuation")),
    // Realm slots (`Realm::INTRINSICS`) and run states (`Interp::STATE`).
    ("src/interp/realm.rs", Cut::Item("pub enum Intrinsic")),
    ("src/interp/state.rs", Cut::Item("pub enum RunState")),
    // Function template flags, capture descriptors and global declaration records.
    ("src/bytecode/template.rs", Cut::Item("pub mod func_flags")),
    ("src/bytecode/ops.rs", Cut::Item("pub mod capture_desc")),
    ("src/bytecode/ops.rs", Cut::Item("pub mod global_decls")),
    // The value encoding.
    ("src/value.rs", Cut::Item("pub mod tag")),
    ("src/value.rs", Cut::TopConsts),
    // Builtins by index (`FuncTemplate::FAST`, `FuncTemplate::NATIVE`); the generated
    // natives whole, since a suspended native frame stores its state machine's state.
    ("src/builtins/fast.rs", Cut::Item("pub(crate) static FAST")),
    ("src/builtins/gen/natives.rs", Cut::All),
    // Named slot numbers and codes of internal records: frames, generator and promise
    // records, Map/Set tables, weak references, RegExp programs, inline caches, ...
    ("src/heap/", Cut::TopConsts),
    ("src/interp/", Cut::TopConsts),
    ("src/builtins/", Cut::TopConsts),
    ("src/regexp/", Cut::TopConsts),
    ("src/bytecode/template.rs", Cut::TopConsts),
    ("src/bytecode/handlers.rs", Cut::TopConsts),
    ("src/bytecode/lines.rs", Cut::TopConsts),
];

/// `src` without comments and without whitespace outside string literals.
pub fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            // Rust block comments nest.
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == b'"' || raw_string_end(b, i).is_some() {
            let end = raw_string_end(b, i).unwrap_or_else(|| literal_end(b, i));
            out.extend_from_slice(&b[i..end]);
            i = end;
        } else if c == b'\'' && char_literal_end(b, i).is_some() {
            let end = char_literal_end(b, i).unwrap();
            out.extend_from_slice(&b[i..end]);
            i = end;
        } else {
            if !c.is_ascii_whitespace() {
                out.push(c);
            }
            i += 1;
        }
    }
    String::from_utf8(out).expect("stripping keeps UTF-8")
}

/// The end of the string literal starting at `b[i] == '"'`.
fn literal_end(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

/// The end of the raw string literal (`r"..."`, `r#"..."#`) starting at `b[i]`, if one
/// does.
fn raw_string_end(b: &[u8], i: usize) -> Option<usize> {
    if b[i] != b'r' || i.checked_sub(1).is_some_and(|p| is_ident(b[p])) {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let hashes = j - i - 1;
    j += 1;
    while j < b.len() {
        if b[j] == b'"' && b[j + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(b.len())
}

/// The end of a char literal at `b[i] == '\''` (`'x'`, `'\n'`, `'\''`, `'\u{..}'`), or
/// None for a lifetime.
fn char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if b.get(j) == Some(&b'\\') {
        j += 2;
        while j < b.len() && b[j] != b'\'' && j < i + 12 {
            j += 1;
        }
    } else {
        // One UTF-8 character.
        j += 1;
        while j < b.len() && (b[j] & 0xC0) == 0x80 {
            j += 1;
        }
    }
    (b.get(j) == Some(&b'\'')).then_some(j + 1)
}

/// The end of the item that starts at `s[i]` (stripped text): its closing `}` or `;` at
/// bracket depth 0. String and char literals are skipped.
fn item_end(s: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0i32;
    while i < s.len() {
        if let Some(e) = raw_string_end(s, i) {
            i = e;
            continue;
        }
        match s[i] {
            b'"' => {
                i = literal_end(s, i);
                continue;
            }
            b'\'' => {
                if let Some(e) = char_literal_end(s, i) {
                    i = e;
                    continue;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b';' if depth == 0 => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The region of one file's source, stripped; an error if it is not there.
pub fn cut(src: &str, cut: Cut) -> Result<String, String> {
    match cut {
        Cut::All => Ok(strip(src)),
        Cut::Item(start) => {
            let s = strip(src);
            let pat = strip(start);
            let found: Vec<usize> = s
                .match_indices(pat.as_str())
                .map(|(at, _)| at)
                .filter(|&at| {
                    let before = at.checked_sub(1).map(|p| s.as_bytes()[p]);
                    let after = s.as_bytes().get(at + pat.len()).copied();
                    // Whole identifiers only: `pub enum Intrinsic` is not
                    // `pub enum IntrinsicKind`.
                    let (first, last) = (pat.as_bytes()[0], pat.as_bytes()[pat.len() - 1]);
                    !(is_ident(last) && after.is_some_and(is_ident))
                        && !(is_ident(first) && before.is_some_and(is_ident))
                })
                .collect();
            if found.len() != 1 {
                return Err(String::from("`") + start + "` occurs " + &found.len().to_string() + " times");
            }
            let end = item_end(s.as_bytes(), found[0]).ok_or_else(|| String::from("`") + start + "` has no end")?;
            Ok(String::from(&s[found[0]..end]))
        }
        Cut::TopConsts => {
            let mut out = String::new();
            let mut at = 0;
            for line in src.split_inclusive('\n') {
                let rest = ["pub(crate) ", "pub(super) ", "pub ", ""]
                    .iter()
                    .find_map(|p| line.strip_prefix(p).and_then(|l| l.strip_prefix("const ")));
                if let Some(rest) = rest
                    && !rest.starts_with("fn ")
                    && !rest.starts_with("_:")
                {
                    let s = strip(&src[at..]);
                    let end = item_end(s.as_bytes(), 0).ok_or("a const without its `;`")?;
                    if !names_an_intrinsic(&s[..end]) {
                        out.push_str(&s[..end]);
                    }
                }
                at += line.len();
            }
            Ok(out)
        }
    }
}

/// Whether a stripped const item is an intrinsic id by name: `constX:u16=id("Name");`,
/// also with a path (`super::intrinsics::id`) or `iid`.
fn names_an_intrinsic(item: &str) -> bool {
    let Some(eq) = item.find('=') else {
        return false;
    };
    let value = &item[eq + 1..];
    let Some(call) = value.find("id(\"") else {
        return false;
    };
    let path = &value[..call];
    let args = &value[call + 4..];
    (path.is_empty() || path == "i" || path.ends_with("::") || path.ends_with("::i"))
        && path.bytes().all(|c| is_ident(c) || c == b':')
        && args.ends_with("\");")
        && !args[..args.len() - 3].contains(['"', '('])
}

/// FNV-1a.
pub fn fnv(mut h: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// The non-test `.rs` files under `dir` (relative paths, `/`-separated, sorted).
pub fn rust_files(root: &std::path::Path, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = Vec::from([String::from(dir.trim_end_matches('/'))]);
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(root.join(&d)).expect("cannot read a region directory") {
            let name = e.expect("cannot read a region directory").file_name();
            let name = name.to_string_lossy();
            let path = d.clone() + "/" + &name;
            if root.join(&path).is_dir() {
                todo.push(path);
            } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The hash of every region of the crate at `root`. An error names a region that is not
/// found.
pub fn records_hash(root: &std::path::Path) -> Result<u32, String> {
    let mut h: u32 = 0x811c_9dc5;
    for &(path, c) in REGIONS {
        let files = if path.ends_with('/') {
            rust_files(root, path)
        } else {
            Vec::from([String::from(path)])
        };
        for f in files {
            let src = std::fs::read_to_string(root.join(&f)).map_err(|e| f.clone() + ": " + &e.to_string())?;
            let text = cut(&src, c).map_err(|e| f.clone() + ": " + &e)?;
            h = fnv(h, f.as_bytes());
            h = fnv(h, &[0]);
            h = fnv(h, text.as_bytes());
            h = fnv(h, &[0]);
        }
    }
    Ok(h)
}
