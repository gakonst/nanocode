pub const COMMANDS: [(&str, &str); 34] = [
    (
        "/model",
        "choose the model, thinking and speed for a new thread; tempo/<model> runs on the Tempo gateway",
    ),
    (
        "/harness",
        "run the thread on the Codex or Claude harness: /harness [codex|claude]",
    ),
    ("/thinking", "open the thinking dial"),
    ("/effort", "open the thinking dial"),
    (
        "/fast",
        "cycle the service tier (standard, fast, ultrafast)",
    ),
    (
        "/btw",
        "open the side pane: a forked side thread that never interrupts the turn; /btw agent|stop #id for its subagents",
    ),
    ("/resume", "resume a saved session"),
    (
        "/login",
        "sign in with ChatGPT, Claude or a Tempo wallet: /login codex|claude|tempo",
    ),
    (
        "/fund",
        "buy MACH by card for the Tempo wallet that pays for tools and tempo/ models: /fund [usd]",
    ),
    (
        "/wallet",
        "open the Tempo wallet web app for the wallet nanocode pays from",
    ),
    ("/new", "start a new session"),
    (
        "/agents",
        "toggle the tree of active subagents; /agents all lists finished ones too",
    ),
    ("/compact", "compact the conversation context"),
    ("/context", "token usage for the last turn and the session"),
    ("/copy", "copy the last reply to the clipboard"),
    ("/theme", "choose the auto, dark or light theme"),
    ("/skills", "insert a skill from this workspace"),
    ("/image", "attach an image from the clipboard"),
    ("/detach", "remove pending image attachments"),
    ("/session", "show session, model and tool details"),
    ("/id", "show the session id and copy it to the clipboard"),
    (
        "/runner",
        "list, connect, listen, select or disconnect hands",
    ),
    ("/expand", "expand or collapse tools and reasoning"),
    ("/mouse", "toggle mouse capture for native text selection"),
    ("/help", "show key bindings"),
    ("/clear", "start a new session"),
    (
        "/collapse",
        "send the side pane's conversation to its thread and close it",
    ),
    ("/split", "open the side thread in a new terminal pane"),
    ("/close", "close the side pane without touching the thread"),
    (
        "/zoom",
        "toggle the focused pane between split and full width",
    ),
    (
        "/agent",
        "focus a subagent by #id or role and message it; /agent alone returns to the main thread",
    ),
    (
        "/stop",
        "close a subagent and its children for good by #id or role; alone closes the focused one; esc esc only interrupts",
    ),
    (
        "/reload",
        "restart all nc windows with the newest binary, continuing active turns and queued prompts",
    ),
    ("/quit", "exit nanocode"),
];

use nanocode_arguments::{Alias, Argument, Command, Group, Matches, Parser};

static RUNNER: Command = Command::new("/runner")
    .about("Manage runner connections and listeners")
    .without_help_subcommand()
    .subcommands(&[
        Command::new("list").aliases(&[Alias::hidden("status")]),
        Command::new("connect").groups(&[Group::named(
            "Connect",
            &[
                Argument::positional("url", "URL", Parser::Text).required(),
                Argument::value("token", "TOKEN", Parser::Text).long("token"),
                Argument::value("name", "NAME", Parser::Text).long("name"),
                Argument::flag("replace").long("replace"),
            ],
        )]),
        Command::new("listen").groups(&[Group::named(
            "Listen",
            &[
                Argument::positional("address", "ADDRESS", Parser::SocketAddress).required(),
                Argument::value("token", "TOKEN", Parser::Text).long("token"),
            ],
        )]),
        Command::new("select")
            .about(
                "Run this session's exec cells on hand NAME, or on this machine without a name; scripts reach other hands with a hand argument on each tool call",
            )
            .groups(&[Group::named(
                "Select",
                &[Argument::positional("name", "NAME", Parser::Text)],
            )]),
        Command::new("disconnect").groups(&[Group::named(
            "Disconnect",
            &[Argument::positional("name", "NAME", Parser::Text).required()],
        )]),
        Command::new("stop-listening"),
    ]);

pub struct RunnerCommand {
    pub action: Option<RunnerAction>,
}

pub enum RunnerAction {
    List,
    Connect {
        url: String,
        token: Option<String>,
        name: Option<String>,
        replace: bool,
    },
    Listen {
        address: std::net::SocketAddr,
        token: Option<String>,
    },
    Select {
        name: Option<String>,
    },
    Disconnect {
        name: String,
    },
    StopListening,
}

impl RunnerCommand {
    pub fn try_parse_from<I, T>(arguments: I) -> Result<Self, nanocode_arguments::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString>,
    {
        let matches = RUNNER.parse_from(arguments)?;
        Ok(Self {
            action: matches
                .subcommand()
                .map(|(name, matches)| RunnerAction::from_matches(name, matches)),
        })
    }
}

impl RunnerAction {
    fn from_matches(name: &str, matches: &Matches) -> Self {
        match name {
            "list" => Self::List,
            "connect" => Self::Connect {
                url: matches.text("url").expect("url is required"),
                token: matches.text("token"),
                name: matches.text("name"),
                replace: matches.flag("replace"),
            },
            "listen" => Self::Listen {
                address: matches.address("address").expect("address is required"),
                token: matches.text("token"),
            },
            "select" => Self::Select {
                name: matches.text("name"),
            },
            "disconnect" => Self::Disconnect {
                name: matches.text("name").expect("name is required"),
            },
            "stop-listening" => Self::StopListening,
            other => unreachable!("{other} is not a /runner subcommand"),
        }
    }
}

pub fn matching<'a>(
    prefix: &'a str,
    extra: Option<(&'static str, &'static str)>,
) -> impl Iterator<Item = (&'static str, &'static str)> + 'a {
    COMMANDS
        .iter()
        .copied()
        .chain(extra)
        .filter(move |(command, _)| command.starts_with(prefix))
}

pub fn complete(prefix: &str, extra: Option<(&'static str, &'static str)>) -> Option<&'static str> {
    matching(prefix, extra).next().map(|(command, _)| command)
}
