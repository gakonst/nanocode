use std::collections::BTreeMap;

use crate::{
    Argument, Command, Kind,
    matches::Matches,
    parser::{Entry, Level},
    terminal::{HEADER, LITERAL, USAGE, display_width, strip},
};

const TAB: &str = "  ";
const NEXT_LINE_INDENT: &str = "        ";
const SHORT_WIDTH: usize = 4;
const DASH_SPACE_WIDTH: usize = 2;
const HELP_WIDTH_COLUMNS: usize = 100;
const NEXT_LINE_SHARE: f32 = 0.40;
const HELP_SUMMARY: &str = "Print help (see more with '--help')";
const HELP_DETAIL: &str = "Print help (see a summary with '-h')";

pub(crate) fn display(argument: &Argument) -> String {
    strip(&stylized(argument, None))
}

fn stylized(argument: &Argument, required: Option<bool>) -> String {
    let mut out = String::new();
    if let Some(long) = argument.long {
        LITERAL.paint(&mut out, &format!("--{long}"));
    } else if let Some(short) = argument.short {
        LITERAL.paint(&mut out, &format!("-{short}"));
    }
    out.push_str(&suffix(argument, required));
    out
}

fn suffix(argument: &Argument, required: Option<bool>) -> String {
    let mut out = String::new();
    let option = argument.takes_value() && !argument.is_positional();
    let optional = option && argument.values_min() == 0;
    if option {
        out.push_str(if optional { " [" } else { " " });
    }
    if argument.takes_value() || argument.is_positional() {
        let name = argument.value_name;
        if argument.is_positional()
            && (argument.values_min() == 0 || !required.unwrap_or(argument.required))
        {
            out.push_str(&format!("[{name}]"));
        } else {
            out.push_str(&format!("<{name}>"));
        }
        if 1 < argument.values_max() || (argument.is_positional() && argument.appends()) {
            out.push_str("...");
        }
    }
    if optional {
        out.push(']');
    }
    out
}

fn escape(text: &str) -> String {
    if text.is_empty() || text.contains(char::is_whitespace) {
        format!("{text:?}")
    } else {
        text.to_owned()
    }
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn indent(text: &str, trailing: &str) -> String {
    text.replace('\n', &format!("\n{trailing}"))
}

pub(crate) fn unroll_requires(level: &Level, id: &str) -> Vec<&'static str> {
    let mut processed: Vec<&str> = Vec::new();
    let mut stack = vec![id];
    let mut requirements = Vec::new();
    while let Some(current) = stack.pop() {
        if processed.contains(&current) {
            continue;
        }
        processed.push(current);
        if let Some(requirement) = level
            .find(current)
            .and_then(|entry| entry.argument.requires)
        {
            if level
                .find(requirement)
                .is_some_and(|entry| entry.argument.requires.is_some())
            {
                stack.push(requirement);
            }
            requirements.push(requirement);
        }
    }
    requirements
}

fn format_group(level: &Level, group: &str) -> String {
    let members: Vec<String> = level
        .group_members(group)
        .map(|entry| {
            if entry.argument.is_positional() {
                entry.argument.value_name.to_owned()
            } else {
                display(entry.argument)
            }
        })
        .collect();
    format!("<{}>", members.join("|"))
}

struct Requirements {
    options: Vec<String>,
    groups: Vec<String>,
    positionals: Vec<Option<String>>,
    members: Vec<&'static str>,
}

fn requirements(
    level: &Level,
    graph: &[&'static str],
    included: &[&'static str],
    matches: Option<&Matches>,
) -> Requirements {
    let mut wanted = Vec::new();
    for id in graph {
        wanted.extend(unroll_requires(level, id));
        wanted.push(*id);
    }
    wanted.extend_from_slice(included);
    let mut found = Requirements {
        options: Vec::new(),
        groups: Vec::new(),
        positionals: Vec::new(),
        members: Vec::new(),
    };
    for id in &wanted {
        if !level.is_group(id) {
            continue;
        }
        let members: Vec<&'static str> = level
            .group_members(id)
            .map(|entry| entry.argument.id)
            .collect();
        if matches.is_some_and(|matches| members.iter().any(|member| matches.is_explicit(member))) {
            continue;
        }
        push_unique(&mut found.groups, format_group(level, id));
        found.members.extend(members);
    }
    for id in &wanted {
        let Some(entry) = level.find(id) else {
            continue;
        };
        if found.members.contains(id) || matches.is_some_and(|matches| matches.is_explicit(id)) {
            continue;
        }
        let styled = stylized(entry.argument, Some(true));
        match entry.index {
            Some(index) => {
                if found.positionals.len() < index + 1 {
                    found.positionals.resize(index + 1, None);
                }
                found.positionals[index] = Some(styled);
            }
            None => push_unique(&mut found.options, styled),
        }
    }
    found
}

pub(crate) fn required_usage(
    level: &Level,
    graph: &[&'static str],
    included: &[&'static str],
    matches: Option<&Matches>,
) -> Vec<String> {
    let found = requirements(level, graph, included, matches);
    let mut usage = found.options;
    usage.extend(found.groups);
    usage.extend(found.positionals.into_iter().flatten());
    usage
}

fn write_arguments(level: &Level, graph: &[&'static str], used: &[&'static str], out: &mut String) {
    let mut found = requirements(level, graph, used, None);
    for entry in level.entries.iter().filter(|entry| entry.index.is_some()) {
        let index = entry.index.expect("positional entries carry an index");
        if entry.argument.hidden || found.members.contains(&entry.argument.id) {
            continue;
        }
        if found.positionals.len() < index + 1 {
            found.positionals.resize(index + 1, None);
        }
        if found.positionals[index].is_none() {
            found.positionals[index] = Some(stylized(entry.argument, Some(false)));
        }
    }
    for item in found
        .options
        .iter()
        .chain(&found.groups)
        .chain(found.positionals.iter().flatten())
    {
        out.push_str(item);
        out.push(' ');
    }
}

fn needs_options_tag(level: &Level) -> bool {
    level.entries.iter().any(|entry| {
        let argument = entry.argument;
        !argument.is_positional()
            && !matches!(argument.long, Some("help" | "version"))
            && !matches!(argument.kind, Kind::Help | Kind::Version)
            && !argument.hidden
            && !argument.required
    })
}

fn write_usage(
    level: &Level,
    graph: Option<&[&'static str]>,
    used: &[&'static str],
    out: &mut String,
) {
    let own_graph = level.required_graph();
    let graph = graph.unwrap_or(&own_graph);
    let name = level.usage_name_fallback();
    if !name.is_empty() {
        LITERAL.paint(out, name);
        out.push(' ');
    }
    if used.is_empty() && needs_options_tag(level) {
        out.push_str("[OPTIONS] ");
    }
    write_arguments(level, graph, used, out);
    if used.is_empty() {
        if level.has_visible_subcommands() {
            out.push_str(if level.command.subcommand_required {
                "<COMMAND>"
            } else {
                "[COMMAND]"
            });
        }
    } else if level.command.subcommand_required {
        out.push_str("<COMMAND>");
    }
}

pub(crate) fn usage(
    level: &Level,
    graph: Option<&[&'static str]>,
    used: &[&'static str],
) -> String {
    let mut out = String::new();
    USAGE.paint(&mut out, "Usage:");
    out.push(' ');
    write_usage(level, graph, used, &mut out);
    out.trim_end().to_owned()
}

pub(crate) fn help(level: &Level, use_long: bool) -> String {
    let use_long = use_long && level.long_help;
    let mut out = String::new();
    let about = if use_long {
        level.command.long_about.or(level.command.about)
    } else {
        level.command.about
    };
    if let Some(about) = about {
        out.push_str(about);
        out.push('\n');
    }
    out.push('\n');
    USAGE.paint(&mut out, "Usage:");
    out.push(' ');
    let mut usage = String::new();
    write_usage(level, None, &[], &mut usage);
    out.push_str(usage.trim_end());
    if level.entries.iter().any(|entry| !entry.argument.hidden) || level.has_visible_subcommands() {
        out.push_str("\n\n");
        write_sections(level, use_long, &mut out);
    }
    if let Some(newline) = out.find('\n')
        && out[..=newline].trim().is_empty()
    {
        out.drain(..=newline);
    }
    let mut help = out.trim_end().to_owned();
    help.push('\n');
    help
}

fn write_sections(level: &Level, use_long: bool, out: &mut String) {
    let positionals: Vec<&Entry> = level
        .entries
        .iter()
        .filter(|entry| entry.index.is_some() && !entry.argument.hidden)
        .collect();
    let options: Vec<&Entry> = level
        .entries
        .iter()
        .filter(|entry| entry.index.is_none() && !entry.argument.hidden)
        .collect();
    let mut first = true;
    let sections = [
        ("Commands:", level.has_visible_subcommands(), Vec::new()),
        ("Arguments:", !positionals.is_empty(), positionals),
        ("Options:", !options.is_empty(), options),
    ];
    for (heading, shown, entries) in sections {
        if !shown {
            continue;
        }
        if !first {
            out.push_str("\n\n");
        }
        first = false;
        HEADER.paint(out, heading);
        out.push('\n');
        if heading == "Commands:" {
            write_subcommands(level, out);
        } else {
            write_entries(level, &entries, use_long, out);
        }
    }
}

fn sort_key(entry: &Entry) -> (usize, String) {
    if let Some(index) = entry.index {
        return (index, String::new());
    }
    let argument = entry.argument;
    let key = if let Some(short) = argument.short {
        let mut key = short.to_ascii_lowercase().to_string();
        key.push(if short.is_ascii_lowercase() { '0' } else { '1' });
        key
    } else if let Some(long) = argument.long {
        long.to_owned()
    } else {
        format!("{{{}", argument.id)
    };
    (entry.order, key)
}

fn write_entries(level: &Level, entries: &[&Entry], use_long: bool, out: &mut String) {
    let mut longest = 2;
    let mut ordered = BTreeMap::new();
    for entry in entries {
        let width = display_width(&display(entry.argument));
        let width = if entry.argument.long.is_some() {
            width + SHORT_WIDTH
        } else {
            width
        };
        longest = longest.max(width);
        ordered.insert(sort_key(entry), *entry);
    }
    let next_line = entries.iter().any(|entry| {
        use_long
            || wraps(
                display_width(about(level, entry, false))
                    + display_width(&specification(entry, false)),
                longest,
            )
    });
    for (position, entry) in ordered.values().enumerate() {
        if position != 0 {
            out.push('\n');
            if next_line && use_long {
                out.push('\n');
            }
        }
        write_entry(level, entry, next_line, longest, use_long, out);
    }
}

fn wraps(help_width: usize, longest: usize) -> bool {
    let taken = longest + TAB.len() * 2;
    HELP_WIDTH_COLUMNS >= taken
        && (taken as f32 / HELP_WIDTH_COLUMNS as f32) > NEXT_LINE_SHARE
        && help_width > HELP_WIDTH_COLUMNS - taken
}

fn about(level: &Level, entry: &Entry, use_long: bool) -> &'static str {
    match entry.argument.kind {
        Kind::Help if level.long_help && use_long => HELP_DETAIL,
        Kind::Help if level.long_help => HELP_SUMMARY,
        _ => entry.argument.help.unwrap_or(""),
    }
}

fn long_choices(entry: &Entry, use_long: bool) -> bool {
    use_long
        && entry
            .argument
            .choices()
            .iter()
            .any(|choice| choice.help.is_some())
}

fn specification(entry: &Entry, use_long: bool) -> String {
    let argument = entry.argument;
    let mut specification = Vec::new();
    if let Some(name) = argument.env {
        let value = if argument.hide_env_values {
            String::new()
        } else {
            format!(
                "={}",
                entry
                    .environment
                    .as_ref()
                    .map(|value| value.to_string_lossy())
                    .unwrap_or_default()
            )
        };
        specification.push(format!("[env: {name}{value}]"));
    }
    if argument.takes_value()
        && let Some(default) = argument.default
    {
        specification.push(format!("[default: {}]", escape(default)));
    }
    if !long_choices(entry, use_long) && !argument.choices().is_empty() {
        let names: Vec<String> = argument
            .choices()
            .iter()
            .map(|choice| escape(choice.name))
            .collect();
        specification.push(format!("[possible values: {}]", names.join(", ")));
    }
    specification.join(if use_long { "\n" } else { " " })
}

fn write_entry(
    level: &Level,
    entry: &Entry,
    next_line: bool,
    longest: usize,
    use_long: bool,
    out: &mut String,
) {
    let argument = entry.argument;
    out.push_str(TAB);
    if let Some(short) = argument.short {
        LITERAL.paint(out, &format!("-{short}"));
    } else if argument.long.is_some() {
        out.push_str("    ");
    }
    if let Some(long) = argument.long {
        if argument.short.is_some() {
            out.push_str(", ");
        }
        LITERAL.paint(out, &format!("--{long}"));
    }
    out.push_str(&suffix(argument, None));
    if !use_long && !next_line {
        let width = display_width(&display(argument));
        let padding = if !argument.is_positional() && argument.long.is_some() {
            longest + TAB.len() - width - SHORT_WIDTH
        } else {
            longest + TAB.len() - width
        };
        out.push_str(&" ".repeat(padding));
    }
    let specification = specification(entry, use_long);
    write_help(
        Some(entry),
        about(level, entry, use_long),
        &specification,
        next_line,
        longest,
        use_long,
        out,
    );
}

fn write_help(
    entry: Option<&Entry>,
    about: &str,
    specification: &str,
    next_line: bool,
    longest: usize,
    use_long: bool,
    out: &mut String,
) {
    if next_line {
        out.push('\n');
        out.push_str(TAB);
        out.push_str(NEXT_LINE_INDENT);
    }
    let spaces = if next_line {
        TAB.len() + NEXT_LINE_INDENT.len()
    } else {
        longest + TAB.len() * 2
    };
    let trailing = " ".repeat(spaces);
    let mut help = about.to_owned();
    let mut help_empty = help.is_empty();
    let next_line_specification = use_long && entry.is_some();
    if !specification.is_empty() && !next_line_specification {
        if !help_empty {
            help.push(' ');
        }
        help.push_str(specification);
        help_empty = help.is_empty();
    }
    out.push_str(&indent(&help, &trailing));
    let mut has_choices = false;
    if let Some(entry) = entry
        && long_choices(entry, use_long)
    {
        has_choices = true;
        let choices = entry.argument.choices();
        let longest_name = choices
            .iter()
            .map(|choice| display_width(choice.name))
            .max()
            .unwrap_or(0);
        let choice_spaces = " ".repeat(spaces + TAB.len() - DASH_SPACE_WIDTH);
        let choice_trailing = " ".repeat(spaces + TAB.len());
        if !help_empty {
            out.push_str("\n\n");
            out.push_str(&choice_spaces);
        }
        out.push_str("Possible values:");
        for choice in choices {
            let mut description = LITERAL.painted(choice.name);
            if let Some(help) = choice.help {
                description.push_str(": ");
                description.push_str(&" ".repeat(longest_name - display_width(choice.name)));
                description.push_str(help);
            }
            out.push('\n');
            out.push_str(&choice_spaces);
            out.push_str("- ");
            out.push_str(&indent(&description, &choice_trailing));
        }
    }
    if !specification.is_empty() && next_line_specification {
        let mut help = String::new();
        if !help_empty || has_choices {
            help.push_str("\n\n");
        }
        help.push_str(specification);
        out.push_str(&indent(&help, &trailing));
    }
}

fn subcommand_specification(command: &Command) -> String {
    let aliases: Vec<&str> = command
        .aliases
        .iter()
        .filter(|alias| alias.visible)
        .map(|alias| alias.name)
        .collect();
    if aliases.is_empty() {
        return String::new();
    }
    let plural = if aliases.len() == 1 { "" } else { "es" };
    format!("[alias{plural}: {}]", aliases.join(", "))
}

fn write_subcommands(level: &Level, out: &mut String) {
    let mut longest = 2;
    let mut ordered = BTreeMap::new();
    for (order, command) in &level.subcommands {
        if command.hidden {
            continue;
        }
        let styled = LITERAL.painted(command.name);
        longest = longest.max(display_width(&styled));
        ordered.insert((*order, styled), *command);
    }
    let next_line = ordered.values().any(|command| {
        let about = command.about.or(command.long_about).unwrap_or("");
        wraps(
            display_width(about) + display_width(&subcommand_specification(command)),
            longest,
        )
    });
    for (position, ((_, styled), command)) in ordered.iter().enumerate() {
        if position != 0 {
            out.push('\n');
        }
        out.push_str(TAB);
        out.push_str(styled);
        if !next_line {
            out.push_str(&" ".repeat(longest + TAB.len() - display_width(styled)));
        }
        let about = command.about.or(command.long_about).unwrap_or("");
        write_help(
            None,
            about,
            &subcommand_specification(command),
            next_line,
            longest,
            false,
            out,
        );
    }
}
