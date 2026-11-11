use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const RESET: &str = "\x1b[0m";
const ITALIC: &str = "\x1b[3m";
const UNDERLINE: &str = "\x1b[4m";
const STRIKE: &str = "\x1b[9m";
const CODE: &str = "\x1b[36m";
const BULLET: &str = "• ";
const QUOTE: &str = "│ ";
const RULE: char = '─';
const FENCE_CHARS: [char; 2] = ['`', '~'];
const FENCE_MIN: usize = 3;

#[derive(Default)]
pub struct Markdown {
    fence: Option<String>,
}

impl Markdown {
    pub fn line(&mut self, line: &str, cols: usize) -> String {
        let body = line.trim_start();
        let indent = &line[..line.len() - body.len()];
        if let Some(fence) = &self.fence {
            if body.starts_with(fence.as_str())
                && body.trim_start_matches(fence.as_str()).trim().is_empty()
            {
                self.fence = None;
                return format!("{DIM}{line}{RESET}");
            }
            return format!("{CODE}{line}{RESET}");
        }
        if let Some(fence) = opening_fence(body) {
            self.fence = Some(fence.to_owned());
            return format!("{DIM}{line}{RESET}");
        }
        format!("{indent}{}", inline(body, cols))
    }

    pub fn reset(&mut self) {
        self.fence = None;
    }
}

fn opening_fence(body: &str) -> Option<&str> {
    let first = body.chars().next().filter(|c| FENCE_CHARS.contains(c))?;
    let run = body.len() - body.trim_start_matches(first).len();
    (run >= FENCE_MIN).then(|| &body[..run])
}

fn inline(body: &str, cols: usize) -> String {
    let mut out = String::with_capacity(body.len());
    let mut styles: Vec<&str> = Vec::new();
    let mut links: Vec<Option<String>> = Vec::new();
    let mut number: Option<u64> = None;
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(body, options) {
        match event {
            Event::Start(tag) => {
                let style = match tag {
                    Tag::Heading { .. } | Tag::Strong => BOLD,
                    Tag::Emphasis => ITALIC,
                    Tag::Strikethrough => STRIKE,
                    Tag::CodeBlock(_) => CODE,
                    Tag::BlockQuote(_) => {
                        out.push_str(DIM);
                        out.push_str(QUOTE);
                        DIM
                    }
                    Tag::List(start) => {
                        number = start;
                        continue;
                    }
                    Tag::Item => {
                        match number {
                            Some(n) => out.push_str(&format!("{n}. ")),
                            None => out.push_str(BULLET),
                        }
                        continue;
                    }
                    Tag::Link {
                        link_type,
                        dest_url,
                        ..
                    }
                    | Tag::Image {
                        link_type,
                        dest_url,
                        ..
                    } => {
                        let shown = !matches!(link_type, LinkType::Autolink | LinkType::Email);
                        links.push(shown.then(|| dest_url.into_string()));
                        UNDERLINE
                    }
                    _ => continue,
                };
                styles.push(style);
                out.push_str(style);
            }
            Event::End(tag) => {
                if !matches!(
                    tag,
                    TagEnd::Heading(_)
                        | TagEnd::Strong
                        | TagEnd::Emphasis
                        | TagEnd::Strikethrough
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_)
                        | TagEnd::Link
                        | TagEnd::Image
                ) {
                    continue;
                }
                styles.pop();
                restore(&mut out, &styles);
                if matches!(tag, TagEnd::Link | TagEnd::Image)
                    && let Some(Some(url)) = links.pop()
                {
                    out.push_str(&format!("{DIM} ({url}){RESET}"));
                    restore_styles(&mut out, &styles);
                }
            }
            Event::Code(code) => {
                out.push_str(CODE);
                out.push_str(&code);
                restore(&mut out, &styles);
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                out.push_str(text.trim_end_matches('\n'));
            }
            Event::TaskListMarker(done) => out.push_str(if done { "[x] " } else { "[ ] " }),
            Event::Rule => out.extend(std::iter::repeat_n(RULE, cols)),
            _ => {}
        }
    }
    out.push_str(RESET);
    out
}

fn restore(out: &mut String, styles: &[&str]) {
    out.push_str(RESET);
    restore_styles(out, styles);
}

fn restore_styles(out: &mut String, styles: &[&str]) {
    styles.iter().for_each(|style| out.push_str(style));
}
