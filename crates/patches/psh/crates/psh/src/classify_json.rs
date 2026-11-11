//! `psh --classify`: what a script does, as JSON.
//!
//! The shape (spans are `[start, end]` byte offsets into the UTF-8 source, or into
//! `sources[i].text` when an object has `"source": i`):
//!
//! - `ok`, `error` (`null` or `{message, span, line, column, incomplete}`, as `--parse`),
//!   `primary` (one category), `categories`, `read_only`, `truncated`.
//! - `commands`: every simple command, nested ones included, in walk order: `span`,
//!   `source`, `name` (`null` if dynamic or a path outside the system directories),
//!   `parent` (index of the command it runs inside), `via` (`script`,
//!   `command-substitution`, `backtick`, `process-substitution`, `wrapper`, `xargs`,
//!   `find-exec`, `shell-string`, `eval`, `trap`, `alias`), `primary`, `categories`,
//!   `reason` (why it is unknown, or `null`), `words` and `assigns` (`{span, text,
//!   value}`; `value` is `null` when the word has expansions), `redirects` (`{span, fd,
//!   op, target}`; `target` is a word or `null` for a here-document).
//! - `reads`, `writes`: `{path, span, source, command}`; `path` is `null` when dynamic,
//!   `command` is `null` for a redirection of a compound command.
//! - `sources`: shell code found in strings (`bash -c`, `eval`, ...): `{span, source,
//!   text}`.
//!
//! Categories: `read`, `network`, `build`, `long-running`, `mutate`, `vcs`, `install`,
//! `unknown`; `primary` is the last of them present.

use psh_classify::{Arg, Categories, Classification, PathEffect, Span};
use psh_syntax::LineIndex;

use crate::json::Json;

pub fn to_json(c: &Classification, source: &str) -> Json {
    let error = match &c.error {
        None => Json::Null,
        Some(e) => {
            let lc = LineIndex::new(source).line_col(source, e.span.start);
            Json::obj()
                .set("message", e.message.as_str())
                .set("span", span(e.span))
                .set("line", lc.line)
                .set("column", lc.column)
                .set("incomplete", e.incomplete)
        }
    };
    let commands = c
        .commands
        .iter()
        .map(|x| {
            Json::obj()
                .set("span", span(x.span))
                .set("source", x.source)
                .set("name", x.name.as_deref())
                .set("parent", x.parent)
                .set("via", x.via.name())
                .set("primary", x.primary().name())
                .set("categories", categories(x.categories))
                .set("reason", x.reason)
                .set("words", x.words.iter().map(arg).collect::<Vec<_>>())
                .set("assigns", x.assigns.iter().map(arg).collect::<Vec<_>>())
                .set(
                    "redirects",
                    x.redirects
                        .iter()
                        .map(|r| {
                            Json::obj()
                                .set("span", span(r.span))
                                .set("fd", r.fd)
                                .set("op", r.op.as_str())
                                .set("target", r.target.as_ref().map_or(Json::Null, arg))
                        })
                        .collect::<Vec<_>>(),
                )
        })
        .collect::<Vec<_>>();
    let sources = c
        .sources
        .iter()
        .map(|s| {
            Json::obj()
                .set("span", span(s.span))
                .set("source", s.source)
                .set("text", s.text.as_str())
        })
        .collect::<Vec<_>>();
    Json::obj()
        .set("ok", c.error.is_none())
        .set("error", error)
        .set("primary", c.primary().name())
        .set("categories", categories(c.categories))
        .set("read_only", c.read_only())
        .set("truncated", c.truncated)
        .set("commands", commands)
        .set("reads", c.reads.iter().map(path).collect::<Vec<_>>())
        .set("writes", c.writes.iter().map(path).collect::<Vec<_>>())
        .set("sources", sources)
}

fn span(s: Span) -> Json {
    Json::Arr(vec![Json::from(s.start), Json::from(s.end)])
}

fn categories(c: Categories) -> Json {
    Json::Arr(c.iter().map(|c| Json::from(c.name())).collect())
}

fn arg(a: &Arg) -> Json {
    Json::obj()
        .set("span", span(a.span))
        .set("text", a.text.as_str())
        .set("value", a.value.as_deref())
}

fn path(p: &PathEffect) -> Json {
    Json::obj()
        .set("path", p.path.as_deref())
        .set("span", span(p.span))
        .set("source", p.source)
        .set("command", p.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(src: &str) -> String {
        to_json(&psh_classify::classify(src), src).to_compact()
    }

    #[test]
    fn read_only_pipeline() {
        assert_eq!(
            json("ls | head -3"),
            concat!(
                r#"{"ok":true,"error":null,"primary":"read","categories":["read"],"read_only":true,"truncated":true,"commands":["#,
                r#"{"span":[0,2],"source":null,"name":"ls","parent":null,"via":"script","primary":"read","categories":["read"],"reason":null,"words":[{"span":[0,2],"text":"ls","value":"ls"}],"assigns":[],"redirects":[]},"#,
                r#"{"span":[5,12],"source":null,"name":"head","parent":null,"via":"script","primary":"read","categories":["read"],"reason":null,"words":[{"span":[5,9],"text":"head","value":"head"},{"span":[10,12],"text":"-3","value":"-3"}],"assigns":[],"redirects":[]}],"#,
                r#""reads":[],"writes":[],"sources":[]}"#
            )
        );
    }

    #[test]
    fn nested_and_writes() {
        let j = json("bash -c 'rm x' > log");
        assert!(j.contains(r#""primary":"mutate""#), "{j}");
        assert!(
            j.contains(r#""sources":[{"span":[8,14],"source":null,"text":"rm x"}]"#),
            "{j}"
        );
        assert!(j.contains(r#""via":"shell-string""#), "{j}");
        assert!(
            j.contains(r#"{"path":"x","span":[3,4],"source":0,"command":1}"#),
            "{j}"
        );
        assert!(
            j.contains(r#"{"path":"log","span":[17,20],"source":null,"command":0}"#),
            "{j}"
        );
    }

    #[test]
    fn errors_and_unknown() {
        let j = json("if then");
        assert!(j.starts_with(r#"{"ok":false,"error":{"message":"#), "{j}");
        assert!(j.contains(r#""primary":"unknown""#), "{j}");
        let j = json("$x");
        assert!(
            j.contains(r#""reason":"the command name is dynamic""#),
            "{j}"
        );
    }
}
