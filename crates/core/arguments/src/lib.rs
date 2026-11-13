#![forbid(unsafe_code)]

mod error;
mod help;
mod matches;
mod parser;
mod similar;
mod terminal;

use std::ffi::OsString;

pub use error::Error;
pub use matches::Matches;

#[derive(Clone, Copy, Debug)]
pub struct Command {
    name: &'static str,
    about: Option<&'static str>,
    long_about: Option<&'static str>,
    version: Option<&'static str>,
    aliases: &'static [Alias],
    hidden: bool,
    subcommand_required: bool,
    help_subcommand: bool,
    help_flag: bool,
    groups: &'static [Group],
    subcommands: &'static [Self],
}

#[derive(Clone, Copy, Debug)]
pub struct Alias {
    name: &'static str,
    visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Group {
    name: Option<&'static str>,
    arguments: &'static [Argument],
}

#[derive(Clone, Copy, Debug)]
pub struct Argument {
    id: &'static str,
    long: Option<&'static str>,
    short: Option<char>,
    value_name: &'static str,
    kind: Kind,
    env: Option<&'static str>,
    help: Option<&'static str>,
    default: Option<&'static str>,
    hidden: bool,
    hide_env_values: bool,
    global: bool,
    required: bool,
    required_unless: Option<&'static str>,
    conflicts: &'static [&'static str],
    requires: Option<&'static str>,
    hyphen_values: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Parser {
    Text,
    NonEmptyText,
    Path,
    U64,
    U128,
    SocketAddress,
    Choice(&'static [Choice]),
    OneOf(&'static [Choice]),
}

#[derive(Clone, Copy, Debug)]
pub struct Choice {
    name: &'static str,
    help: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Flag,
    Value(Parser),
    OptionalValue(Parser),
    Repeated(Parser),
    Positional(Parser),
    Trailing,
    Help,
    Version,
}

impl Command {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            about: None,
            long_about: None,
            version: None,
            aliases: &[],
            hidden: false,
            subcommand_required: false,
            help_subcommand: true,
            help_flag: true,
            groups: &[],
            subcommands: &[],
        }
    }

    #[must_use]
    pub const fn about(self, about: &'static str) -> Self {
        Self {
            about: Some(about),
            ..self
        }
    }

    #[must_use]
    pub const fn long_about(self, long_about: &'static str) -> Self {
        Self {
            long_about: Some(long_about),
            ..self
        }
    }

    #[must_use]
    pub const fn version(self, version: &'static str) -> Self {
        Self {
            version: Some(version),
            ..self
        }
    }

    #[must_use]
    pub const fn aliases(self, aliases: &'static [Alias]) -> Self {
        Self { aliases, ..self }
    }

    #[must_use]
    pub const fn hidden(self) -> Self {
        Self {
            hidden: true,
            ..self
        }
    }

    #[must_use]
    pub const fn subcommand_required(self) -> Self {
        Self {
            subcommand_required: true,
            ..self
        }
    }

    #[must_use]
    pub const fn without_help_subcommand(self) -> Self {
        Self {
            help_subcommand: false,
            ..self
        }
    }

    const fn without_help_flag(self) -> Self {
        Self {
            help_flag: false,
            ..self
        }
    }

    #[must_use]
    pub const fn groups(self, groups: &'static [Group]) -> Self {
        Self { groups, ..self }
    }

    #[must_use]
    pub const fn subcommands(self, subcommands: &'static [Self]) -> Self {
        Self {
            subcommands,
            ..self
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn arguments(&self) -> impl Iterator<Item = &'static Argument> + use<> {
        self.groups.iter().flat_map(|group| group.arguments.iter())
    }

    pub fn parse_from<I, T>(&'static self, arguments: I) -> Result<Matches, Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        parser::parse(self, arguments.into_iter().map(Into::into).collect())
    }

    pub fn parse(&'static self) -> Matches {
        match self.parse_from(std::env::args_os()) {
            Ok(matches) => matches,
            Err(error) => error.exit(),
        }
    }
}

impl Alias {
    #[must_use]
    pub const fn visible(name: &'static str) -> Self {
        Self {
            name,
            visible: true,
        }
    }

    #[must_use]
    pub const fn hidden(name: &'static str) -> Self {
        Self {
            name,
            visible: false,
        }
    }
}

impl Group {
    #[must_use]
    pub const fn new(arguments: &'static [Argument]) -> Self {
        Self {
            name: None,
            arguments,
        }
    }

    #[must_use]
    pub const fn named(name: &'static str, arguments: &'static [Argument]) -> Self {
        Self {
            name: Some(name),
            arguments,
        }
    }

    pub fn arguments(&self) -> impl Iterator<Item = &'static Argument> + use<> {
        self.arguments.iter()
    }
}

impl Choice {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self { name, help: None }
    }

    #[must_use]
    pub const fn help(self, help: &'static str) -> Self {
        Self {
            help: Some(help),
            ..self
        }
    }
}

impl Argument {
    const fn with_kind(id: &'static str, value_name: &'static str, kind: Kind) -> Self {
        Self {
            id,
            long: None,
            short: None,
            value_name,
            kind,
            env: None,
            help: None,
            default: None,
            hidden: false,
            hide_env_values: false,
            global: false,
            required: false,
            required_unless: None,
            conflicts: &[],
            requires: None,
            hyphen_values: false,
        }
    }

    #[must_use]
    pub const fn flag(id: &'static str) -> Self {
        Self::with_kind(id, "", Kind::Flag).default("false")
    }

    #[must_use]
    pub const fn value(id: &'static str, value_name: &'static str, parser: Parser) -> Self {
        Self::with_kind(id, value_name, Kind::Value(parser))
    }

    #[must_use]
    pub const fn optional_value(
        id: &'static str,
        value_name: &'static str,
        parser: Parser,
    ) -> Self {
        Self::with_kind(id, value_name, Kind::OptionalValue(parser))
    }

    #[must_use]
    pub const fn repeated(id: &'static str, value_name: &'static str, parser: Parser) -> Self {
        Self::with_kind(id, value_name, Kind::Repeated(parser))
    }

    #[must_use]
    pub const fn positional(id: &'static str, value_name: &'static str, parser: Parser) -> Self {
        Self::with_kind(id, value_name, Kind::Positional(parser))
    }

    #[must_use]
    pub const fn trailing(id: &'static str, value_name: &'static str) -> Self {
        Self::with_kind(id, value_name, Kind::Trailing)
    }

    #[must_use]
    pub const fn long(self, long: &'static str) -> Self {
        Self {
            long: Some(long),
            ..self
        }
    }

    #[must_use]
    pub const fn short(self, short: char) -> Self {
        Self {
            short: Some(short),
            ..self
        }
    }

    #[must_use]
    pub const fn env(self, env: &'static str) -> Self {
        Self {
            env: Some(env),
            ..self
        }
    }

    #[must_use]
    pub const fn help(self, help: &'static str) -> Self {
        Self {
            help: Some(help),
            ..self
        }
    }

    #[must_use]
    pub const fn default(self, default: &'static str) -> Self {
        Self {
            default: Some(default),
            ..self
        }
    }

    #[must_use]
    pub const fn hidden(self) -> Self {
        Self {
            hidden: true,
            ..self
        }
    }

    #[must_use]
    pub const fn hide_env_values(self) -> Self {
        Self {
            hide_env_values: true,
            ..self
        }
    }

    #[must_use]
    pub const fn global(self) -> Self {
        Self {
            global: true,
            ..self
        }
    }

    #[must_use]
    pub const fn required(self) -> Self {
        Self {
            required: true,
            ..self
        }
    }

    #[must_use]
    pub const fn required_unless(self, id: &'static str) -> Self {
        Self {
            required_unless: Some(id),
            ..self
        }
    }

    #[must_use]
    pub const fn conflicts(self, ids: &'static [&'static str]) -> Self {
        Self {
            conflicts: ids,
            ..self
        }
    }

    #[must_use]
    pub const fn requires(self, id: &'static str) -> Self {
        Self {
            requires: Some(id),
            ..self
        }
    }

    #[must_use]
    pub const fn allow_hyphen_values(self) -> Self {
        Self {
            hyphen_values: true,
            ..self
        }
    }

    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    #[must_use]
    pub const fn get_long(&self) -> Option<&'static str> {
        self.long
    }

    #[must_use]
    pub const fn get_short(&self) -> Option<char> {
        self.short
    }

    #[must_use]
    pub const fn takes_value(&self) -> bool {
        self.values_max() != 0
    }

    #[must_use]
    pub const fn requires_value(&self) -> bool {
        self.values_min() > 0
    }

    const fn values_min(&self) -> usize {
        match self.kind {
            Kind::Value(_) | Kind::Repeated(_) | Kind::Positional(_) | Kind::Trailing => 1,
            Kind::Flag | Kind::OptionalValue(_) | Kind::Help | Kind::Version => 0,
        }
    }

    const fn values_max(&self) -> usize {
        match self.kind {
            Kind::Value(_) | Kind::Repeated(_) | Kind::Positional(_) | Kind::OptionalValue(_) => 1,
            Kind::Trailing => usize::MAX,
            Kind::Flag | Kind::Help | Kind::Version => 0,
        }
    }

    const fn is_positional(&self) -> bool {
        matches!(self.kind, Kind::Positional(_) | Kind::Trailing)
    }

    const fn is_trailing(&self) -> bool {
        matches!(self.kind, Kind::Trailing)
    }

    const fn takes_hyphen_values(&self) -> bool {
        self.hyphen_values || self.is_trailing()
    }

    const fn appends(&self) -> bool {
        matches!(self.kind, Kind::Repeated(_) | Kind::Trailing)
    }

    const fn multiple_values(&self) -> bool {
        self.values_min() != self.values_max()
    }

    const fn is_multiple(&self) -> bool {
        self.multiple_values() || self.appends()
    }

    const fn choices(&self) -> &'static [Choice] {
        match self.kind {
            Kind::Value(Parser::Choice(choices) | Parser::OneOf(choices))
            | Kind::OptionalValue(Parser::Choice(choices) | Parser::OneOf(choices))
            | Kind::Repeated(Parser::Choice(choices) | Parser::OneOf(choices))
            | Kind::Positional(Parser::Choice(choices) | Parser::OneOf(choices)) => choices,
            _ => &[],
        }
    }
}
