//! `sqlite3 [OPTIONS] [FILE] [SQL...]`: read-only when every SQL argument only reads
//! (SELECT, read pragmas, `.schema`, `.tables`, ...), whether or not `-readonly` is
//! given; a mutation when the SQL writes the database but runs nothing else; unknown
//! when the SQL comes from stdin or a file, or can write other files or run programs.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Category;

/// Options that take a value.
const VALUED: &[&str] = &[
    "-cmd",
    "-init",
    "-separator",
    "-newline",
    "-nullvalue",
    "-escape",
    "-maxsize",
    "-mmap",
    "-pagecache",
    "-lookaside",
    "-vfs",
    "-heap",
    "-A",
];

/// What sqlite3 does, and the index of the database operand: `None` for unknown (no SQL
/// to inspect, an `-init` file, the archive mode, or SQL that runs programs or writes
/// other files).
pub(crate) fn classify(args: &[Option<&str>]) -> Option<(Category, Option<usize>)> {
    let mut sql: Vec<&str> = Vec::new();
    let mut file = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i]?;
        let opt = a
            .strip_prefix('-')
            .map(|o| o.strip_prefix('-').unwrap_or(o));
        match opt {
            Some("init") | Some("A") | Some("unsafe-testing") => return None,
            Some("cmd") => {
                sql.push(args.get(i + 1).copied().flatten()?);
                i += 1;
            }
            Some(o) if VALUED.iter().any(|v| &v[1..] == o) => i += 1,
            Some("version" | "help") => return Some((Category::Read, None)),
            Some(_) => {}
            None if file.is_none() => file = Some(i),
            None => sql.push(a),
        }
        i += 1;
    }
    if sql.is_empty() {
        // The SQL would come from stdin.
        return None;
    }
    let mut writes = false;
    for s in &sql {
        match statements(s)? {
            Effect::Read => {}
            Effect::WritesDatabase => writes = true,
        }
    }
    let c = if writes {
        Category::Mutate
    } else {
        Category::Read
    };
    Some((c, file))
}

enum Effect {
    Read,
    WritesDatabase,
}

/// Dot-commands that only show things.
const READ_DOT: &[&str] = &[
    "schema",
    "tables",
    "table",
    "indexes",
    "indices",
    "headers",
    "header",
    "mode",
    "width",
    "dbinfo",
    "databases",
    "fullschema",
    "show",
    "timer",
    "nullvalue",
    "separator",
    "help",
    "eqp",
    "stats",
    "print",
    "echo",
    "bail",
    "changes",
    "dbconfig",
    "lint",
];

/// Pragmas that only read when written without a value.
const READ_PRAGMAS: &[&str] = &[
    "table_info",
    "table_xinfo",
    "table_list",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
    "foreign_key_check",
    "database_list",
    "compile_options",
    "integrity_check",
    "quick_check",
    "page_count",
    "page_size",
    "freelist_count",
    "user_version",
    "schema_version",
    "application_id",
    "journal_mode",
    "encoding",
    "collation_list",
    "function_list",
    "module_list",
    "pragma_list",
    "data_version",
    "auto_vacuum",
    "cache_size",
    "foreign_keys",
    "synchronous",
    "busy_timeout",
];

/// Words that make a statement write the database (or be one we do not try to read).
const WRITING: &[&str] = &[
    "insert",
    "update",
    "delete",
    "replace",
    "upsert",
    "create",
    "drop",
    "alter",
    "reindex",
    "analyze",
    "vacuum",
    "begin",
    "commit",
    "rollback",
    "savepoint",
    "release",
];

/// Words that write other files or run code.
const OPAQUE: &[&str] = &[
    "attach",
    "detach",
    "load_extension",
    "writefile",
    "edit",
    "fts3_tokenizer",
];

/// `None` when the SQL may do more than read or write its database.
fn statements(sql: &str) -> Option<Effect> {
    let mut effect = Effect::Read;
    for line in split(sql) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(dot) = t.strip_prefix('.') {
            let name = dot.split_whitespace().next().unwrap_or("");
            if !READ_DOT.contains(&name) {
                return None;
            }
            continue;
        }
        let words = words(t);
        if words.iter().any(|w| OPAQUE.contains(&w.as_str())) {
            return None;
        }
        let first = words.first().map(String::as_str).unwrap_or("");
        // `VACUUM INTO 'file'` writes another file.
        if first == "vacuum" && words.iter().any(|w| w == "into") {
            return None;
        }
        match first {
            "select" | "values" | "with" | "explain" => {}
            "pragma" => {
                let name = words.get(1).map(String::as_str).unwrap_or("");
                // `PRAGMA main.x`: the schema name comes first.
                let name = if words.get(2).is_some() && t.contains('.') && !t.contains('(') {
                    words.get(2).map(String::as_str).unwrap_or(name)
                } else {
                    name
                };
                if t.contains('=') || !READ_PRAGMAS.contains(&name) {
                    effect = Effect::WritesDatabase;
                }
                continue;
            }
            _ if WRITING.contains(&first) => {
                effect = Effect::WritesDatabase;
                continue;
            }
            _ => return None,
        }
        if words.iter().any(|w| WRITING.contains(&w.as_str())) {
            effect = Effect::WritesDatabase;
        }
    }
    Some(effect)
}

/// Statements: split at `;` outside quotes, and dot-commands on lines of their own.
fn split(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut line_start = true;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if line_start && cur.trim().is_empty() && c == '.' => {
                // A dot-command runs to the end of the line.
                out.push(core::mem::take(&mut cur));
                cur.push('.');
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                    cur.push(d);
                }
                out.push(core::mem::take(&mut cur));
                line_start = true;
                continue;
            }
            None => match c {
                '\'' | '"' | '`' | '[' => {
                    quote = Some(if c == '[' { ']' } else { c });
                    cur.push(c);
                }
                ';' => out.push(core::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
        if c == '\n' {
            line_start = true;
        } else if !c.is_whitespace() {
            line_start = false;
        }
    }
    out.push(cur);
    out
}

/// The statement's keywords and identifiers, lower-cased; string literals and quoted
/// identifiers are dropped.
fn words(stmt: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in stmt.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' || c == '`' || c == '[' {
            quote = Some(if c == '[' { ']' } else { c });
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(args: &[&str]) -> Option<Category> {
        let a: Vec<Option<&str>> = args.iter().map(|s| Some(*s)).collect();
        classify(&a).map(|(c, _)| c)
    }

    #[test]
    fn reads() {
        assert_eq!(
            c(&["db", "select * from t where name = 'drop table'"]),
            Some(Category::Read)
        );
        assert_eq!(c(&["-readonly", "db", ".schema"]), Some(Category::Read));
        assert_eq!(
            c(&["-header", "-json", "db", ".tables"]),
            Some(Category::Read)
        );
        assert_eq!(c(&["db", "PRAGMA table_info(users)"]), Some(Category::Read));
        assert_eq!(
            c(&["db", "pragma user_version; select count(*) from x;"]),
            Some(Category::Read)
        );
        assert_eq!(c(&["db", ".mode column\nselect 1"]), Some(Category::Read));
        assert_eq!(
            c(&["-cmd", ".headers on", "db", "select 1"]),
            Some(Category::Read)
        );
        assert_eq!(c(&["--version"]), Some(Category::Read));
    }

    #[test]
    fn writes() {
        assert_eq!(
            c(&["db", "insert into t values (1)"]),
            Some(Category::Mutate)
        );
        assert_eq!(
            c(&["db", "PRAGMA journal_mode=WAL"]),
            Some(Category::Mutate)
        );
        assert_eq!(
            c(&["db", "with x as (select 1) delete from t"]),
            Some(Category::Mutate)
        );
        assert_eq!(c(&["db", "create table t(a)"]), Some(Category::Mutate));
    }

    #[test]
    fn unknown() {
        assert_eq!(c(&["db"]), None);
        assert_eq!(c(&["db", ".shell rm -rf /"]), None);
        assert_eq!(c(&["db", "select 1;\n.system ls"]), None);
        assert_eq!(c(&["db", ".output out.txt"]), None);
        assert_eq!(c(&["db", "attach 'x.db' as x"]), None);
        assert_eq!(c(&["db", "select writefile('x', 'y')"]), None);
        assert_eq!(c(&["db", "select load_extension('x')"]), None);
        assert_eq!(c(&["db", "vacuum into 'copy.db'"]), None);
        assert_eq!(c(&["-init", "x.sql", "db", "select 1"]), None);
        assert_eq!(c(&["-readonly", "db"]), None);
        assert_eq!(classify(&[Some("db"), None]), None);
    }
}
