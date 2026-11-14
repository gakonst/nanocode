use std::{net::SocketAddr, path::PathBuf};

use nanocode_arguments::{self as arguments, Alias, Argument, Choice, Group, Matches, Parser};
use nanocode_observability::ObservabilityOutputArgs;

#[derive(Debug)]
pub struct Cli {
    pub command: Option<Command>,
    pub session: SessionArgs,
    pub observability: ObservabilityOutputArgs,
}

#[derive(Debug)]
pub enum Command {
    Hand(HandArgs),
    Sessions(SessionsArgs),
    Exec(SessionArgs),
    Models,
    Resume {
        id: Option<String>,
    },
    #[cfg(unix)]
    Cua(CuaArgs),
}

#[derive(Clone, Debug)]
pub struct CuaArgs {
    pub command: CuaCommand,
}

#[derive(Clone, Debug)]
pub enum CuaCommand {
    Install {
        browser: Vec<String>,
        skip_driver: bool,
    },
    Uninstall {
        browser: Vec<String>,
    },
    Status,
    #[cfg(target_os = "macos")]
    DriverMcp,
    BrowserHost {
        #[allow(dead_code)]
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessArg {
    Codex,
    Claude,
}

impl HarnessArg {
    const ALL: [Self; 2] = [Self::Codex, Self::Claude];
    const CHOICES: [Choice; 2] = [
        Choice::new("codex").help("OpenAI Codex models (ChatGPT login)"),
        Choice::new("claude").help("Anthropic Claude models (Claude Code login)"),
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CyberAccessProgramArg {
    Standard,
    DaybreakBlue,
    DaybreakRed,
}

impl CyberAccessProgramArg {
    const ALL: [Self; 3] = [Self::Standard, Self::DaybreakBlue, Self::DaybreakRed];
    const CHOICES: [Choice; 3] = [
        Choice::new("standard"),
        Choice::new("daybreak_blue"),
        Choice::new("daybreak_red"),
    ];
}

impl From<CyberAccessProgramArg> for nanocode::codex::CyberAccessProgram {
    fn from(program: CyberAccessProgramArg) -> Self {
        match program {
            CyberAccessProgramArg::Standard => Self::Standard,
            CyberAccessProgramArg::DaybreakBlue => Self::DaybreakBlue,
            CyberAccessProgramArg::DaybreakRed => Self::DaybreakRed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayArg {
    Openrouter,
    Vercel,
    Cloudflare,
    CloudflareGateway,
}

impl GatewayArg {
    const ALL: [Self; 4] = [
        Self::Openrouter,
        Self::Vercel,
        Self::Cloudflare,
        Self::CloudflareGateway,
    ];
    const CHOICES: [Choice; 4] = [
        Choice::new("openrouter").help("OpenRouter Responses API; key from OPENROUTER_API_KEY"),
        Choice::new("vercel").help("Vercel AI Gateway; key from AI_GATEWAY_API_KEY or VERCEL_OIDC_TOKEN"),
        Choice::new("cloudflare")
            .help("Cloudflare unified API; token from CLOUDFLARE_API_TOKEN, needs --cloudflare-account"),
        Choice::new("cloudflare-gateway").help(
            "Cloudflare AI Gateway OpenAI proxy; OPENAI_API_KEY, CF_AIG_TOKEN for stored keys, or CLOUDFLARE_API_TOKEN for Unified Billing; needs --cloudflare-account and --cloudflare-gateway",
        ),
    ];
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunnerModeArg {
    #[default]
    Alongside,
    Replace,
}

impl RunnerModeArg {
    const ALL: [Self; 2] = [Self::Alongside, Self::Replace];
    const CHOICES: [Choice; 2] = [
        Choice::new("alongside")
            .help("Keep local tools; the runner's workspace is reachable under /<runner-name>"),
        Choice::new("replace").help("Run every tool on the runner instead of this machine"),
    ];
}

const TIERS: [Choice; 3] = [
    Choice::new("standard"),
    Choice::new("fast"),
    Choice::new("ultrafast"),
];

const SESSION: Group = Group::new(&[
    Argument::positional("prompt", "PROMPT", Parser::Text)
        .allow_hyphen_values()
        .help("Prompt to send as soon as the session is ready"),
    Argument::value("model", "MODEL", Parser::Text)
        .short('m')
        .long("model")
        .env("NANOCODE_MODEL")
        .help(
            "Model id or alias (see 'nanocode models'); tempo/<model> runs it on the Tempo gateway, paid from the Tempo wallet; defaults to claude-opus-5-5 at xhigh thinking",
        ),
    Argument::value("harness", "HARNESS", Parser::Choice(&HarnessArg::CHOICES))
        .long("harness")
        .help(
            "Harness to run on: its default model when --model is not given, and the harness for a tempo/ model that is not a Codex or Claude model (Codex unless set)",
        ),
    Argument::value("thinking", "THINKING", Parser::Text)
        .short('t')
        .long("thinking")
        .help("Thinking level supported by the model (none, low, medium, high, xhigh, max)"),
    Argument::value("tier", "TIER", Parser::OneOf(&TIERS))
        .long("tier")
        .help("Service tier; only tiers the model supports are accepted"),
    Argument::value("context_window", "TOKENS", Parser::U64)
        .long("context-window")
        .env("NANOCODE_CONTEXT_WINDOW")
        .help(
            "The model's context window in tokens, for a model nanocode does not know, such as one on a custom endpoint; a known model's window cannot be raised past its maximum",
        ),
    Argument::value("max_output_tokens", "TOKENS", Parser::U64)
        .long("max-output-tokens")
        .env("NANOCODE_MAX_OUTPUT_TOKENS")
        .help(
            "Longest reply in tokens the Claude harness asks for (max_tokens); Codex requests carry no reply limit, as Codex's do not",
        ),
    Argument::value("cyber_access_program", "PROGRAM", Parser::Choice(&CyberAccessProgramArg::CHOICES))
        .long("cyber-access-program")
        .help(
            "Request an experimental Cyber access program on Codex models (OpenAI only, not with --gateway), as Codex's --cyber-access-program; omit to use server defaults",
        ),
    Argument::value("workspace", "WORKSPACE", Parser::Path)
        .short('C')
        .long("cd")
        .help("Workspace directory (defaults to the current directory)"),
    Argument::optional_value("resume", "SESSION_ID", Parser::Text)
        .short('r')
        .long("resume")
        .help("Resume a saved session; without an id, pick one interactively"),
    Argument::value("reload_state", "RELOAD_STATE", Parser::Path)
        .long("reload-state")
        .hidden(),
    Argument::flag("continue_last")
        .short('c')
        .long("continue")
        .help("Resume the most recent session in this workspace"),
    Argument::flag("ephemeral")
        .long("ephemeral")
        .help("Keep the session in memory only; nothing is saved"),
    Argument::value("home", "HOME", Parser::Path)
        .long("home")
        .env("NANOCODE_HOME")
        .help("Directory for saved sessions, history and pasted images (defaults to ~/.nanocode)"),
    Argument::flag("no_mouse")
        .long("no-mouse")
        .help("Leave mouse capture off so the terminal keeps native text selection"),
    Argument::flag("no_subagents")
        .long("no-subagents")
        .help("Disable subagent tools"),
    Argument::flag("subagents")
        .long("subagents")
        .conflicts(&["no_subagents"])
        .help("Offer subagent tools to models that get the harness tools directly too (off for them by default)"),
]);

const GATEWAY: Group = Group::named(
    "GatewayArgs",
    &[
        Argument::value("gateway", "GATEWAY", Parser::Choice(&GatewayArg::CHOICES))
            .long("gateway")
            .env("NANOCODE_GATEWAY")
            .help(
                "Route Codex models (gpt-6.1-sol, terra, luna, astra) through a Responses gateway for this process, including /new, /model and resumed sessions; keys are read from the provider's environment variable and never saved; Claude models keep their own login, and models named tempo/<model> run on the Tempo gateway; restart to change",
            ),
        Argument::value("cloudflare_account", "ID", Parser::Text)
            .long("cloudflare-account")
            .env("CLOUDFLARE_ACCOUNT_ID")
            .help("Cloudflare account id for --gateway cloudflare or cloudflare-gateway"),
        Argument::value("cloudflare_gateway", "ID", Parser::Text)
            .long("cloudflare-gateway")
            .env("CLOUDFLARE_GATEWAY_ID")
            .help(
                "Cloudflare AI Gateway id; required for cloudflare-gateway, optional for cloudflare (account default gateway when unset)",
            ),
        Argument::value("gateway_base_url", "URL", Parser::Text)
            .long("gateway-base-url")
            .requires("gateway")
            .help("Override the gateway base URL, for example a Cloudflare custom domain"),
        Argument::value("tempo_currency", "TOKEN", Parser::Text)
            .long("tempo-currency")
            .env("NANOCODE_TEMPO_CURRENCY")
            .help(
                "Token to prefer paying the Tempo gateway in: mach (the default), ousd, usdc.e or a token address; when the gateway does not take it, it is paid in one it does, in a session of its own",
            ),
        Argument::value("tempo_fee_token", "TOKEN", Parser::Text)
            .long("tempo-fee-token")
            .env("NANOCODE_TEMPO_FEE_TOKEN")
            .help(
                "Token to pay the gas of Tempo gateway session opens and top-ups in, instead of the fee token its payment challenge names: usdc.e, usdt0 or a token address; MACH and OUSD cannot pay gas",
            ),
        Argument::value("tempo_session", "NAME", Parser::Text)
            .long("tempo-session")
            .env("NANOCODE_TEMPO_SESSION")
            .help(
                "Pay each Tempo service (the Tempo gateway's models and tools, Mercator and the services shell commands reach) from this named session instead of the lowest of 64 slots no other process holds",
            ),
        Argument::flag("tempo_close_session")
            .long("tempo-close-session")
            .env("NANOCODE_TEMPO_CLOSE_SESSION")
            .help(
                "Close the Tempo gateway session cooperatively on exit, which returns the unspent deposit at once, instead of keeping it open for the next run to reuse",
            ),
        Argument::flag("tempo_keep_session")
            .long("tempo-keep-session")
            .hidden()
            .env("NANOCODE_TEMPO_KEEP_SESSION")
            .conflicts(&["tempo_close_session"]),
        Argument::value("tempo_deposit", "BASE_UNITS", Parser::U128)
            .long("tempo-deposit")
            .env("NANOCODE_TEMPO_DEPOSIT")
            .help(
                "Session deposit and top-up for the Tempo gateway's models and tools, in the token's base units (6 decimals), when it suggests no deposit of its own: 2000000 (2 MACH) unless set",
            ),
        Argument::value("tempo_tool_deposit", "BASE_UNITS", Parser::U128)
            .long("tempo-tool-deposit")
            .env("NANOCODE_TEMPO_TOOL_DEPOSIT")
            .help(
                "Session deposit and top-up, in base units, for the Tempo gateway's tools and the paid services shell commands reach, when a service suggests no deposit of its own (the Tempo gateway suggests 1 MACH): 100000 (0.1 MACH) unless set",
            ),
        Argument::flag("no_tempo_tools")
            .long("no-tempo-tools")
            .env("NANOCODE_NO_TEMPO_TOOLS")
            .help(
                "Leave out the paid tools of the Tempo gateway (key-value store, buckets, compute) and Mercator, and the local proxy through which shell commands pay Tempo's services (*.mpp.tempo.xyz), the Tempo gateway and Mercator; every session has them by default and pays from the Tempo wallet (/login tempo, /fund) when one charges. A session whose environment sets its own HTTP(S)_PROXY, ALL_PROXY or CA bundle keeps those and gets no proxy",
            ),
        Argument::repeated("identity_issuer", "URL", Parser::Text)
            .long("identity-issuer")
            .env("NANOCODE_IDENTITY_ISSUER")
            .help(
                "Identity issuer an MPP service may name when it asks for an email pass, repeatable or comma-separated: the address and code go only to these; https://identity.mpp.tempo.xyz unless set",
            ),
        Argument::value("identity_email", "ADDRESS", Parser::Text)
            .long("identity-email")
            .env("NANOCODE_IDENTITY_EMAIL")
            .help(
                "Email address to get an email pass for when an MPP service asks for one; without it the session asks (/pass <email>)",
            ),
        Argument::repeated("identity_attestation", "JWS", Parser::Text)
            .long("identity-attestation")
            .env("NANOCODE_IDENTITY_ATTESTATION")
            .help(
                "Email pass already obtained from an issuer, sent in Identity-Attestation to the origin it names until it expires, repeatable or comma-separated",
            ),
    ],
);

const REMOTE: Group = Group::named(
    "RemoteArgs",
    &[
        Argument::value("runner_connect", "WS_URL", Parser::Text)
            .long("runner-connect")
            .conflicts(&["runner_host"])
            .help("Connect to a runner started with 'nanocode runner --listen'"),
        Argument::value("runner_host", "ADDR", Parser::SocketAddress)
            .long("runner-host")
            .help("Listen for a runner started with 'nanocode runner --connect'"),
        Argument::value("runner_token", "RUNNER_TOKEN", Parser::Text)
            .long("runner-token")
            .env("NANOCODE_RUNNER_TOKEN")
            .hide_env_values()
            .help("Shared secret authenticating the runner connection"),
        Argument::value("runner_name", "RUNNER_NAME", Parser::Text)
            .long("runner-name")
            .help(
                "Runner name; also the mount point /<name> in alongside mode. Defaults to runner, or to the name a Code Mode executor advertises",
            ),
        Argument::value("runner_workspace", "PATH", Parser::Path)
            .long("runner-workspace")
            .help("Override the physical workspace advertised by the runner"),
        Argument::value("runner_mode", "RUNNER_MODE", Parser::Choice(&RunnerModeArg::CHOICES))
            .long("runner-mode")
            .default("alongside")
            .help("Use the runner alongside local tools or instead of them"),
        Argument::value("runner_call_timeout_ms", "MS", Parser::U64)
            .long("runner-call-timeout-ms")
            .help("Hard deadline for each remote tool call, in milliseconds"),
        Argument::flag("tab")
            .long("tab")
            .conflicts(&["runner_connect", "runner_host"])
            .help(
                "Make a browser tab this session's hand: prints a nanocode.xyz/hand link that pairs the tab with this agent through the nanocode.xyz hand relay, and waits for it",
            ),
    ],
);

const HAND: Group = Group::named(
    "HandArgs",
    &[
        Argument::positional("url", "URL", Parser::Text)
            .required_unless("listen")
            .conflicts(&["listen"])
            .help(
                "Brain endpoint to dial, such as wss://<worker>/sessions/<name>/hands; the hand reconnects until the brain disconnects it",
            ),
        Argument::value("listen", "ADDR", Parser::SocketAddress)
            .long("listen")
            .help("Listen on this loopback address for a brain to dial instead"),
        Argument::value("token", "TOKEN", Parser::Text)
            .long("token")
            .env("NANOCODE_RUNNER_TOKEN")
            .hide_env_values()
            .required()
            .help("Shared secret presented to or required from the brain"),
        Argument::value("name", "NAME", Parser::Text)
            .long("name")
            .help("Hand name (defaults to this machine's host name)"),
        Argument::value("directory", "DIRECTORY", Parser::Path)
            .short('C')
            .long("cd")
            .help("Starting directory (defaults to the current directory)"),
        Argument::flag("cua")
            .long("cua")
            .help("Also serve computer use from the installed provider"),
        Argument::value("cell_yield_ms", "MS", Parser::U64)
            .long("cell-yield-ms")
            .help("How long a cell runs before it yields when the brain's call names no yield_time_ms (defaults to 10000). A brain whose host set a cell yield, or that has an operation deadline, names one in every call, so this reaches only a brain with neither"),
        Argument::flag("own_processes")
            .long("own-processes")
            .help("Run every command in a process of its own, never in the hand's: one that exhausts memory then ends alone"),
        Argument::value("pricing", "FILE", Parser::Path)
            .long("pricing")
            .help(
                "Charge for operations by the rules in this JSON file, matched first to last: {\"payee\": \"0x…\", \"describe\": \"…\", \"currency\": \"OUSD\" (default) or \"USDC.e\", \"rules\": [{\"paths\": [\"data/**\"], \"ops\": [\"read\"], \"usd\": 0.01}]}",
            ),
        Argument::flag("tools_only")
            .long("tools-only")
            .help("Run no scripts here: agents call this hand's tools one at a time, so --pricing sees every file and command"),
    ],
);

const SESSIONS: Group = Group::named(
    "SessionsArgs",
    &[
        Argument::value("home", "HOME", Parser::Path)
            .long("home")
            .env("NANOCODE_HOME")
            .help("Session store directory (defaults to ~/.nanocode)"),
        Argument::flag("all")
            .long("all")
            .help("Include sessions from every workspace"),
        Argument::value("limit", "N", Parser::U64)
            .long("limit")
            .help("List at most N sessions, newest first (100 unless set)"),
        Argument::positional("session", "SESSION", Parser::Text)
            .help("Print this session's user and assistant messages instead of listing sessions"),
    ],
);

#[cfg(unix)]
const CUA: arguments::Command = arguments::Command::new("cua")
    .about("Set up computer use: cua-driver for native apps and the nanocode browser extension for background tab groups")
    .subcommand_required()
    .subcommands(&[
        arguments::Command::new("install")
            .about(
                "Ask macOS for the permissions of nanocode's built-in cua-driver (elsewhere install cua-driver) and install the nanocode extension's native host in each Chromium browser",
            )
            .groups(&[Group::named(
                "Install",
                &[
                    Argument::repeated("browser", "BROWSER", Parser::Text)
                        .long("browser")
                        .help(
                            "Browser name (Chrome, Edge, Brave, ...) or an absolute Chromium user data directory; repeat for several (defaults to every one found)",
                        ),
                    Argument::flag("skip_driver")
                        .long("skip-driver")
                        .help("Leave cua-driver and its permissions as they are"),
                ],
            )]),
        arguments::Command::new("uninstall")
            .about("Remove the nanocode extension's native host")
            .groups(&[Group::named(
                "Uninstall",
                &[Argument::repeated("browser", "BROWSER", Parser::Text)
                    .long("browser")
                    .help("Browser name or user data directory (defaults to every one found)")],
            )]),
        arguments::Command::new("status").about("Show what computer use finds on this machine"),
        #[cfg(target_os = "macos")]
        arguments::Command::new("driver-mcp").hidden(),
        arguments::Command::new("browser-host")
            .hidden()
            .groups(&[Group::named("BrowserHost", &[Argument::trailing("args", "ARGS")])]),
    ]);

pub static COMMAND: arguments::Command = arguments::Command::new("nanocode")
    .version(env!("CARGO_PKG_VERSION"))
    .about("Interactive coding agent for Codex and Claude")
    .long_about(
        "Interactive coding agent for Codex and Claude. Run without a subcommand to open the terminal interface; 'nc' is an alias of 'nanocode'.",
    )
    .groups(&[SESSION, GATEWAY, REMOTE, ObservabilityOutputArgs::ARGUMENTS])
    .subcommands(&[
        arguments::Command::new("hand")
            .aliases(&[Alias::visible("runner")])
            .about("Make this machine a hand that runs a nanocode agent's exec cells: dial the brain's URL, or listen for the brain")
            .groups(&[HAND]),
        arguments::Command::new("sessions")
            .about("List saved sessions (current workspace unless --all)")
            .groups(&[SESSIONS]),
        arguments::Command::new("exec")
            .about(
                "Run one prompt without the terminal interface, streaming the reply to stdout and reasoning summaries and tool calls to stderr",
            )
            .groups(&[SESSION, GATEWAY, REMOTE]),
        arguments::Command::new("models").about("List models per harness with thinking levels and speed tiers"),
        arguments::Command::new("resume")
            .about("Resume a saved session; without an id, pick one interactively")
            .groups(&[Group::named(
                "Resume",
                &[Argument::positional("id", "ID", Parser::Text).help("Session id to resume")],
            )]),
        #[cfg(unix)]
        CUA,
    ]);

impl Cli {
    pub fn parse() -> Self {
        Self::from_matches(&COMMAND.parse())
    }

    pub fn from_matches(matches: &Matches) -> Self {
        Self {
            command: matches
                .subcommand()
                .map(|(name, matches)| Command::from_matches(name, matches)),
            session: SessionArgs::from_matches(matches),
            observability: ObservabilityOutputArgs::from_matches(matches),
        }
    }
}

impl Command {
    fn from_matches(name: &str, matches: &Matches) -> Self {
        match name {
            "hand" => Self::Hand(HandArgs::from_matches(matches)),
            "sessions" => Self::Sessions(SessionsArgs::from_matches(matches)),
            "exec" => Self::Exec(SessionArgs::from_matches(matches)),
            "models" => Self::Models,
            "resume" => Self::Resume {
                id: matches.text("id"),
            },
            #[cfg(unix)]
            "cua" => Self::Cua(CuaArgs::from_matches(matches)),
            other => unreachable!("{other} is not a nanocode subcommand"),
        }
    }
}

#[cfg(unix)]
impl CuaArgs {
    fn from_matches(matches: &Matches) -> Self {
        let (name, matches) = matches.subcommand().expect("cua requires a subcommand");
        let command = match name {
            "install" => CuaCommand::Install {
                browser: matches.texts("browser"),
                skip_driver: matches.flag("skip_driver"),
            },
            "uninstall" => CuaCommand::Uninstall {
                browser: matches.texts("browser"),
            },
            "status" => CuaCommand::Status,
            #[cfg(target_os = "macos")]
            "driver-mcp" => CuaCommand::DriverMcp,
            "browser-host" => CuaCommand::BrowserHost {
                args: matches.texts("args"),
            },
            other => unreachable!("{other} is not a cua subcommand"),
        };
        Self { command }
    }
}

#[derive(Clone, Debug)]
pub struct SessionArgs {
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub harness: Option<HarnessArg>,
    pub thinking: Option<String>,
    pub tier: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub cyber_access_program: Option<CyberAccessProgramArg>,
    pub workspace: Option<PathBuf>,
    pub resume: Option<String>,
    pub reload_state: Option<PathBuf>,
    pub continue_last: bool,
    pub ephemeral: bool,
    pub home: Option<PathBuf>,
    pub no_mouse: bool,
    pub no_subagents: bool,
    pub subagents: bool,
    pub gateway: GatewayArgs,
    pub remote: RemoteArgs,
}

#[derive(Clone, Debug, Default)]
pub struct GatewayArgs {
    pub gateway: Option<GatewayArg>,
    pub cloudflare_account: Option<String>,
    pub cloudflare_gateway: Option<String>,
    pub gateway_base_url: Option<String>,
    pub tempo_currency: Option<String>,
    pub tempo_fee_token: Option<String>,
    pub tempo_session: Option<String>,
    pub tempo_close_session: bool,
    pub tempo_keep_session: bool,
    pub tempo_deposit: Option<u128>,
    pub tempo_tool_deposit: Option<u128>,
    pub no_tempo_tools: bool,
    pub identity_issuer: Vec<String>,
    pub identity_email: Option<String>,
    pub identity_attestation: Vec<String>,
}

impl SessionArgs {
    fn from_matches(matches: &Matches) -> Self {
        Self {
            prompt: matches.text("prompt"),
            model: matches.text("model"),
            harness: matches
                .choice("harness")
                .map(|index| HarnessArg::ALL[index]),
            thinking: matches.text("thinking"),
            tier: matches.text("tier"),
            context_window: matches.u64("context_window"),
            max_output_tokens: matches.u64("max_output_tokens"),
            cyber_access_program: matches
                .choice("cyber_access_program")
                .map(|index| CyberAccessProgramArg::ALL[index]),
            workspace: matches.path("workspace"),
            resume: matches.text("resume"),
            reload_state: matches.path("reload_state"),
            continue_last: matches.flag("continue_last"),
            ephemeral: matches.flag("ephemeral"),
            home: matches.path("home"),
            no_mouse: matches.flag("no_mouse"),
            no_subagents: matches.flag("no_subagents"),
            subagents: matches.flag("subagents"),
            gateway: GatewayArgs::from_matches(matches),
            remote: RemoteArgs::from_matches(matches),
        }
    }

    pub fn or(self, root: Self) -> Self {
        Self {
            prompt: self.prompt.or(root.prompt),
            model: self.model.or(root.model),
            harness: self.harness.or(root.harness),
            thinking: self.thinking.or(root.thinking),
            tier: self.tier.or(root.tier),
            context_window: self.context_window.or(root.context_window),
            max_output_tokens: self.max_output_tokens.or(root.max_output_tokens),
            cyber_access_program: self.cyber_access_program.or(root.cyber_access_program),
            workspace: self.workspace.or(root.workspace),
            resume: self.resume.or(root.resume),
            reload_state: self.reload_state.or(root.reload_state),
            continue_last: self.continue_last || root.continue_last,
            ephemeral: self.ephemeral || root.ephemeral,
            home: self.home.or(root.home),
            no_mouse: self.no_mouse || root.no_mouse,
            no_subagents: self.no_subagents || root.no_subagents,
            subagents: self.subagents || root.subagents,
            gateway: GatewayArgs {
                gateway: self.gateway.gateway.or(root.gateway.gateway),
                cloudflare_account: self
                    .gateway
                    .cloudflare_account
                    .or(root.gateway.cloudflare_account),
                cloudflare_gateway: self
                    .gateway
                    .cloudflare_gateway
                    .or(root.gateway.cloudflare_gateway),
                gateway_base_url: self
                    .gateway
                    .gateway_base_url
                    .or(root.gateway.gateway_base_url),
                tempo_currency: self.gateway.tempo_currency.or(root.gateway.tempo_currency),
                tempo_fee_token: self
                    .gateway
                    .tempo_fee_token
                    .or(root.gateway.tempo_fee_token),
                tempo_session: self.gateway.tempo_session.or(root.gateway.tempo_session),
                tempo_close_session: self.gateway.tempo_close_session
                    || root.gateway.tempo_close_session,
                tempo_keep_session: self.gateway.tempo_keep_session
                    || root.gateway.tempo_keep_session,
                tempo_deposit: self.gateway.tempo_deposit.or(root.gateway.tempo_deposit),
                tempo_tool_deposit: self
                    .gateway
                    .tempo_tool_deposit
                    .or(root.gateway.tempo_tool_deposit),
                no_tempo_tools: self.gateway.no_tempo_tools || root.gateway.no_tempo_tools,
                identity_issuer: match self.gateway.identity_issuer.is_empty() {
                    true => root.gateway.identity_issuer,
                    false => self.gateway.identity_issuer,
                },
                identity_email: self.gateway.identity_email.or(root.gateway.identity_email),
                identity_attestation: match self.gateway.identity_attestation.is_empty() {
                    true => root.gateway.identity_attestation,
                    false => self.gateway.identity_attestation,
                },
            },
            remote: RemoteArgs {
                runner_connect: self.remote.runner_connect.or(root.remote.runner_connect),
                runner_host: self.remote.runner_host.or(root.remote.runner_host),
                runner_token: self.remote.runner_token.or(root.remote.runner_token),
                runner_name: self.remote.runner_name.or(root.remote.runner_name),
                runner_workspace: self
                    .remote
                    .runner_workspace
                    .or(root.remote.runner_workspace),
                runner_call_timeout_ms: self
                    .remote
                    .runner_call_timeout_ms
                    .or(root.remote.runner_call_timeout_ms),
                tab: self.remote.tab || root.remote.tab,
                ..self.remote
            },
        }
    }
}

impl GatewayArgs {
    fn from_matches(matches: &Matches) -> Self {
        Self {
            gateway: matches
                .choice("gateway")
                .map(|index| GatewayArg::ALL[index]),
            cloudflare_account: matches.text("cloudflare_account"),
            cloudflare_gateway: matches.text("cloudflare_gateway"),
            gateway_base_url: matches.text("gateway_base_url"),
            tempo_currency: matches.text("tempo_currency"),
            tempo_fee_token: matches.text("tempo_fee_token"),
            tempo_session: matches.text("tempo_session"),
            tempo_close_session: matches.flag("tempo_close_session"),
            tempo_keep_session: matches.flag("tempo_keep_session"),
            tempo_deposit: matches.u128("tempo_deposit"),
            tempo_tool_deposit: matches.u128("tempo_tool_deposit"),
            no_tempo_tools: matches.flag("no_tempo_tools"),
            identity_issuer: listed(matches, "identity_issuer"),
            identity_email: matches.text("identity_email"),
            identity_attestation: listed(matches, "identity_attestation"),
        }
    }
}

fn listed(matches: &Matches, id: &str) -> Vec<String> {
    matches
        .texts(id)
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct RemoteArgs {
    pub runner_connect: Option<String>,
    pub runner_host: Option<SocketAddr>,
    pub runner_token: Option<String>,
    pub runner_name: Option<String>,
    pub runner_workspace: Option<PathBuf>,
    pub runner_mode: RunnerModeArg,
    pub runner_call_timeout_ms: Option<u64>,
    pub tab: bool,
}

impl RemoteArgs {
    fn from_matches(matches: &Matches) -> Self {
        Self {
            runner_connect: matches.text("runner_connect"),
            runner_host: matches.address("runner_host"),
            runner_token: matches.text("runner_token"),
            runner_name: matches.text("runner_name"),
            runner_workspace: matches.path("runner_workspace"),
            runner_mode: RunnerModeArg::ALL[matches
                .choice("runner_mode")
                .expect("runner_mode has a default")],
            runner_call_timeout_ms: matches.u64("runner_call_timeout_ms"),
            tab: matches.flag("tab"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HandArgs {
    pub url: Option<String>,
    pub listen: Option<SocketAddr>,
    pub token: String,
    pub name: Option<String>,
    pub directory: Option<PathBuf>,
    pub cua: bool,
    pub cell_yield_ms: Option<u64>,
    pub own_processes: bool,
    pub pricing: Option<PathBuf>,
    pub tools_only: bool,
}

impl HandArgs {
    fn from_matches(matches: &Matches) -> Self {
        Self {
            url: matches.text("url"),
            listen: matches.address("listen"),
            token: matches.text("token").expect("token is required"),
            name: matches.text("name"),
            directory: matches.path("directory"),
            cua: matches.flag("cua"),
            cell_yield_ms: matches.u64("cell_yield_ms"),
            own_processes: matches.flag("own_processes"),
            pricing: matches.path("pricing"),
            tools_only: matches.flag("tools_only"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionsArgs {
    pub home: Option<PathBuf>,
    pub all: bool,
    pub limit: Option<u64>,
    pub session: Option<String>,
}

impl SessionsArgs {
    fn from_matches(matches: &Matches) -> Self {
        Self {
            home: matches.path("home"),
            all: matches.flag("all"),
            limit: matches.u64("limit"),
            session: matches.text("session"),
        }
    }
}
