use std::fmt;

use crate::{
    help,
    parser::Level,
    terminal::{self, ERROR, INVALID, LITERAL, Stream, VALID},
};

const TAB: &str = "  ";
const USAGE_EXIT_CODE: i32 = 2;
const SUCCESS_EXIT_CODE: i32 = 0;

#[derive(Debug)]
pub struct Error {
    stream: Stream,
    styled: String,
}

impl Error {
    #[must_use]
    pub fn use_stderr(&self) -> bool {
        self.stream == Stream::Stderr
    }

    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.use_stderr() {
            USAGE_EXIT_CODE
        } else {
            SUCCESS_EXIT_CODE
        }
    }

    #[must_use]
    pub fn styled(&self) -> &str {
        &self.styled
    }

    pub fn print(&self) -> std::io::Result<()> {
        terminal::write(self.stream, &self.styled)
    }

    pub fn exit(&self) -> ! {
        let _ = self.print();
        std::process::exit(self.exit_code())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&terminal::strip(&self.styled))
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
struct Rich {
    message: String,
    subcommands: Option<Vec<String>>,
    argument: Option<String>,
    value: Option<String>,
    tips: Option<Vec<String>>,
    usage: Option<String>,
}

fn similar(out: &mut String, context: &str, possible: &[String]) {
    out.push_str(TAB);
    VALID.paint(out, "tip:");
    if possible.len() == 1 {
        out.push_str(&format!(" a similar {context} exists: "));
    } else {
        out.push_str(&format!(" some similar {context}s exist: "));
    }
    for (index, name) in possible.iter().enumerate() {
        if index != 0 {
            out.push_str(", ");
        }
        out.push('\'');
        VALID.paint(out, name);
        out.push('\'');
    }
}

fn rich(level: &Level, rich: Rich) -> Error {
    let mut out = String::new();
    ERROR.paint(&mut out, "error:");
    out.push(' ');
    out.push_str(&rich.message);
    let mut suggested = false;
    let similars = [
        ("subcommand", rich.subcommands),
        ("argument", rich.argument.map(|argument| vec![argument])),
        ("value", rich.value.map(|value| vec![value])),
    ];
    for (context, possible) in similars {
        let Some(possible) = possible else {
            continue;
        };
        out.push('\n');
        if !suggested {
            out.push('\n');
            suggested = true;
        }
        similar(&mut out, context, &possible);
    }
    if let Some(tips) = rich.tips {
        if !suggested {
            out.push('\n');
        }
        for tip in tips {
            out.push('\n');
            out.push_str(TAB);
            VALID.paint(&mut out, "tip:");
            out.push(' ');
            out.push_str(&tip);
        }
    }
    if let Some(usage) = rich.usage {
        out.push_str("\n\n");
        out.push_str(&usage);
    }
    match level.help_flag() {
        Some(flag) => {
            out.push_str("\n\nFor more information, try '");
            LITERAL.paint(&mut out, flag);
            out.push_str("'.\n");
        }
        None => out.push('\n'),
    }
    Error {
        stream: Stream::Stderr,
        styled: out,
    }
}

fn quoted(style: terminal::Style, text: &str) -> String {
    format!("'{}'", style.painted(text))
}

fn list(name: &str, values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let painted: Vec<String> = values.iter().map(|value| VALID.painted(value)).collect();
    format!("\n{TAB}[{name}: {}]", painted.join(", "))
}

pub(crate) fn help(level: &Level, use_long: bool) -> Error {
    Error {
        stream: Stream::Stdout,
        styled: help::help(level, use_long),
    }
}

pub(crate) fn help_on_missing(level: &Level) -> Error {
    Error {
        stream: Stream::Stderr,
        styled: help::help(level, false),
    }
}

pub(crate) fn version(level: &Level) -> Error {
    Error {
        stream: Stream::Stdout,
        styled: format!(
            "{} {}\n",
            level.command.name,
            level.command.version.unwrap_or("")
        ),
    }
}

pub(crate) fn argument_conflict(
    level: &Level,
    argument: String,
    mut others: Vec<String>,
    usage: String,
) -> Error {
    let invalid = quoted(INVALID, &argument);
    let message = if others.len() == 1 && others[0] == argument {
        format!("the argument {invalid} cannot be used multiple times")
    } else {
        let prior = match others.len() {
            0 => " one or more of the other specified arguments".to_owned(),
            1 => format!(" {}", quoted(INVALID, &others.remove(0))),
            _ => others.iter().fold(":".to_owned(), |prior, other| {
                format!("{prior}\n{TAB}{}", INVALID.painted(other))
            }),
        };
        format!("the argument {invalid} cannot be used with{prior}")
    };
    rich(
        level,
        Rich {
            message,
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn invalid_value(
    level: &Level,
    value: String,
    valid: &[&str],
    argument: String,
) -> Error {
    let names: Vec<String> = valid.iter().map(|name| escape(name)).collect();
    let suggestion = crate::similar::did_you_mean(&value, valid.iter().copied()).pop();
    let mut message = if value.is_empty() {
        format!(
            "a value is required for {} but none was supplied",
            quoted(INVALID, &argument)
        )
    } else {
        format!(
            "invalid value {} for {}",
            quoted(INVALID, &value),
            quoted(LITERAL, &argument)
        )
    };
    message.push_str(&list("possible values", &names));
    rich(
        level,
        Rich {
            message,
            value: suggestion,
            ..Rich::default()
        },
    )
}

fn escape(text: &str) -> String {
    if text.is_empty() || text.contains(char::is_whitespace) {
        format!("{text:?}")
    } else {
        text.to_owned()
    }
}

pub(crate) fn invalid_subcommand(
    level: &Level,
    name: String,
    candidates: Vec<String>,
    bin_name: String,
    suggest_trailing: bool,
    usage: String,
) -> Error {
    let mut tips = Vec::new();
    if suggest_trailing {
        tips.push(format!(
            "to pass {} as a value, use {}",
            quoted(INVALID, &name),
            quoted(VALID, &format!("{bin_name} -- {name}"))
        ));
    }
    rich(
        level,
        Rich {
            message: format!("unrecognized subcommand {}", quoted(INVALID, &name)),
            subcommands: Some(candidates),
            tips: Some(tips),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn unrecognized_subcommand(level: &Level, name: String, usage: String) -> Error {
    rich(
        level,
        Rich {
            message: format!("unrecognized subcommand {}", quoted(INVALID, &name)),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn missing_required(level: &Level, required: Vec<String>, usage: String) -> Error {
    let mut message = "the following required arguments were not provided:".to_owned();
    for argument in &required {
        message.push_str(&format!("\n{TAB}{}", VALID.painted(argument)));
    }
    rich(
        level,
        Rich {
            message,
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn missing_subcommand(
    level: &Level,
    name: String,
    available: Vec<&str>,
    usage: String,
) -> Error {
    let available: Vec<String> = available.iter().map(|name| escape(name)).collect();
    let mut message = format!(
        "{} requires a subcommand but one was not provided",
        quoted(INVALID, &name)
    );
    message.push_str(&list("subcommands", &available));
    rich(
        level,
        Rich {
            message,
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn invalid_utf8(level: &Level, usage: String) -> Error {
    rich(
        level,
        Rich {
            message: "invalid UTF-8 was detected in one or more arguments".to_owned(),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn too_many_values(
    level: &Level,
    value: String,
    argument: String,
    usage: String,
) -> Error {
    rich(
        level,
        Rich {
            message: format!(
                "unexpected value {} for {} found; no more were expected",
                quoted(INVALID, &value),
                quoted(LITERAL, &argument)
            ),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn value_validation(
    level: &Level,
    argument: String,
    value: String,
    problem: String,
) -> Error {
    rich(
        level,
        Rich {
            message: format!(
                "invalid value {} for {}: {problem}",
                quoted(INVALID, &value),
                quoted(LITERAL, &argument)
            ),
            ..Rich::default()
        },
    )
}

pub(crate) fn unknown_argument(
    level: &Level,
    argument: String,
    suggestion: Option<(String, Option<&str>)>,
    suggest_trailing: bool,
    usage: String,
) -> Error {
    let mut tips = Vec::new();
    if suggest_trailing {
        tips.push(format!(
            "to pass {} as a value, use {}",
            quoted(INVALID, &argument),
            quoted(VALID, &format!("-- {argument}"))
        ));
    }
    let mut similar = None;
    match suggestion {
        Some((flag, Some(command))) => tips.push(format!(
            "{} exists",
            quoted(VALID, &format!("{command} {flag}"))
        )),
        Some((flag, None)) => similar = Some(flag),
        None => {}
    }
    rich(
        level,
        Rich {
            message: format!("unexpected argument {} found", quoted(INVALID, &argument)),
            argument: similar,
            tips: (!tips.is_empty()).then_some(tips),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}

pub(crate) fn unnecessary_double_dash(level: &Level, argument: String, usage: String) -> Error {
    let tip = format!(
        "subcommand {} exists; to use it, remove the {} before it",
        quoted(VALID, &argument),
        quoted(INVALID, "--")
    );
    rich(
        level,
        Rich {
            message: format!("unexpected argument {} found", quoted(INVALID, &argument)),
            tips: Some(vec![tip]),
            usage: Some(usage),
            ..Rich::default()
        },
    )
}
