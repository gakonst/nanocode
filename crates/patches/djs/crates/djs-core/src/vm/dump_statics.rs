//! The class text a plain dump writes runs no static code again (0209, D2950). A plain
//! dump makes a class by evaluating its text, which would run its static blocks and
//! static field initializers a second time; their effects are in the dumped heap, and
//! the dump writes them after the class (static values as patches, deleted members by
//! `$djs.keep`). So the text is written with that code blanked by spaces (newlines kept,
//! offsets unchanged): `static {   }`, `static n    ;`. A static private field (or
//! auto-accessor) keeps a literal initializer, because only the class's code can set it
//! (D2883); any other is refused. (Computed keys run at evaluation too: the exporter
//! refuses any but stable ones already, `Dumper::computed_keys`.)
//! `Function.prototype.toString` of such a class shows the blanks after a load.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::compiler::{ClassSpan, ClassSpanKind};

impl Dumper<'_> {
    /// Computes the text a plain dump writes for every class it holds that has static
    /// code ([`Dumper::fn_source`] returns it).
    pub(super) fn blank_class_statics(&mut self) {
        let h = self.h;
        for n in 0..self.nodes.len() {
            if !self.is_class_node(n) {
                continue;
            }
            let o = self.nodes[n].v.as_object().expect("a class");
            let Some(src) = self.sources.fn_text(h, o) else {
                continue;
            };
            if !src.contains("static") {
                continue;
            }
            let t = h.closure_template(o);
            let bits = template::debug_info(h, t).script.get().map_or(0, |s| {
                h.arena().get::<u32>(s, crate::heap::cells::Script::OPTIONS)
            });
            let Some(spans) = self.class_spans(&src, bits) else {
                let p = self.nodes[n].path.clone();
                self.refuse(p, "internal: the class's text does not compile alone");
                continue;
            };
            if spans.is_empty() {
                continue;
            }
            let mut out = String::with_capacity(src.len());
            let mut pos = 0;
            for ClassSpan { start, end, kind } in spans {
                let (start, end) = (start as usize, end as usize);
                let Some(part) = src.get(start..end) else {
                    continue;
                };
                let blank = match kind {
                    ClassSpanKind::StaticBlock | ClassSpanKind::StaticInit => true,
                    ClassSpanKind::StaticPrivateInit => {
                        if literal(part.trim_start().strip_prefix('=').unwrap_or("")) {
                            false
                        } else {
                            let p = self.nodes[n].path.clone();
                            self.refuse(
                                p,
                                "plain dumps: a class's static private field whose initializer is not a literal (it would run again, and only the class's code can set the field, D2950)",
                            );
                            false
                        }
                    }
                };
                if blank && start >= pos {
                    out.push_str(&src[pos..start]);
                    for c in part.chars() {
                        out.push(if c == '\n' || c == '\r' { c } else { ' ' });
                    }
                    pos = end;
                }
            }
            out.push_str(&src[pos..]);
            if out != src && !self.class_texts.iter().any(|c| c.0 == src) {
                self.class_texts.push_out((src, out));
            }
        }
    }

    /// The spans of `src`'s static code, in its offsets.
    fn class_spans(&mut self, src: &str, bits: u32) -> Option<Vec<ClassSpan>> {
        let scratch = self.scratch_heap()?;
        // An anonymous class's text is an expression only.
        let text = cat!("(", src, ")");
        let opts = crate::compiler::CompileOptions::from_bits("dump.js", &[], bits);
        let spans = crate::compiler::class_spans(scratch, text.as_bytes(), &opts).ok()?;
        Some(
            spans
                .into_iter()
                .map(|s| ClassSpan {
                    start: s.start - 1,
                    end: s.end - 1,
                    ..s
                })
                .collect(),
        )
    }
}

/// A literal the dump can keep as written: a number, a simple string, `true`, `false`,
/// `null`, `undefined` (evaluating it has no effect).
fn literal(t: &str) -> bool {
    let t = t.trim().trim_end_matches(';').trim_end();
    matches!(t, "true" | "false" | "null" | "undefined")
        || {
            let d = t.strip_prefix('-').unwrap_or(t);
            !d.is_empty()
                && d.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                && d.parse::<f64>().is_ok()
        }
        || (t.len() >= 2
            && (t.starts_with('"') && t.ends_with('"') || t.starts_with('\'') && t.ends_with('\''))
            && !t[1..t.len() - 1].contains(['"', '\'', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        for t in [
            "1",
            "2.5",
            "-1",
            "\"a\"",
            "'b'",
            "true",
            "null",
            "undefined",
            " 0 ",
        ] {
            assert!(literal(t), "{t}");
        }
        for t in ["f()", "a", "\"a\" + b", "1e3", "--1", "`x`", ""] {
            assert!(!literal(t), "{t}");
        }
    }
}
