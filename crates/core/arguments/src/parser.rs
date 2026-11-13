use std::{
    ffi::{OsStr, OsString},
    fmt::Display,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use crate::{
    Argument, Command, Group, Kind, Parser,
    error::{self, Error},
    help,
    matches::{Matched, Matches, Source, Value},
    similar, terminal,
};

pub(crate) const HELP_NAME: &str = "help";
pub(crate) const ORDER_UNSET: usize = 999;
const ESCAPE: &str = "--";

static HELP_FLAG: Argument = Argument::with_kind("help", "", Kind::Help)
    .short('h')
    .long("help")
    .help("Print help");
static VERSION_FLAG: Argument = Argument::with_kind("version", "", Kind::Version)
    .short('V')
    .long("version")
    .help("Print version");
static HELP_COMMAND: Command = Command::new(HELP_NAME)
    .about("Print this message or the help of the given subcommand(s)")
    .without_help_flag()
    .without_help_subcommand()
    .groups(&[Group::new(&[
        Argument::trailing("subcommand", "COMMAND").help("Print help for the subcommand(s)")
    ])]);

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) argument: &'static Argument,
    pub(crate) order: usize,
    pub(crate) index: Option<usize>,
    pub(crate) group: Option<&'static str>,
    pub(crate) environment: Option<OsString>,
}

pub(crate) struct Level {
    pub(crate) command: &'static Command,
    pub(crate) bin_name: Option<String>,
    pub(crate) usage_name: Option<String>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) subcommands: Vec<(usize, &'static Command)>,
    pub(crate) long_help: bool,
}

impl Level {
    fn build(
        command: &'static Command,
        bin_name: Option<String>,
        usage_name: Option<String>,
        inherited: Vec<Entry>,
    ) -> Self {
        let mut entries = Vec::new();
        let mut order = 0;
        let mut index = 0;
        for group in command.groups {
            for argument in group.arguments {
                let (entry_order, entry_index) = if argument.is_positional() {
                    index += 1;
                    (ORDER_UNSET, Some(index))
                } else {
                    order += 1;
                    (order - 1, None)
                };
                entries.push(Entry {
                    argument,
                    order: entry_order,
                    index: entry_index,
                    group: group.name,
                    environment: argument.env.and_then(std::env::var_os),
                });
            }
        }
        assert!(
            index <= 1,
            "{} takes at most one positional argument",
            command.name
        );
        for entry in inherited {
            if !entries
                .iter()
                .any(|own| own.argument.id == entry.argument.id)
            {
                entries.push(Entry {
                    group: None,
                    ..entry
                });
            }
        }
        let long_help = command.long_about.is_some()
            || entries.iter().any(|entry| {
                !entry.argument.hidden
                    && entry
                        .argument
                        .choices()
                        .iter()
                        .any(|choice| choice.help.is_some())
            });
        let builtins = [
            command.help_flag.then_some(&HELP_FLAG),
            command.version.and(Some(&VERSION_FLAG)),
        ];
        entries.extend(builtins.into_iter().flatten().map(|argument| Entry {
            argument,
            order: ORDER_UNSET,
            index: None,
            group: None,
            environment: None,
        }));
        let mut subcommands: Vec<(usize, &'static Command)> =
            command.subcommands.iter().enumerate().collect();
        if command.help_subcommand && !subcommands.is_empty() {
            subcommands.push((ORDER_UNSET, &HELP_COMMAND));
        }
        Self {
            command,
            bin_name,
            usage_name,
            entries,
            subcommands,
            long_help,
        }
    }

    pub(crate) fn child(&self, command: &'static Command) -> Self {
        let mut middle = String::from(" ");
        for usage in help::required_usage(self, &self.required_graph(), &[], None) {
            middle.push_str(&terminal::strip(&usage));
            middle.push(' ');
        }
        let (bin_name, usage_name) = match &self.bin_name {
            Some(bin_name) => (
                format!("{bin_name} {}", command.name),
                format!("{bin_name}{middle}{}", command.name),
            ),
            None => (command.name.to_owned(), command.name.to_owned()),
        };
        let inherited = if command.name == HELP_NAME && self.command.help_subcommand {
            Vec::new()
        } else {
            self.entries
                .iter()
                .filter(|entry| entry.argument.global)
                .cloned()
                .collect()
        };
        Self::build(command, Some(bin_name), Some(usage_name), inherited)
    }

    pub(crate) fn bin_name_fallback(&self) -> &str {
        self.bin_name.as_deref().unwrap_or(self.command.name)
    }

    pub(crate) fn usage_name_fallback(&self) -> &str {
        self.usage_name
            .as_deref()
            .unwrap_or_else(|| self.bin_name_fallback())
    }

    pub(crate) fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.argument.id == id)
    }

    pub(crate) fn is_group(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.group == Some(id))
    }

    pub(crate) fn group_members(&self, group: &str) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |entry| entry.group == Some(group))
    }

    fn find_long(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.argument.long == Some(name))
    }

    fn find_short(&self, short: char) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.argument.short == Some(short))
    }

    fn positional(&self, position: usize) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.index == Some(position))
    }

    pub(crate) fn has_positionals(&self) -> bool {
        self.entries.iter().any(|entry| entry.index.is_some())
    }

    pub(crate) fn has_visible_subcommands(&self) -> bool {
        self.subcommands
            .iter()
            .any(|(_, command)| command.name != HELP_NAME && !command.hidden)
    }

    fn longs(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter_map(|entry| entry.argument.long)
            .collect()
    }

    fn find_subcommand(&self, name: &OsStr) -> Option<&'static Command> {
        self.subcommands
            .iter()
            .map(|(_, command)| *command)
            .find(|command| {
                name == command.name || command.aliases.iter().any(|alias| name == alias.name)
            })
    }

    fn subcommand_names(&self) -> Vec<&'static str> {
        self.subcommands
            .iter()
            .flat_map(|(_, command)| {
                std::iter::once(command.name).chain(command.aliases.iter().map(|alias| alias.name))
            })
            .collect()
    }

    pub(crate) fn required_graph(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter(|entry| entry.argument.required)
            .map(|entry| entry.argument.id)
            .collect()
    }

    pub(crate) const fn help_flag(&self) -> Option<&'static str> {
        if self.command.help_flag {
            Some("--help")
        } else if !self.subcommands.is_empty() && self.command.help_subcommand {
            Some(HELP_NAME)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ident {
    Short,
    Long,
    Index,
}

struct Pending {
    entry: usize,
    ident: Option<Ident>,
    values: Vec<OsString>,
}

#[derive(Clone, Copy)]
enum State {
    Done,
    Option(usize),
    Positional(usize),
}

enum Step {
    Done,
    Option(usize),
    Hyphen,
    Unknown(String),
    Unneeded {
        rest: String,
        used: Vec<&'static str>,
        argument: String,
    },
}

struct Run<'a> {
    level: &'a Level,
    matches: Matches,
    pending: Option<Pending>,
}

pub(crate) fn parse(command: &'static Command, raw: Vec<OsString>) -> Result<Matches, Error> {
    let bin_name = raw
        .first()
        .and_then(|first| Path::new(first).file_name())
        .and_then(OsStr::to_str)
        .map(str::to_owned);
    let level = Level::build(command, bin_name, None, Vec::new());
    let mut cursor = usize::from(!raw.is_empty());
    let mut matches = parse_level(&level, &raw, &mut cursor)?;
    let mut globals = Vec::new();
    collect_globals(command, &matches, &mut globals);
    fill_globals(&globals, &mut matches, &mut Vec::new());
    Ok(matches)
}

fn parse_level(level: &Level, raw: &[OsString], cursor: &mut usize) -> Result<Matches, Error> {
    let mut run = Run {
        level,
        matches: Matches::default(),
        pending: None,
    };
    if let Some(command) = run.arguments(raw, cursor)? {
        let child = level.child(command);
        let matches = parse_level(&child, raw, cursor)?;
        run.matches.subcommand = Some((command.name, Box::new(matches)));
    }
    run.resolve_pending()?;
    run.add_environment()?;
    run.add_defaults()?;
    run.validate()?;
    Ok(run.matches)
}

fn collect_globals(command: &'static Command, matches: &Matches, globals: &mut Vec<&'static str>) {
    globals.extend(
        command
            .arguments()
            .filter(|argument| argument.global)
            .map(|argument| argument.id),
    );
    if let Some((name, matches)) = matches.subcommand()
        && let Some(sub) = command.subcommands.iter().find(|sub| sub.name == name)
    {
        collect_globals(sub, matches, globals);
    }
}

fn fill_globals(globals: &[&'static str], matches: &mut Matches, values: &mut Vec<Matched>) {
    for id in globals {
        let Some(own) = matches.get(id) else {
            continue;
        };
        match values.iter().position(|value| value.id == *id) {
            Some(index) if values[index].source > own.source => {}
            Some(index) => values[index] = own.clone(),
            None => values.push(own.clone()),
        }
    }
    if let Some((_, sub)) = matches.subcommand.as_mut() {
        fill_globals(globals, sub, values);
    }
    for value in values.iter() {
        matches.insert(value.clone());
    }
}

fn help_subcommand(level: &Level, names: &[OsString]) -> Error {
    let mut current: Option<Level> = None;
    for name in names {
        let parent = current.as_ref().unwrap_or(level);
        let Some(command) = parent.find_subcommand(name) else {
            return error::unrecognized_subcommand(
                parent,
                name.to_string_lossy().into_owned(),
                help::usage(parent, None, &[]),
            );
        };
        current = Some(parent.child(command));
    }
    error::help(current.as_ref().unwrap_or(level), true)
}

fn long_parts(argument: &OsStr) -> Option<(Result<&str, String>, Option<usize>)> {
    let bytes = argument.as_encoded_bytes();
    let rest = bytes.strip_prefix(b"--")?;
    if rest.is_empty() {
        return None;
    }
    let equals = rest.iter().position(|byte| *byte == b'=');
    let end = ESCAPE.len() + equals.unwrap_or(rest.len());
    let name = std::str::from_utf8(&bytes[ESCAPE.len()..end])
        .map_err(|_| lossy(argument, ESCAPE.len(), end));
    Some((name, equals.map(|_| end + 1)))
}

#[cfg(windows)]
fn lossy(argument: &OsStr, start: usize, end: usize) -> String {
    let (head, _) =
        split_os(argument, end).expect("Windows arguments split at code point boundaries");
    let (_, middle) =
        split_os(&head, start).expect("Windows arguments split at code point boundaries");
    middle.to_string_lossy().into_owned()
}

#[cfg(not(windows))]
fn lossy(argument: &OsStr, start: usize, end: usize) -> String {
    String::from_utf8_lossy(&argument.as_encoded_bytes()[start..end]).into_owned()
}

fn short_flags(argument: &OsStr) -> Option<(&str, bool)> {
    let bytes = argument.as_encoded_bytes();
    let rest = bytes.strip_prefix(b"-")?;
    if rest.is_empty() || rest.starts_with(b"-") {
        return None;
    }
    Some(match std::str::from_utf8(rest) {
        Ok(text) => (text, false),
        Err(problem) => (
            std::str::from_utf8(&rest[..problem.valid_up_to()])
                .expect("the prefix up to valid_up_to is UTF-8"),
            true,
        ),
    })
}

#[cfg(unix)]
fn split_os(argument: &OsStr, at: usize) -> Option<(OsString, OsString)> {
    use std::os::unix::ffi::OsStrExt;
    let (head, tail) = argument.as_bytes().split_at(at);
    Some((
        OsStr::from_bytes(head).to_owned(),
        OsStr::from_bytes(tail).to_owned(),
    ))
}

#[cfg(windows)]
fn split_os(argument: &OsStr, at: usize) -> Option<(OsString, OsString)> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let head = &argument.as_encoded_bytes()[..at];
    let units = head
        .iter()
        .filter(|byte| !matches!(byte, 0x80..=0xbf))
        .count()
        + head.iter().filter(|byte| **byte >= 0xf0).count();
    let wide: Vec<u16> = argument.encode_wide().collect();
    Some((
        OsString::from_wide(&wide[..units]),
        OsString::from_wide(&wide[units..]),
    ))
}

#[cfg(not(any(unix, windows)))]
fn split_os(argument: &OsStr, at: usize) -> Option<(OsString, OsString)> {
    let (head, tail) = argument.as_encoded_bytes().split_at(at);
    Some((
        OsString::from(std::str::from_utf8(head).ok()?),
        OsString::from(std::str::from_utf8(tail).ok()?),
    ))
}

fn is_long(argument: &OsStr) -> bool {
    argument.as_encoded_bytes().starts_with(b"--") && argument != ESCAPE
}

fn is_short(argument: &OsStr) -> bool {
    let bytes = argument.as_encoded_bytes();
    bytes.starts_with(b"-") && argument != "-" && !bytes.starts_with(b"--")
}

impl Run<'_> {
    fn arguments(
        &mut self,
        raw: &[OsString],
        cursor: &mut usize,
    ) -> Result<Option<&'static Command>, Error> {
        let mut state = State::Done;
        let mut position = 1;
        let mut trailing = false;
        while let Some(argument) = raw.get(*cursor) {
            *cursor += 1;
            if !trailing {
                if matches!(state, State::Done)
                    && let Some(command) = self.level.find_subcommand(argument)
                {
                    if command.name == HELP_NAME && self.level.command.help_subcommand {
                        return Err(help_subcommand(self.level, &raw[*cursor..]));
                    }
                    return Ok(Some(command));
                }
                match self.flag(argument, state, position, trailing, &raw[*cursor..])? {
                    Some(Step::Done) => {
                        state = State::Done;
                        continue;
                    }
                    Some(Step::Option(entry)) => {
                        state = State::Option(entry);
                        continue;
                    }
                    Some(Step::Unknown(_) | Step::Unneeded { .. }) => {
                        unreachable!("flag turns unknown and unneeded arguments into errors")
                    }
                    Some(Step::Hyphen) | None => {}
                }
                if argument == ESCAPE && !self.hyphen_state(state) {
                    trailing = true;
                    continue;
                }
                if let State::Option(entry) = state {
                    self.pending_values(entry, None).push(argument.clone());
                    state = if self.needs_more(entry) {
                        State::Option(entry)
                    } else {
                        State::Done
                    };
                    continue;
                }
            }
            let Some(entry) = self.level.positional(position) else {
                let _ = self.resolve_pending();
                return Err(self.match_error(argument, trailing));
            };
            let positional = self.level.entries[entry].argument;
            if positional.is_trailing() {
                trailing = true;
            }
            if self.pending.as_ref().map(|pending| pending.entry) != Some(entry)
                || !positional.multiple_values()
            {
                self.resolve_pending()?;
            }
            self.pending_values(entry, Some(Ident::Index))
                .push(argument.clone());
            if positional.is_multiple() {
                state = State::Positional(entry);
            } else {
                position += 1;
                state = State::Done;
            }
        }
        Ok(None)
    }

    fn flag(
        &mut self,
        argument: &OsStr,
        state: State,
        position: usize,
        trailing: bool,
        remaining: &[OsString],
    ) -> Result<Option<Step>, Error> {
        if argument == ESCAPE {
            return Ok(None);
        }
        if let Some((name, value)) = long_parts(argument) {
            return match self.long(argument, name, value, state, position)? {
                Step::Unknown(name) => {
                    let _ = self.resolve_pending();
                    Err(self.did_you_mean(&name, remaining, trailing))
                }
                Step::Unneeded {
                    rest,
                    used,
                    argument,
                } => {
                    let _ = self.resolve_pending();
                    Err(error::too_many_values(
                        self.level,
                        rest,
                        argument,
                        help::usage(self.level, None, &used),
                    ))
                }
                step => Ok(Some(step)),
            };
        }
        if let Some((flags, invalid)) = short_flags(argument) {
            return match self.short(argument, flags, invalid, state, position)? {
                Step::Unknown(name) => {
                    let _ = self.resolve_pending();
                    let suggest = !trailing && self.level.has_positionals();
                    Err(error::unknown_argument(
                        self.level,
                        name,
                        None,
                        suggest,
                        help::usage(self.level, None, &[]),
                    ))
                }
                step => Ok(Some(step)),
            };
        }
        Ok(None)
    }

    fn hyphen_state(&self, state: State) -> bool {
        match state {
            State::Option(entry) | State::Positional(entry) => {
                self.level.entries[entry].argument.is_trailing()
            }
            State::Done => false,
        }
    }

    fn hyphen_position(&self, position: usize) -> bool {
        self.level
            .positional(position)
            .is_some_and(|entry| self.level.entries[entry].argument.takes_hyphen_values())
    }

    fn long(
        &mut self,
        argument: &OsStr,
        name: Result<&str, String>,
        value: Option<usize>,
        state: State,
        position: usize,
    ) -> Result<Step, Error> {
        if self.hyphen_state(state) {
            return Ok(Step::Hyphen);
        }
        let name = match name {
            Ok(name) => name,
            Err(lossy) => return Ok(Step::Unknown(lossy)),
        };
        let Some(entry) = self.level.find_long(name) else {
            if self.hyphen_position(position) {
                return Ok(Step::Hyphen);
            }
            return Ok(Step::Unknown(name.to_owned()));
        };
        let found = self.level.entries[entry].argument;
        if found.takes_value() {
            let value = match value {
                Some(at) => Some(self.tail(argument, at)?),
                None => None,
            };
            return self.option_value(Ident::Long, value, entry);
        }
        if let Some(at) = value {
            let required = self.level.required_graph();
            let mut used: Vec<&'static str> = self
                .matches
                .explicit_ids()
                .into_iter()
                .filter(|id| {
                    self.level
                        .find(id)
                        .is_none_or(|entry| !(entry.argument.hidden || required.contains(id)))
                })
                .collect();
            used.push(found.id);
            return Ok(Step::Unneeded {
                rest: lossy(argument, at, argument.len()),
                used,
                argument: help::display(found),
            });
        }
        self.react(Some(Ident::Long), Source::CommandLine, entry, Vec::new())?;
        Ok(Step::Done)
    }

    fn short(
        &mut self,
        argument: &OsStr,
        flags: &str,
        invalid: bool,
        state: State,
        position: usize,
    ) -> Result<Step, Error> {
        if self.hyphen_state(state) {
            return Ok(Step::Hyphen);
        }
        if self.hyphen_position(position)
            && (invalid
                || flags
                    .chars()
                    .any(|short| self.level.find_short(short).is_none()))
        {
            return Ok(Step::Hyphen);
        }
        let mut step = None;
        for (offset, short) in flags.char_indices() {
            let Some(entry) = self.level.find_short(short) else {
                return Ok(Step::Unknown(format!("-{short}")));
            };
            if !self.level.entries[entry].argument.takes_value() {
                self.react(Some(Ident::Short), Source::CommandLine, entry, Vec::new())?;
                step = Some(Step::Done);
                continue;
            }
            let next = 1 + offset + short.len_utf8();
            let value = if next < argument.len() {
                let value = self.tail(argument, next)?;
                match value.as_encoded_bytes().strip_prefix(b"=") {
                    Some(_) => Some(self.tail(&value, 1)?),
                    None => Some(value),
                }
            } else {
                None
            };
            return self.option_value(Ident::Short, value, entry);
        }
        if invalid {
            return Ok(Step::Unknown(format!(
                "-{}",
                lossy(argument, 1 + flags.len(), argument.len())
            )));
        }
        Ok(step.expect("a short cluster holds at least one flag"))
    }

    fn tail(&self, argument: &OsStr, at: usize) -> Result<OsString, Error> {
        split_os(argument, at)
            .map(|(_, tail)| tail)
            .ok_or_else(|| error::invalid_utf8(self.level, help::usage(self.level, None, &[])))
    }

    fn option_value(
        &mut self,
        ident: Ident,
        value: Option<OsString>,
        entry: usize,
    ) -> Result<Step, Error> {
        if let Some(value) = value {
            self.react(Some(ident), Source::CommandLine, entry, vec![value])?;
            return Ok(Step::Done);
        }
        self.resolve_pending()?;
        self.pending_values(entry, Some(ident));
        Ok(Step::Option(entry))
    }

    fn pending_values(&mut self, entry: usize, ident: Option<Ident>) -> &mut Vec<OsString> {
        let pending = self.pending.get_or_insert(Pending {
            entry,
            ident,
            values: Vec::new(),
        });
        assert_eq!(
            pending.entry, entry,
            "one argument collects values at a time"
        );
        &mut pending.values
    }

    fn needs_more(&self, entry: usize) -> bool {
        let pending = self
            .pending
            .as_ref()
            .filter(|pending| pending.entry == entry)
            .map_or(0, |pending| pending.values.len());
        pending < self.level.entries[entry].argument.values_max()
    }

    fn resolve_pending(&mut self) -> Result<(), Error> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        self.react(
            pending.ident,
            Source::CommandLine,
            pending.entry,
            pending.values,
        )
    }

    fn react(
        &mut self,
        ident: Option<Ident>,
        source: Source,
        entry: usize,
        mut values: Vec<OsString>,
    ) -> Result<(), Error> {
        self.resolve_pending()?;
        let argument = self.level.entries[entry].argument;
        if source == Source::CommandLine {
            self.verify_count(argument, &values)?;
        }
        if values.is_empty() && matches!(argument.kind, Kind::OptionalValue(_)) {
            values.push(OsString::new());
        }
        match argument.kind {
            Kind::Help => return Err(error::help(self.level, ident != Some(Ident::Short))),
            Kind::Version => return Err(error::version(self.level)),
            Kind::Flag => {
                if values.is_empty() {
                    values.push(OsString::from("true"));
                }
                self.reject_repeat(argument)?;
            }
            Kind::Value(_) | Kind::OptionalValue(_) | Kind::Positional(_) => {
                self.reject_repeat(argument)?
            }
            Kind::Repeated(_) | Kind::Trailing => {}
        }
        self.start(entry, source);
        for value in values {
            let parsed = parse_value(self.level, argument, &value)?;
            self.matches.push_value(argument.id, parsed);
        }
        Ok(())
    }

    fn verify_count(&self, argument: &Argument, values: &[OsString]) -> Result<(), Error> {
        if argument.values_min() > 0 && values.is_empty() {
            let names: Vec<&str> = argument
                .choices()
                .iter()
                .map(|choice| choice.name)
                .collect();
            return Err(error::invalid_value(
                self.level,
                String::new(),
                &names,
                help::display(argument),
            ));
        }
        assert!(
            values.len() <= argument.values_max(),
            "{} takes at most {} values",
            argument.id,
            argument.values_max()
        );
        assert!(
            argument.multiple_values() || values.len() == argument.values_min(),
            "{} takes exactly {} values",
            argument.id,
            argument.values_min()
        );
        Ok(())
    }

    fn reject_repeat(&mut self, argument: &Argument) -> Result<(), Error> {
        if self.matches.remove(argument.id) {
            let display = help::display(argument);
            return Err(error::argument_conflict(
                self.level,
                display.clone(),
                vec![display],
                help::usage(self.level, None, &[]),
            ));
        }
        Ok(())
    }

    fn start(&mut self, entry: usize, source: Source) {
        let entry = &self.level.entries[entry];
        self.matches.start(entry.argument.id, false, source);
        if source != Source::Default
            && let Some(group) = entry.group
        {
            self.matches.start(group, true, source);
        }
    }

    fn add_environment(&mut self) -> Result<(), Error> {
        for (index, entry) in self.level.entries.iter().enumerate() {
            if self.matches.contains(entry.argument.id) {
                continue;
            }
            if let Some(value) = &entry.environment {
                self.react(None, Source::Environment, index, vec![value.clone()])?;
            }
        }
        Ok(())
    }

    fn add_defaults(&mut self) -> Result<(), Error> {
        for (index, entry) in self.level.entries.iter().enumerate() {
            if let Some(default) = entry.argument.default
                && !self.matches.contains(entry.argument.id)
            {
                self.react(None, Source::Default, index, vec![OsString::from(default)])?;
            }
        }
        Ok(())
    }

    fn did_you_mean(&mut self, name: &str, remaining: &[OsString], trailing: bool) -> Error {
        let suggestion = match similar::did_you_mean(name, self.level.longs()).pop() {
            Some(candidate) => Some((candidate, None)),
            None => self
                .level
                .subcommands
                .iter()
                .filter_map(|(_, command)| {
                    let child = self.level.child(command);
                    let candidate = similar::did_you_mean(name, child.longs()).pop()?;
                    let score = remaining
                        .iter()
                        .position(|argument| argument == command.name)?;
                    Some((score, (candidate, Some(command.name))))
                })
                .min_by_key(|(score, _)| *score)
                .map(|(_, suggestion)| suggestion),
        };
        if let Some((candidate, _)) = &suggestion
            && let Some(entry) = self.level.find_long(candidate)
        {
            self.start(entry, Source::CommandLine);
        }
        let used = self.visible_explicit();
        let suggest_trailing = (suggestion.is_none()
            || self
                .level
                .entries
                .iter()
                .any(|entry| entry.argument.is_trailing()))
            && !trailing
            && self.level.has_positionals();
        error::unknown_argument(
            self.level,
            format!("--{name}"),
            suggestion.map(|(candidate, command)| (format!("--{candidate}"), command)),
            suggest_trailing,
            help::usage(self.level, None, &used),
        )
    }

    fn match_error(&self, argument: &OsStr, trailing: bool) -> Error {
        let display = argument.to_string_lossy().into_owned();
        if trailing && self.level.find_subcommand(argument).is_some() {
            return error::unnecessary_double_dash(
                self.level,
                display,
                help::usage(self.level, None, &[]),
            );
        }
        let suggest =
            !trailing && self.level.has_positionals() && (is_long(argument) || is_short(argument));
        if !self.level.subcommands.is_empty() {
            let candidates = similar::did_you_mean(&display, self.level.subcommand_names());
            if !candidates.is_empty() {
                return error::invalid_subcommand(
                    self.level,
                    display,
                    candidates,
                    self.level.bin_name_fallback().to_owned(),
                    suggest,
                    help::usage(self.level, None, &[]),
                );
            }
            if !self.level.has_positionals() {
                return error::unrecognized_subcommand(
                    self.level,
                    display,
                    help::usage(self.level, None, &[]),
                );
            }
        }
        error::unknown_argument(
            self.level,
            display,
            None,
            suggest,
            help::usage(self.level, None, &[]),
        )
    }

    fn visible_explicit(&self) -> Vec<&'static str> {
        self.matches
            .explicit_ids()
            .into_iter()
            .filter(|id| {
                self.level
                    .find(id)
                    .is_some_and(|entry| !entry.argument.hidden)
            })
            .collect()
    }

    fn validate(&mut self) -> Result<(), Error> {
        if self.matches.subcommand.is_none() && self.level.command.subcommand_required {
            if self.matches.explicit_ids().is_empty() {
                return Err(error::help_on_missing(self.level));
            }
            return Err(error::missing_subcommand(
                self.level,
                self.level.bin_name_fallback().to_owned(),
                self.level.subcommand_names(),
                help::usage(self.level, None, &[]),
            ));
        }
        self.validate_conflicts()?;
        self.validate_required()
    }

    fn conflicts_of(&self, id: &str, explicit: &[&'static str]) -> Vec<&'static str> {
        let direct = |id: &str| {
            self.level
                .find(id)
                .map_or(&[][..], |entry| entry.argument.conflicts)
        };
        let mut conflicts = Vec::new();
        for other in explicit {
            if *other == id {
                continue;
            }
            if direct(id).contains(other) {
                conflicts.push(*other);
            }
            if direct(other).contains(&id) {
                conflicts.push(*other);
            }
        }
        conflicts
    }

    fn validate_conflicts(&self) -> Result<(), Error> {
        let explicit = self.matches.explicit_ids();
        for id in &explicit {
            let Some(former) = self.level.find(id) else {
                continue;
            };
            let mut conflicts = Vec::new();
            for conflict in self.conflicts_of(id, &explicit) {
                if !conflicts.contains(&conflict) {
                    conflicts.push(conflict);
                }
            }
            if conflicts.is_empty() {
                continue;
            }
            let used: Vec<&'static str> = explicit
                .iter()
                .copied()
                .filter(|id| {
                    self.level
                        .find(id)
                        .is_some_and(|entry| !entry.argument.hidden)
                })
                .filter(|id| !conflicts.contains(id))
                .collect();
            let required: Vec<&'static str> = used
                .iter()
                .filter_map(|id| {
                    self.level
                        .find(id)
                        .and_then(|entry| entry.argument.requires)
                })
                .filter(|id| !used.contains(id) && !conflicts.contains(id))
                .chain(used.iter().copied())
                .collect();
            let others = conflicts
                .iter()
                .map(|id| {
                    help::display(
                        self.level
                            .find(id)
                            .expect("conflicts name arguments of this command")
                            .argument,
                    )
                })
                .collect();
            return Err(error::argument_conflict(
                self.level,
                help::display(former.argument),
                others,
                help::usage(self.level, None, &required),
            ));
        }
        Ok(())
    }

    fn validate_required(&self) -> Result<(), Error> {
        let explicit = self.matches.explicit_ids();
        let mut required = self.level.required_graph();
        for id in &explicit {
            for requirement in help::unroll_requires(self.level, id) {
                if !required.contains(&requirement) {
                    required.push(requirement);
                }
            }
        }
        let mut missing = Vec::new();
        for id in &required {
            if !explicit.contains(id)
                && self.level.find(id).is_some()
                && self.conflicts_of(id, &explicit).is_empty()
            {
                missing.push(*id);
            }
        }
        for entry in &self.level.entries {
            if let Some(unless) = entry.argument.required_unless
                && !explicit.contains(&entry.argument.id)
                && !explicit.contains(&unless)
            {
                missing.push(entry.argument.id);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let listed = help::required_usage(self.level, &required, &missing, Some(&self.matches))
            .iter()
            .map(|usage| terminal::strip(usage))
            .collect();
        let mut used = self.visible_explicit();
        used.extend(missing);
        Err(error::missing_required(
            self.level,
            listed,
            help::usage(self.level, Some(&required), &used),
        ))
    }
}

fn parse_value(level: &Level, argument: &Argument, raw: &OsStr) -> Result<Value, Error> {
    match argument.kind {
        Kind::Flag => match raw.to_str() {
            Some("true") => Ok(Value::Flag(true)),
            Some("false") => Ok(Value::Flag(false)),
            _ => Err(error::invalid_value(
                level,
                raw.to_string_lossy().into_owned(),
                &["true", "false"],
                help::display(argument),
            )),
        },
        Kind::Value(parser)
        | Kind::OptionalValue(parser)
        | Kind::Repeated(parser)
        | Kind::Positional(parser) => parse_with(level, argument, parser, raw),
        Kind::Trailing => text(level, raw).map(Value::Text),
        Kind::Help | Kind::Version => unreachable!("help and version take no values"),
    }
}

fn parse_with(
    level: &Level,
    argument: &Argument,
    parser: Parser,
    raw: &OsStr,
) -> Result<Value, Error> {
    let empty = || error::invalid_value(level, String::new(), &[], help::display(argument));
    match parser {
        Parser::Text => text(level, raw).map(Value::Text),
        Parser::NonEmptyText if raw.is_empty() => Err(empty()),
        Parser::NonEmptyText => text(level, raw).map(Value::Text),
        Parser::Path if raw.is_empty() => Err(empty()),
        Parser::Path => Ok(Value::Path(PathBuf::from(raw))),
        Parser::U64 => number(level, argument, raw, str::parse::<u64>, Value::U64),
        Parser::U128 => number(level, argument, raw, str::parse::<u128>, Value::U128),
        Parser::SocketAddress => number(
            level,
            argument,
            raw,
            str::parse::<SocketAddr>,
            Value::Address,
        ),
        Parser::Choice(choices) => {
            let names: Vec<&str> = choices.iter().map(|choice| choice.name).collect();
            let Some(text) = raw.to_str() else {
                return Err(error::invalid_value(
                    level,
                    raw.to_string_lossy().into_owned(),
                    &names,
                    help::display(argument),
                ));
            };
            names
                .iter()
                .position(|name| *name == text)
                .map(Value::Choice)
                .ok_or_else(|| {
                    error::invalid_value(level, text.to_owned(), &names, help::display(argument))
                })
        }
        Parser::OneOf(choices) => {
            let names: Vec<&str> = choices.iter().map(|choice| choice.name).collect();
            let text = text(level, raw)?;
            if names.contains(&text.as_str()) {
                Ok(Value::Text(text))
            } else {
                Err(error::invalid_value(
                    level,
                    text,
                    &names,
                    help::display(argument),
                ))
            }
        }
    }
}

fn text(level: &Level, raw: &OsStr) -> Result<String, Error> {
    raw.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error::invalid_utf8(level, help::usage(level, None, &[])))
}

fn number<T, E: Display>(
    level: &Level,
    argument: &Argument,
    raw: &OsStr,
    parse: impl Fn(&str) -> Result<T, E>,
    wrap: fn(T) -> Value,
) -> Result<Value, Error> {
    let text = text(level, raw)?;
    parse(&text).map(wrap).map_err(|problem| {
        error::value_validation(
            level,
            help::display(argument),
            text.clone(),
            problem.to_string(),
        )
    })
}
