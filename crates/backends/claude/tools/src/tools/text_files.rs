use serde_json::Value;

const MAX_READ_BYTES: u64 = 262_144;
const MAX_READ_TOKENS: usize = 25_000;
const BYTES_PER_TOKEN: usize = 4;
const SELECTED_BYTES_PER_TOKEN: u64 = 128;
const ALLOW_LARGE_BYTES_PER_TOKEN: u64 = 3;
const TRUNCATED: &str = "[Truncated: PARTIAL view \u{2014} ";
const FITS_WITH_ALLOW_LARGE: &str = " To read it anyway, up to what still fits in your context, call Read again with allow_large: true \u{2014} only if you genuinely need all of it or the user asked for the whole file.";
const TOO_LARGE_FOR_ROOM: &str = " Even with allow_large, this is more than fits in the context you have left this turn (other large reads in the same turn share that room).";
const ALLOW_LARGE_UNAVAILABLE: &str =
    " allow_large is not available where this read runs; read it in parts with offset and limit.";
const BINARY_EXTENSIONS: [&str; 88] = [
    ".bmp", ".ico", ".tiff", ".tif", ".mp4", ".mov", ".avi", ".mkv", ".webm", ".wmv", ".flv",
    ".m4v", ".mpeg", ".mpg", ".mp3", ".wav", ".ogg", ".flac", ".aac", ".m4a", ".wma", ".aiff",
    ".opus", ".zip", ".tar", ".gz", ".bz2", ".7z", ".rar", ".xz", ".z", ".tgz", ".iso", ".exe",
    ".dll", ".so", ".dylib", ".bin", ".o", ".a", ".obj", ".lib", ".app", ".msi", ".deb", ".rpm",
    ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp", ".ttf", ".otf",
    ".woff", ".woff2", ".eot", ".pyc", ".pyo", ".class", ".jar", ".war", ".ear", ".node", ".wasm",
    ".rlib", ".sqlite", ".sqlite3", ".db", ".mdb", ".idx", ".psd", ".ai", ".eps", ".sketch",
    ".fig", ".xd", ".blend", ".3ds", ".max", ".swf", ".fla", ".lockb", ".dat", ".data",
];

#[derive(Clone, Copy)]
struct ReadLimits {
    bytes: u64,
    tokens: usize,
}

pub(crate) fn size_text(bytes: u64) -> String {
    let trimmed = |value: f64| format!("{value:.1}").trim_end_matches(".0").to_owned();
    let kilobytes = bytes as f64 / 1024.0;
    if kilobytes < 1.0 {
        return format!("{bytes} bytes");
    }
    if kilobytes < 1024.0 {
        return format!("{}KB", trimmed(kilobytes));
    }
    let megabytes = kilobytes / 1024.0;
    if megabytes < 1024.0 {
        return format!("{}MB", trimmed(megabytes));
    }
    format!("{}GB", trimmed(megabytes / 1024.0))
}

pub fn read<I: Iterator<Item = Result<String, String>>>(
    file_path: &str,
    size: u64,
    input: &Value,
    read_token_budget: usize,
    open: impl FnOnce() -> Result<I, String>,
) -> Result<String, String> {
    read_view(file_path, size, input, read_token_budget, open).map(|(text, _)| text)
}

pub fn read_view<I: Iterator<Item = Result<String, String>>>(
    file_path: &str,
    size: u64,
    input: &Value,
    read_token_budget: usize,
    open: impl FnOnce() -> Result<I, String>,
) -> Result<(String, bool), String> {
    let offset = input
        .get("offset")
        .map_or(Some(1), |value| {
            value.as_i64().map(|offset| offset.max(0).unsigned_abs())
        })
        .ok_or("invalid offset")?;
    let limit = input
        .get("limit")
        .map(|value| value.as_u64().ok_or("invalid limit"))
        .transpose()?
        .filter(|limit| *limit > 0);
    let opted = input
        .get("allow_large")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let defaults = ReadLimits {
        bytes: MAX_READ_BYTES,
        tokens: MAX_READ_TOKENS,
    };
    let room = (read_token_budget != usize::MAX).then_some(read_token_budget);
    let raised = room
        .filter(|room| *room > MAX_READ_TOKENS)
        .map(|room| ReadLimits {
            bytes: (room as u64)
                .saturating_mul(BYTES_PER_TOKEN as u64 * ALLOW_LARGE_BYTES_PER_TOKEN)
                .max(MAX_READ_BYTES),
            tokens: room,
        });
    let limits = raised.filter(|_| opted).unwrap_or(defaults);
    let hint = |bytes: Option<u64>, tokens: Option<usize>| -> &'static str {
        let Some(_) = room else {
            return if opted { ALLOW_LARGE_UNAVAILABLE } else { "" };
        };
        let Some(raised) = raised.filter(|_| !opted) else {
            return TOO_LARGE_FOR_ROOM;
        };
        if bytes.is_none_or(|bytes| bytes <= raised.bytes)
            && tokens.is_none_or(|tokens| tokens <= raised.tokens)
        {
            FITS_WITH_ALLOW_LARGE
        } else {
            TOO_LARGE_FOR_ROOM
        }
    };
    if limit.is_none() && size > limits.bytes {
        return Err(format!(
            "File content ({}) exceeds maximum allowed size ({}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.{}",
            size_text(size),
            size_text(limits.bytes),
            hint(Some(size), None)
        ));
    }
    let max_selected = (MAX_READ_TOKENS as u64 * SELECTED_BYTES_PER_TOKEN)
        .max(raised.filter(|_| opted).map_or(0, |raised| raised.bytes));
    let start = offset.saturating_sub(1);
    let end = limit.map(|limit| start.saturating_add(limit));
    let mut lines = Vec::new();
    let mut selected = 0_u64;
    let mut total = 0_u64;
    for line in open()? {
        let mut line = line?;
        if total == 0 && line.starts_with('\u{feff}') {
            line.drain(..'\u{feff}'.len_utf8());
        }
        if total >= start && end.is_none_or(|end| total < end) {
            selected += line.len() as u64 + u64::from(!lines.is_empty());
            if limit.is_some() && selected > max_selected {
                return Err(format!(
                    "The requested line range contains over {} of text, more than a read can return. Use a smaller limit \u{2014} or, if a single line is this large, no limit will fit it: search for specific content instead.{}",
                    size_text(max_selected),
                    hint(Some(selected), None)
                ));
            }
            lines.push(line);
        }
        total += 1;
    }
    let first = offset.max(1);
    let whole = start == 0 && end.is_none_or(|end| end >= total);
    let content = lines.join("\n");
    let tokens = content.len().div_ceil(BYTES_PER_TOKEN);
    let mut banner = None;
    let mut shown = lines.len();
    let mut text = content;
    if tokens > limits.tokens {
        if offset > 1 || limit.is_some() {
            return Err(format!(
                "File content ({tokens} tokens) exceeds maximum allowed tokens ({}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.{}",
                limits.tokens,
                hint(None, Some(tokens))
            ));
        }
        let cap = MAX_READ_TOKENS;
        let reason = if opted && limits.tokens > cap {
            format!(
                "allow_large had room for {} tokens this turn",
                limits.tokens
            )
        } else {
            format!("cap {cap}")
        };
        let per_token = (text.len() as f64 / tokens.max(1) as f64).max(0.5);
        let estimate = |text: &str| text.len() as f64 / per_token;
        let mut count = ((lines.len() as f64 * cap as f64 / tokens.max(1) as f64 * 0.85).floor()
            as usize)
            .clamp(1, lines.len().max(1));
        let mut page = lines[..count.min(lines.len())].join("\n");
        for _ in 0..6 {
            if estimate(&page) <= cap as f64 || count <= 1 {
                break;
            }
            count = ((count as f64 * 0.7).floor() as usize).max(1);
            page = lines[..count].join("\n");
        }
        let hint = hint(None, Some(tokens));
        if estimate(&page) > cap as f64 || page.trim().is_empty() {
            let mut chars = ((cap as f64 * per_token * 0.85).floor() as usize).max(1);
            for _ in 0..6 {
                page = text[..text.floor_char_boundary(chars)].to_owned();
                if estimate(&page) <= cap as f64 {
                    break;
                }
                chars = ((chars as f64 * 0.7).floor() as usize).max(1);
            }
            banner = Some(format!(
                "{TRUNCATED}{file_path}: showing the first {} of {} characters ({tokens} tokens, {reason}); this file has very long lines and cannot be paginated by line. Use Grep to find a specific section, or Read with offset/limit to page through it.{hint} Do NOT answer from this excerpt alone if the answer may be elsewhere in the file.]",
                page.chars().count(),
                text.chars().count()
            ));
            shown = page.split('\n').count();
        } else {
            shown = count;
            if (count as u64) < total {
                banner = Some(format!(
                    "{TRUNCATED}{file_path}: showing lines 1-{count} of {total} total ({tokens} tokens, {reason}). Call Read with offset={} limit={count} for the next page, or Grep to find a specific section.{hint} Do NOT answer from this page alone if the answer may be further in the file.]",
                    count + 1
                ));
            }
        }
        text = page;
    }
    let mut out = if !text.is_empty() || (shown >= 1 && total > 1) {
        text.split('\n')
            .enumerate()
            .map(|(index, line)| format!("{}\t{line}", first + index as u64))
            .collect::<Vec<_>>()
            .join("\n")
    } else if shown >= 1 || total == 0 {
        "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
            .to_owned()
    } else {
        format!(
            "<system-reminder>Warning: the file exists but is shorter than the provided offset ({first}). The file has {total} lines.</system-reminder>"
        )
    };
    let whole = whole && banner.is_none();
    if let Some(banner) = banner {
        out.push_str(&format!("\n<system-reminder>{banner}</system-reminder>"));
    }
    Ok((out, whole))
}

#[must_use]
pub fn binary(file_path: &str) -> Option<String> {
    let extension = file_path[file_path.rfind('.')?..].to_ascii_lowercase();
    BINARY_EXTENSIONS.contains(&extension.as_str()).then(|| {
        format!("This tool cannot read binary files. The file appears to be a binary {extension} file. Use a skill for this file type if one is available, or a shell command or script that can read the format.")
    })
}

pub fn definitions() -> Vec<Value> {
    ["Read", "Edit", "Write"]
        .into_iter()
        .map(crate::upstream::tool_value)
        .collect()
}

pub fn check(name: &str, input: &Value) -> Result<(), String> {
    let allowed: &[&str] = match name {
        "Read" => &["file_path", "offset", "limit", "pages", "allow_large"],
        "Write" => &["file_path", "content"],
        "Edit" => &["file_path", "old_string", "new_string", "replace_all"],
        _ => return Err(format!("unknown workspace tool: {name}")),
    };
    let fields = input
        .as_object()
        .ok_or("workspace input must be an object")?;
    if let Some(key) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unsupported {name} option: {key}"));
    }
    Ok(())
}

pub struct Edit<'a> {
    old: &'a str,
    new: &'a str,
    replace_all: bool,
}

impl<'a> Edit<'a> {
    pub fn parse(input: &'a Value) -> Result<Self, String> {
        let field = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing or invalid {key}"))
        };
        let old = field("old_string")?;
        let new = field("new_string")?;
        let replace_all = input
            .get("replace_all")
            .map_or(Some(false), Value::as_bool)
            .ok_or("invalid replace_all")?;
        Ok(Self {
            old,
            new,
            replace_all,
        })
    }

    #[must_use]
    pub const fn replace_all(&self) -> bool {
        self.replace_all
    }

    #[must_use]
    pub const fn creates(&self) -> bool {
        self.old.is_empty()
    }

    #[must_use]
    pub fn changes(&self) -> bool {
        self.old != self.new
    }

    pub fn apply(&self, content: &str) -> Result<(String, usize), String> {
        if self.creates() {
            if !content.trim().is_empty() {
                return Err("Cannot create new file - file already exists.".into());
            }
            return Ok((self.new.to_owned(), 1));
        }
        if self.old == self.new {
            return Err(
                "No changes to make: old_string and new_string are exactly the same.".into(),
            );
        }
        let count = content.matches(self.old).count();
        if count == 0 {
            return Err(format!(
                "String to replace not found in file.\nString: {}",
                self.old
            ));
        }
        if count != 1 && !self.replace_all {
            return Err(format!(
                "Found {count} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {}",
                self.old
            ));
        }
        let updated = if self.replace_all {
            content.replace(self.old, self.new)
        } else {
            content.replacen(self.old, self.new, 1)
        };
        Ok((updated, count))
    }
}

const FILE_STATE: &str =
    " (file state is current in your context \u{2014} no need to Read it back)";
const STALE: &str = " (note: the file had been modified on disk since you last read it \u{2014} the edit applied cleanly, but the file contains other changes not in your context. Read it before edits that depend on surrounding content.)";
pub const MODIFIED_SINCE_READ: &str = "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    Current,
    Unread,
    Stale,
}

#[must_use]
pub fn edited(shown: &str, replace_all: bool) -> String {
    edited_with(shown, replace_all, Known::Current)
}

#[must_use]
pub fn edited_with(shown: &str, replace_all: bool, known: Known) -> String {
    let note = match known {
        Known::Current => FILE_STATE,
        Known::Unread => "",
        Known::Stale => STALE,
    };
    if replace_all {
        format!(
            "The file {shown} has been updated. All occurrences were successfully replaced.{note}"
        )
    } else {
        format!("The file {shown} has been updated successfully.{note}")
    }
}

#[must_use]
pub fn written(shown: &str, existed: bool) -> String {
    if existed {
        format!("The file {shown} has been updated successfully.{FILE_STATE}")
    } else {
        format!("File created successfully at: {shown}{FILE_STATE}")
    }
}
