use std::ffi::{OsStr, OsString};
use std::fmt::Write;

const NEXT_LINE_INDENT_COLUMNS: usize = 10;
const DEFAULT_WIDTH_COLUMNS: usize = 100;
const MIN_WIDTH_COLUMNS: usize = 40;
const MIN_HELP_WIDTH_COLUMNS: usize = 20;
const TABLE_INDENT_COLUMNS: usize = 2;
const TABLE_GAP_COLUMNS: usize = 2;
const NEXT_LINE_LEFT_SHARE: f32 = 0.40;
const SUGGESTION_DISTANCE_DIVISOR: usize = 3;
const SUGGESTION_PREFIX_CHARS_MIN: usize = 3;

pub struct Cli<A: 'static> {
    name: &'static str,
    version: Option<&'static str>,
    about: Option<&'static str>,
    usage: Option<&'static str>,
    after_help: Option<&'static str>,
    opts: &'static [Opt<A>],
    args: &'static [Pos<A>],
}

impl<A: 'static> Cli<A> {
    pub const fn new(name: &'static str) -> Cli<A> {
        Cli {
            name,
            version: None,
            about: None,
            usage: None,
            after_help: None,
            opts: &[],
            args: &[],
        }
    }

    pub const fn version(mut self, version: &'static str) -> Cli<A> {
        self.version = Some(version);
        self
    }

    pub const fn about(mut self, about: &'static str) -> Cli<A> {
        self.about = Some(about);
        self
    }

    pub const fn usage(mut self, usage: &'static str) -> Cli<A> {
        self.usage = Some(usage);
        self
    }

    pub const fn after_help(mut self, text: &'static str) -> Cli<A> {
        self.after_help = Some(text);
        self
    }

    pub const fn opts(mut self, opts: &'static [Opt<A>]) -> Cli<A> {
        self.opts = opts;
        self
    }

    pub const fn args(mut self, args: &'static [Pos<A>]) -> Cli<A> {
        self.args = args;
        self
    }

    fn find_short(&self, c: char) -> Option<&Opt<A>> {
        self.opts
            .iter()
            .find(|opt| opt.id.is_some() && opt.short == Some(c))
    }

    fn find_long(&self, name: &str) -> Option<&Opt<A>> {
        self.opts
            .iter()
            .find(|opt| opt.id.is_some() && opt.long == Some(name))
    }

    fn builtin_short(&self, c: char) -> Option<Builtin> {
        if self.find_short(c).is_some() {
            None
        } else if c == 'h' {
            Some(Builtin::Help)
        } else if c == 'V' && self.version.is_some() {
            Some(Builtin::Version)
        } else {
            None
        }
    }

    fn builtin_long(&self, name: &str) -> Option<Builtin> {
        if self.find_long(name).is_some() {
            None
        } else if name == "help" {
            Some(Builtin::Help)
        } else if name == "version" && self.version.is_some() {
            Some(Builtin::Version)
        } else {
            None
        }
    }

    pub fn parser_from_args(&self, args: Vec<OsString>) -> Parser<'_, A> {
        let prog = self.name.to_string();
        let context = Context::of(self, &prog);
        Parser {
            arguments: Arguments::new(args),
            cli: self,
            prog,
            position_index: 0,
            positions_filled: 0,
            last: Last::Nothing,
            context,
        }
    }

    fn options(&self) -> Vec<OptionInfo> {
        let mut options = Vec::new();
        let mut section = None;
        let mut builtins_added = false;
        for opt in self.opts {
            if opt.id.is_some() {
                options.push(OptionInfo {
                    short: opt.short,
                    long: opt.long,
                    value_name: opt.value,
                    help: opt.help,
                    values: opt.values,
                    section,
                    hidden: opt.hidden,
                });
            } else {
                if !builtins_added {
                    self.push_builtins(&mut options);
                    builtins_added = true;
                }
                section = Some(opt.help);
            }
        }
        if !builtins_added {
            self.push_builtins(&mut options);
        }
        options
    }

    fn push_builtins(&self, options: &mut Vec<OptionInfo>) {
        let builtins = [
            (Builtin::Help, 'h', "help", "Print help"),
            (Builtin::Version, 'V', "version", "Print version"),
        ];
        for (builtin, short, long, help) in builtins {
            let short = Some(short).filter(|&c| self.builtin_short(c) == Some(builtin));
            let long = Some(long).filter(|&name| self.builtin_long(name) == Some(builtin));
            if short.is_some() || long.is_some() {
                options.push(OptionInfo {
                    short,
                    long,
                    value_name: None,
                    help,
                    values: &[],
                    section: None,
                    hidden: false,
                });
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Builtin {
    Help,
    Version,
}

pub struct Opt<A: 'static> {
    id: Option<A>,
    short: Option<char>,
    long: Option<&'static str>,
    value: Option<&'static str>,
    help: &'static str,
    values: &'static [&'static str],
    hidden: bool,
}

impl<A: 'static> Opt<A> {
    pub const fn new(id: A) -> Opt<A> {
        Opt {
            id: Some(id),
            short: None,
            long: None,
            value: None,
            help: "",
            values: &[],
            hidden: false,
        }
    }

    pub const fn section(title: &'static str) -> Opt<A> {
        Opt {
            id: None,
            short: None,
            long: None,
            value: None,
            help: title,
            values: &[],
            hidden: false,
        }
    }

    pub const fn short(mut self, c: char) -> Opt<A> {
        self.short = Some(c);
        self
    }

    pub const fn long(mut self, name: &'static str) -> Opt<A> {
        self.long = Some(name);
        self
    }

    pub const fn value(mut self, name: &'static str) -> Opt<A> {
        self.value = Some(name);
        self
    }

    pub const fn help(mut self, text: &'static str) -> Opt<A> {
        self.help = text;
        self
    }

    pub const fn possible_values(mut self, values: &'static [&'static str]) -> Opt<A> {
        self.values = values;
        self
    }

    pub const fn hidden(mut self) -> Opt<A> {
        self.hidden = true;
        self
    }

    fn display(&self, short: bool) -> String {
        let mut display = match (short, self.short, self.long) {
            (true, Some(c), _) | (false, Some(c), None) => format!("-{c}"),
            (_, _, Some(long)) => format!("--{long}"),
            (_, None, None) => String::new(),
        };
        if let Some(value) = self.value {
            display.push_str(&format!(" <{value}>"));
        }
        display
    }
}

pub struct Pos<A: 'static> {
    id: A,
    name: &'static str,
    help: &'static str,
    required: bool,
}

impl<A: 'static> Pos<A> {
    pub const fn new(id: A, name: &'static str) -> Pos<A> {
        Pos {
            id,
            name,
            help: "",
            required: true,
        }
    }

    pub const fn help(mut self, text: &'static str) -> Pos<A> {
        self.help = text;
        self
    }

    pub const fn optional(mut self) -> Pos<A> {
        self.required = false;
        self
    }

    fn display(&self) -> String {
        if self.required {
            format!("<{}>", self.name)
        } else {
            format!("[{}]", self.name)
        }
    }
}

type Rows = Vec<(String, String)>;

struct OptionInfo {
    short: Option<char>,
    long: Option<&'static str>,
    value_name: Option<&'static str>,
    help: &'static str,
    values: &'static [&'static str],
    section: Option<&'static str>,
    hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Help,
    Version,
    UnknownOption,
    UnexpectedArgument,
    UnexpectedValue,
    MissingValue,
    MissingArgument,
    InvalidValue,
    Custom,
}

#[derive(Debug, Clone)]
struct Context {
    prog: String,
    usage: String,
    help_flag: Option<&'static str>,
}

impl Context {
    fn of<A>(cli: &Cli<A>, prog: &str) -> Context {
        let options = cli.options();
        let help_flag = if options.iter().any(|option| option.long == Some("help")) {
            Some("--help")
        } else if options.iter().any(|option| option.short == Some('h')) {
            Some("-h")
        } else {
            None
        };
        Context {
            prog: prog.to_string(),
            usage: usage(cli, prog),
            help_flag,
        }
    }
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    tip: Option<String>,
    context: Option<Context>,
}

impl Error {
    fn with_kind(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error {
            kind,
            message: message.into(),
            tip: None,
            context: None,
        }
    }

    fn with_tip(mut self, tip: Option<String>) -> Error {
        self.tip = tip;
        self
    }

    fn with_context(mut self, context: &Context) -> Error {
        if self.context.is_none() {
            self.context = Some(context.clone());
        }
        self
    }

    #[cfg(test)]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Help | ErrorKind::Version => 0,
            ErrorKind::UnknownOption
            | ErrorKind::UnexpectedArgument
            | ErrorKind::UnexpectedValue
            | ErrorKind::MissingValue
            | ErrorKind::MissingArgument
            | ErrorKind::InvalidValue
            | ErrorKind::Custom => 2,
        }
    }

    pub fn render(&self) -> String {
        if let ErrorKind::Help | ErrorKind::Version = self.kind {
            return self.message.clone();
        }
        let mut rendered = format!("error: {}\n", self.message);
        if let Some(tip) = &self.tip {
            rendered.push_str(&format!("\n  tip: {tip}\n"));
        }
        if let Some(context) = &self.context {
            rendered.push_str(&format!("\nUsage: {}\n", context.usage));
            if let Some(flag) = context.help_flag {
                rendered.push_str(&format!(
                    "\nFor more information, try '{} {}'.\n",
                    context.prog, flag
                ));
            }
        }
        rendered.truncate(rendered.trim_end().len());
        rendered
    }

    pub fn print(&self) {
        match self.kind {
            ErrorKind::Help | ErrorKind::Version => println!("{}", self.render()),
            ErrorKind::UnknownOption
            | ErrorKind::UnexpectedArgument
            | ErrorKind::UnexpectedValue
            | ErrorKind::MissingValue
            | ErrorKind::MissingArgument
            | ErrorKind::InvalidValue
            | ErrorKind::Custom => eprintln!("{}", self.render()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Default,
    ExplicitOptionValue,
    ShortOptionChain(usize),
    ArgumentPause,
}

enum Param {
    Short(char),
    Long(String),
    Positional,
}

enum ArgumentError {
    MissingValue,
    InvalidUnicode,
}

struct Arguments {
    rest: std::vec::IntoIter<OsString>,
    current: Option<OsString>,
    state: State,
    options_enabled: bool,
}

impl Arguments {
    fn new(args: Vec<OsString>) -> Arguments {
        let mut rest = args.into_iter();
        let current = rest.next();
        Arguments {
            rest,
            current,
            state: State::Default,
            options_enabled: true,
        }
    }

    fn advance(&mut self) -> Option<OsString> {
        self.state = State::Default;
        std::mem::replace(&mut self.current, self.rest.next())
    }

    fn finished(&self) -> bool {
        self.current.is_none()
    }

    fn peek_raw_arg(&self) -> Option<&OsStr> {
        match self.state {
            State::Default | State::ArgumentPause => self.current.as_deref(),
            State::ExplicitOptionValue | State::ShortOptionChain(_) => None,
        }
    }

    fn raw_arg(&mut self) -> Option<OsString> {
        match self.state {
            State::Default | State::ArgumentPause => self.advance(),
            State::ExplicitOptionValue | State::ShortOptionChain(_) => None,
        }
    }

    fn optional_raw_value(&mut self) -> Option<OsString> {
        match self.state {
            State::Default | State::ArgumentPause => None,
            State::ExplicitOptionValue => self.advance(),
            State::ShortOptionChain(position) => {
                let arg = self.advance()?;
                Some(suffix(&arg, position))
            }
        }
    }

    fn raw_value(&mut self) -> Result<OsString, ArgumentError> {
        self.optional_raw_value()
            .or_else(|| self.advance())
            .ok_or(ArgumentError::MissingValue)
    }

    fn param(&mut self) -> Result<Option<Param>, ArgumentError> {
        loop {
            let arg = match self.state {
                State::Default => match self.current.as_deref() {
                    Some(arg) => arg,
                    None => return Ok(None),
                },
                State::ArgumentPause | State::ExplicitOptionValue => {
                    self.advance();
                    continue;
                }
                State::ShortOptionChain(position) => {
                    let arg = self
                        .current
                        .as_deref()
                        .expect("a short option chain has its argument");
                    match char_at(arg, position)? {
                        None => {
                            self.advance();
                            continue;
                        }
                        Some(c) => {
                            let next = position + c.len_utf8();
                            if next >= arg.len() {
                                self.advance();
                            } else {
                                self.state = State::ShortOptionChain(next);
                            }
                            return Ok(Some(Param::Short(c)));
                        }
                    }
                }
            };
            let bytes = arg.as_encoded_bytes();
            if !self.options_enabled {
                return Ok(Some(self.pause()));
            } else if bytes == b"--" {
                self.options_enabled = false;
                self.advance();
                continue;
            } else if bytes.starts_with(b"--") {
                let name = if let Some(equals) = bytes.iter().position(|&b| b == b'=') {
                    let name = std::str::from_utf8(&bytes[..equals + 1])
                        .map_err(|_| ArgumentError::InvalidUnicode)?;
                    let name = name[2..name.len() - 1].to_string();
                    let value = suffix(arg, equals + 1);
                    self.current = Some(value);
                    self.state = State::ExplicitOptionValue;
                    name
                } else {
                    let arg = self.advance().expect("the current argument exists");
                    let mut name = arg
                        .into_string()
                        .map_err(|_| ArgumentError::InvalidUnicode)?;
                    name.drain(..2);
                    name
                };
                return Ok(Some(Param::Long(name)));
            } else if bytes.len() > 1 && bytes[0] == b'-' {
                self.state = State::ShortOptionChain(1);
                continue;
            } else {
                return Ok(Some(self.pause()));
            }
        }
    }

    fn pause(&mut self) -> Param {
        self.state = State::ArgumentPause;
        Param::Positional
    }
}

fn char_at(s: &OsStr, index: usize) -> Result<Option<char>, ArgumentError> {
    const UTF8_CHAR_BYTES_MAX: usize = 4;
    let prefix = match s.as_encoded_bytes().get(index..) {
        Some(bytes) => bytes.get(..UTF8_CHAR_BYTES_MAX).unwrap_or(bytes),
        None => return Ok(None),
    };
    let prefix = match std::str::from_utf8(prefix) {
        Ok(prefix) => prefix,
        Err(error) => match error.valid_up_to() {
            0 => return Err(ArgumentError::InvalidUnicode),
            valid => std::str::from_utf8(&prefix[..valid]).expect("the prefix is valid UTF-8"),
        },
    };
    Ok(prefix.chars().next())
}

fn suffix(s: &OsStr, start: usize) -> OsString {
    let bytes = s.as_encoded_bytes();
    assert!(std::str::from_utf8(&bytes[..start]).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(&bytes[start..]).to_owned()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let prefix = std::str::from_utf8(&bytes[..start]).expect("the prefix is valid UTF-8");
        let wide: Vec<u16> = s.encode_wide().collect();
        OsString::from_wide(&wide[prefix.encode_utf16().count()..])
    }
    #[cfg(not(any(unix, windows)))]
    {
        OsString::from(String::from_utf8_lossy(&bytes[start..]).into_owned())
    }
}

enum Last<'a, A: 'static> {
    Nothing,
    Opt(&'a Opt<A>, bool),
    Pos(&'a Pos<A>),
}

impl<A: 'static> Last<'_, A> {
    fn describe(&self) -> Option<String> {
        match self {
            Last::Opt(opt, short) => Some(opt.display(*short)),
            Last::Pos(pos) => Some(pos.display()),
            Last::Nothing => None,
        }
    }

    fn values(&self) -> &'static [&'static str] {
        match self {
            Last::Opt(opt, _) => opt.values,
            Last::Pos(_) | Last::Nothing => &[],
        }
    }
}

pub struct Parser<'a, A: 'static> {
    arguments: Arguments,
    cli: &'a Cli<A>,
    prog: String,
    position_index: usize,
    positions_filled: usize,
    last: Last<'a, A>,
    context: Context,
}

impl<'a, A: Copy + 'static> Parser<'a, A> {
    pub fn param(&mut self) -> Result<Option<A>, Error> {
        self.last = Last::Nothing;
        match self.arguments.param() {
            Ok(None) => self.finish().map(|()| None),
            Ok(Some(Param::Short(c))) => self.handle_short(c),
            Ok(Some(Param::Long(name))) => self.handle_long(name),
            Ok(Some(Param::Positional)) => self.handle_positional(),
            Err(error) => Err(self.convert_error(error)),
        }
    }

    pub fn value(&mut self) -> Result<String, Error> {
        if self.last.values().is_empty() {
            let value = self
                .arguments
                .raw_value()
                .map_err(|error| self.convert_error(error))?;
            return value
                .into_string()
                .map_err(|_| self.convert_error(ArgumentError::InvalidUnicode));
        }
        let value = self
            .arguments
            .raw_value()
            .map_err(|error| self.convert_error(error))?;
        self.check_possible_value(&value)?;
        Ok(value.into_string().expect("possible values are UTF-8"))
    }

    pub fn raw_value(&mut self) -> Result<OsString, Error> {
        let value = self
            .arguments
            .raw_value()
            .map_err(|error| self.convert_error(error))?;
        self.check_possible_value(&value)?;
        Ok(value)
    }

    pub fn peek_raw_arg(&self) -> Option<&OsStr> {
        self.arguments.peek_raw_arg()
    }

    pub fn raw_arg(&mut self) -> Option<OsString> {
        self.arguments.raw_arg()
    }

    pub fn usage(&self) -> String {
        usage(self.cli, &self.prog)
    }

    pub fn help_text(&self) -> String {
        help_text(self.cli, &self.prog, render_width())
    }

    pub fn error(&self, message: impl Into<String>) -> Error {
        Error::with_kind(ErrorKind::Custom, message).with_context(&self.context)
    }

    pub fn subcommand<B: Copy + 'static>(self, cli: &'a Cli<B>) -> Parser<'a, B> {
        let prog = format!("{} {}", self.prog, cli.name);
        let context = Context::of(cli, &prog);
        Parser {
            arguments: self.arguments,
            cli,
            prog,
            position_index: 0,
            positions_filled: 0,
            last: Last::Nothing,
            context,
        }
    }

    fn handle_short(&mut self, c: char) -> Result<Option<A>, Error> {
        if let Some(opt) = self.cli.find_short(c) {
            return self.found_opt(opt, true);
        }
        match self.cli.builtin_short(c) {
            Some(builtin) => Err(self.builtin(builtin)),
            None => Err(Error::with_kind(
                ErrorKind::UnknownOption,
                format!("unexpected argument '-{c}' found"),
            )
            .with_context(&self.context)),
        }
    }

    fn handle_long(&mut self, name: String) -> Result<Option<A>, Error> {
        if let Some(opt) = self.cli.find_long(&name) {
            return self.found_opt(opt, false);
        }
        if let Some(builtin) = self.cli.builtin_long(&name) {
            return Err(self.builtin(builtin));
        }
        let builtins = ["help", "version"]
            .into_iter()
            .filter(|name| self.cli.builtin_long(name).is_some());
        let candidates = self
            .cli
            .opts
            .iter()
            .filter(|opt| opt.id.is_some() && !opt.hidden)
            .filter_map(|opt| opt.long)
            .chain(builtins);
        let tip = suggest(&name, candidates)
            .map(|similar| format!("a similar option exists: '--{similar}'"));
        Err(Error::with_kind(
            ErrorKind::UnknownOption,
            format!("unexpected argument '--{name}' found"),
        )
        .with_tip(tip)
        .with_context(&self.context))
    }

    fn found_opt(&mut self, opt: &'a Opt<A>, short: bool) -> Result<Option<A>, Error> {
        if opt.value.is_none()
            && !short
            && self.arguments.peek_raw_arg().is_none()
            && !self.arguments.finished()
        {
            let value = self.arguments.optional_raw_value().unwrap_or_default();
            return Err(Error::with_kind(
                ErrorKind::UnexpectedValue,
                format!(
                    "unexpected value '{}' for '{}'",
                    value.to_string_lossy(),
                    opt.display(false)
                ),
            )
            .with_context(&self.context));
        }
        self.last = Last::Opt(opt, short);
        Ok(opt.id)
    }

    fn handle_positional(&mut self) -> Result<Option<A>, Error> {
        if let Some(pos) = self.cli.args.get(self.position_index) {
            self.positions_filled = self.position_index + 1;
            self.position_index += 1;
            self.last = Last::Pos(pos);
            return Ok(Some(pos.id));
        }
        let value = self.arguments.raw_arg().unwrap_or_default();
        Err(Error::with_kind(
            ErrorKind::UnexpectedArgument,
            format!("unexpected argument '{}' found", value.to_string_lossy()),
        )
        .with_context(&self.context))
    }

    fn finish(&self) -> Result<(), Error> {
        let rest = self
            .cli
            .args
            .get(self.positions_filled..)
            .unwrap_or_default();
        match rest.iter().find(|pos| pos.required) {
            Some(pos) => Err(Error::with_kind(
                ErrorKind::MissingArgument,
                format!("the argument '{}' is required", pos.display()),
            )
            .with_context(&self.context)),
            None => Ok(()),
        }
    }

    fn check_possible_value(&self, value: &OsStr) -> Result<(), Error> {
        let values = self.last.values();
        if values.is_empty() || value.to_str().is_some_and(|value| values.contains(&value)) {
            return Ok(());
        }
        let value = value.to_string_lossy();
        let tip = match suggest(&value, values.iter().copied()) {
            Some(similar) => format!("a similar value exists: '{similar}'"),
            None => format!("possible values: {}", values.join(", ")),
        };
        Err(Error::with_kind(
            ErrorKind::InvalidValue,
            format!(
                "invalid value '{}' for '{}'",
                value,
                self.last.describe().unwrap_or_default()
            ),
        )
        .with_tip(Some(tip))
        .with_context(&self.context))
    }

    fn builtin(&self, builtin: Builtin) -> Error {
        match builtin {
            Builtin::Help => Error::with_kind(ErrorKind::Help, self.help_text()),
            Builtin::Version => Error::with_kind(
                ErrorKind::Version,
                format!("{} {}", self.prog, self.cli.version.unwrap_or_default()),
            ),
        }
    }

    fn convert_error(&self, error: ArgumentError) -> Error {
        let what = match self.last.describe() {
            Some(what) => format!("'{what}'"),
            None => "argument".to_string(),
        };
        let (kind, message) = match error {
            ArgumentError::MissingValue => (
                ErrorKind::MissingValue,
                format!("a value is required for {what} but none was supplied"),
            ),
            ArgumentError::InvalidUnicode => (
                ErrorKind::InvalidValue,
                format!("invalid unicode in value for {what}"),
            ),
        };
        Error::with_kind(kind, message).with_context(&self.context)
    }
}

fn render_width() -> usize {
    let detected = std::env::var("COLUMNS")
        .ok()
        .and_then(|columns| columns.parse::<usize>().ok())
        .filter(|&columns| columns > 0)
        .or_else(|| psh_os::stdout_columns().filter(|&columns| columns > 0))
        .unwrap_or(DEFAULT_WIDTH_COLUMNS);
    detected.clamp(MIN_WIDTH_COLUMNS, DEFAULT_WIDTH_COLUMNS)
}

fn usage<A>(cli: &Cli<A>, prog: &str) -> String {
    if let Some(usage) = cli.usage {
        return format!("{prog} {usage}");
    }
    let mut usage = prog.to_string();
    if cli.options().iter().any(|option| !option.hidden) {
        usage.push_str(" [OPTIONS]");
    }
    for pos in cli.args {
        usage.push(' ');
        usage.push_str(&pos.display());
    }
    usage
}

fn option_display(option: &OptionInfo, pad_long: bool) -> String {
    let mut display = match (option.short, option.long) {
        (Some(short), Some(long)) => format!("-{short}, --{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, Some(long)) if pad_long => format!("    --{long}"),
        (None, Some(long)) => format!("--{long}"),
        (None, None) => String::new(),
    };
    if let Some(value) = option.value_name {
        display.push_str(&format!(" <{value}>"));
    }
    display
}

fn item_help(help: &str, values: &[&str]) -> String {
    let mut text = help.to_string();
    if !values.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("[possible values: {}]", values.join(", ")));
    }
    text
}

fn help_text<A>(cli: &Cli<A>, prog: &str, width: usize) -> String {
    let mut out = String::new();
    if let Some(about) = cli.about {
        for line in wrap(about.trim_end(), width) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }
    writeln!(out, "Usage: {}", usage(cli, prog)).expect("writing to a String succeeds");
    if !cli.args.is_empty() {
        let rows: Vec<(String, String)> = cli
            .args
            .iter()
            .map(|pos| (pos.display(), item_help(pos.help, &[])))
            .collect();
        write_table(&mut out, "Arguments", &rows, width);
    }
    let options: Vec<OptionInfo> = cli
        .options()
        .into_iter()
        .filter(|option| !option.hidden)
        .collect();
    let pad_long = options.iter().any(|option| option.short.is_some());
    let mut sections: Vec<(Option<&str>, Rows)> = Vec::new();
    for option in &options {
        let row = (
            option_display(option, pad_long),
            item_help(option.help, option.values),
        );
        match sections
            .iter_mut()
            .find(|section| section.0 == option.section)
        {
            Some(section) => section.1.push(row),
            None => sections.push((option.section, vec![row])),
        }
    }
    for (title, rows) in sections {
        write_table(&mut out, title.unwrap_or("Options"), &rows, width);
    }
    if let Some(after_help) = cli.after_help {
        out.push('\n');
        for line in wrap(after_help.trim_end(), width) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn text_width(s: &str) -> usize {
    s.chars().count()
}

fn use_next_line(rows: &[(String, String)], width: usize) -> bool {
    let longest = rows.iter().map(|row| text_width(&row.0)).max().unwrap_or(0);
    let taken = longest + TABLE_INDENT_COLUMNS + TABLE_GAP_COLUMNS;
    rows.iter().any(|(_, help)| {
        width >= taken
            && (taken as f32 / width as f32) > NEXT_LINE_LEFT_SHARE
            && text_width(help) > width - taken
    })
}

fn write_table(out: &mut String, title: &str, rows: &[(String, String)], width: usize) {
    if rows.is_empty() {
        return;
    }
    writeln!(out, "\n{title}:").expect("writing to a String succeeds");
    if !use_next_line(rows, width) {
        let longest = rows.iter().map(|row| text_width(&row.0)).max().unwrap_or(0);
        let column = TABLE_INDENT_COLUMNS + longest + TABLE_GAP_COLUMNS;
        for (left, help) in rows {
            let lines = wrap(
                help,
                width.saturating_sub(column).max(MIN_HELP_WIDTH_COLUMNS),
            );
            let mut line = format!("  {left}");
            if let Some(first) = lines.first() {
                line.push_str(&" ".repeat(column - TABLE_INDENT_COLUMNS - text_width(left)));
                line.push_str(first);
            }
            out.push_str(line.trim_end());
            out.push('\n');
            for line in lines.iter().skip(1) {
                if !line.is_empty() {
                    out.push_str(&" ".repeat(column));
                    out.push_str(line);
                }
                out.push('\n');
            }
        }
    } else {
        for (index, (left, help)) in rows.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            writeln!(out, "  {left}").expect("writing to a String succeeds");
            let help_width = width
                .saturating_sub(NEXT_LINE_INDENT_COLUMNS)
                .max(MIN_HELP_WIDTH_COLUMNS);
            for line in wrap(help, help_width) {
                if !line.is_empty() {
                    out.push_str(&" ".repeat(NEXT_LINE_INDENT_COLUMNS));
                    out.push_str(&line);
                }
                out.push('\n');
            }
        }
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if text.is_empty() {
        return lines;
    }
    for line in text.split('\n') {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            lines.push(String::new());
            continue;
        }
        let indent = &line[..line.len() - trimmed.len()];
        let mut current = indent.to_string();
        let mut current_width = text_width(indent);
        let mut has_word = false;
        let mut rest = trimmed;
        while !rest.is_empty() {
            let space_length = rest.len() - rest.trim_start().len();
            let space = &rest[..space_length];
            rest = &rest[space_length..];
            let word_length = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..word_length];
            rest = &rest[word_length..];
            let word_width = text_width(word);
            if has_word && current_width + text_width(space) + word_width > width {
                lines.push(std::mem::replace(&mut current, indent.to_string()));
                current_width = text_width(indent);
                has_word = false;
            }
            if has_word {
                current.push_str(space);
                current_width += text_width(space);
            }
            current.push_str(word);
            current_width += word_width;
            has_word = true;
        }
        lines.push(current);
    }
    lines
}

fn suggest<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let threshold = (input.chars().count() / SUGGESTION_DISTANCE_DIVISOR).max(1);
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let distance = edit_distance(input, candidate);
            let close = distance <= threshold
                || (input.len() >= SUGGESTION_PREFIX_CHARS_MIN && candidate.starts_with(input));
            close.then_some((distance, candidate))
        })
        .min_by_key(|pair| pair.0)
        .map(|pair| pair.1)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut distances = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in distances.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in distances[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            distances[i][j] = (distances[i - 1][j] + 1)
                .min(distances[i][j - 1] + 1)
                .min(distances[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                distances[i][j] = distances[i][j].min(distances[i - 2][j - 2] + 1);
            }
        }
    }
    distances[a.len()][b.len()]
}
