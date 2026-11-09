use std::{net::SocketAddr, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "nanocode",
    version,
    about = "Interactive coding agent for Codex and Claude",
    long_about = "Interactive coding agent for Codex and Claude. Run without a subcommand to open the terminal interface; 'nc' is an alias of 'nanocode'."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub session: SessionArgs,

    #[command(flatten)]
    pub observability: nanocode_observability::ObservabilityOutputArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(
        visible_alias = "runner",
        about = "Make this machine a hand that runs a nanocode agent's exec cells: dial the brain's URL, or listen for the brain"
    )]
    Hand(HandArgs),
    #[command(about = "List saved sessions (current workspace unless --all)")]
    Sessions(SessionsArgs),
    #[command(
        about = "Run one prompt without the terminal interface, streaming the reply to stdout and reasoning summaries and tool calls to stderr"
    )]
    Exec(SessionArgs),
    #[command(about = "List models per harness with thinking levels and speed tiers")]
    Models,
    #[command(about = "Resume a saved session; without an id, pick one interactively")]
    Resume {
        #[arg(help = "Session id to resume")]
        id: Option<String>,
    },
    #[cfg(unix)]
    #[command(
        about = "Set up computer use: cua-driver for native apps and the nanocode browser extension for background tab groups"
    )]
    Cua(CuaArgs),
}

#[derive(Clone, Debug, Args)]
pub struct CuaArgs {
    #[command(subcommand)]
    pub command: CuaCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum CuaCommand {
    #[command(
        about = "Ask macOS for the permissions of nanocode's built-in cua-driver (elsewhere install cua-driver) and install the nanocode extension's native host in each Chromium browser"
    )]
    Install {
        #[arg(
            long,
            help = "Browser name (Chrome, Edge, Brave, ...) or an absolute Chromium user data directory; repeat for several (defaults to every one found)"
        )]
        browser: Vec<String>,
        #[arg(long, help = "Leave cua-driver and its permissions as they are")]
        skip_driver: bool,
    },
    #[command(about = "Remove the nanocode extension's native host")]
    Uninstall {
        #[arg(
            long,
            help = "Browser name or user data directory (defaults to every one found)"
        )]
        browser: Vec<String>,
    },
    #[command(about = "Show what computer use finds on this machine")]
    Status,
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    DriverMcp,
    #[command(hide = true)]
    BrowserHost {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum HarnessArg {
    #[value(help = "OpenAI Codex models (ChatGPT login)")]
    Codex,
    #[value(help = "Anthropic Claude models (Claude Code login)")]
    Claude,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum CyberAccessProgramArg {
    Standard,
    DaybreakBlue,
    DaybreakRed,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum GatewayArg {
    #[value(help = "OpenRouter Responses API; key from OPENROUTER_API_KEY")]
    Openrouter,
    #[value(help = "Vercel AI Gateway; key from AI_GATEWAY_API_KEY or VERCEL_OIDC_TOKEN")]
    Vercel,
    #[value(
        help = "Cloudflare unified API; token from CLOUDFLARE_API_TOKEN, needs --cloudflare-account"
    )]
    Cloudflare,
    #[value(
        help = "Cloudflare AI Gateway OpenAI proxy; OPENAI_API_KEY, CF_AIG_TOKEN for stored keys, or CLOUDFLARE_API_TOKEN for Unified Billing; needs --cloudflare-account and --cloudflare-gateway"
    )]
    CloudflareGateway,
    #[value(
        help = "Tempo wallet (tempo wallet login): pay Codex and Claude per request over MPP sessions through Tempo's proxies, or with --gateway-base-url one MPP endpoint"
    )]
    Tempo,
    #[value(
        help = "The Tempo gateway's models (gateway.tempo-dev.workers.dev), paid from the Tempo wallet"
    )]
    TempoGateway,
    #[value(
        help = "Boundless inference (api.inference.boundless.network/mpp), paid from the Tempo wallet"
    )]
    Boundless,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum TempoNetworkArg {
    #[value(help = "Tempo mainnet (chain 4217), paid in USDC.e by default")]
    Mainnet,
    #[value(help = "Tempo testnet (chain 42431), paid in pathUSD by default")]
    Testnet,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum RunnerModeArg {
    #[default]
    #[value(help = "Keep local tools; the runner's workspace is reachable under /<runner-name>")]
    Alongside,
    #[value(help = "Run every tool on the runner instead of this machine")]
    Replace,
}

#[derive(Clone, Debug, Args)]
pub struct SessionArgs {
    #[arg(help = "Prompt to send as soon as the session is ready")]
    pub prompt: Option<String>,

    #[arg(
        short,
        long,
        env = "NANOCODE_MODEL",
        help = "Model id or alias (see 'nanocode models'); defaults to claude-opus-5-5 at xhigh thinking"
    )]
    pub model: Option<String>,

    #[arg(long, value_enum, help = "Harness to use when --model is not given")]
    pub harness: Option<HarnessArg>,

    #[arg(
        short,
        long,
        help = "Thinking level supported by the model (none, low, medium, high, xhigh, max)"
    )]
    pub thinking: Option<String>,

    #[arg(
        long,
        value_parser = ["standard", "fast", "ultrafast"],
        help = "Service tier; only tiers the model supports are accepted"
    )]
    pub tier: Option<String>,

    #[arg(
        long,
        value_enum,
        value_name = "PROGRAM",
        help = "Request an experimental Cyber access program on Codex models (OpenAI only, not with --gateway), as Codex's --cyber-access-program; omit to use server defaults"
    )]
    pub cyber_access_program: Option<CyberAccessProgramArg>,

    #[arg(
        short = 'C',
        long = "cd",
        help = "Workspace directory (defaults to the current directory)"
    )]
    pub workspace: Option<PathBuf>,

    #[arg(
        short,
        long,
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "SESSION_ID",
        help = "Resume a saved session; without an id, pick one interactively"
    )]
    pub resume: Option<String>,

    #[arg(long, hide = true)]
    pub reload_state: Option<PathBuf>,

    #[arg(
        short = 'c',
        long = "continue",
        help = "Resume the most recent session in this workspace"
    )]
    pub continue_last: bool,

    #[arg(long, help = "Keep the session in memory only; nothing is saved")]
    pub ephemeral: bool,

    #[arg(
        long,
        env = "NANOCODE_HOME",
        help = "Directory for saved sessions, history and pasted images (defaults to ~/.nanocode)"
    )]
    pub home: Option<PathBuf>,

    #[arg(
        long,
        help = "Leave mouse capture off so the terminal keeps native text selection"
    )]
    pub no_mouse: bool,

    #[arg(long, help = "Disable subagent tools")]
    pub no_subagents: bool,

    #[command(flatten)]
    pub gateway: GatewayArgs,

    #[command(flatten)]
    pub remote: RemoteArgs,
}

#[derive(Clone, Debug, Default, Args)]
pub struct GatewayArgs {
    #[arg(
        long,
        value_enum,
        env = "NANOCODE_GATEWAY",
        help = "Gateway to prefer for the models it serves. Every gateway is offered at once and /model groups the models by gateway: your ChatGPT and Claude logins, Tempo's proxies, the Tempo gateway and Boundless (paid from the Tempo wallet), and OpenRouter, Vercel and Cloudflare when their keys are in the environment (read from there, never saved); agents see every gateway's models (list_models) and can run a subagent on any of them (spawn_agent's gateway), paid as that gateway charges"
    )]
    pub gateway: Option<GatewayArg>,

    #[arg(
        long,
        env = "CLOUDFLARE_ACCOUNT_ID",
        value_name = "ID",
        help = "Cloudflare account id for --gateway cloudflare or cloudflare-gateway"
    )]
    pub cloudflare_account: Option<String>,

    #[arg(
        long,
        env = "CLOUDFLARE_GATEWAY_ID",
        value_name = "ID",
        help = "Cloudflare AI Gateway id; required for cloudflare-gateway, optional for cloudflare (account default gateway when unset)"
    )]
    pub cloudflare_gateway: Option<String>,

    #[arg(
        long,
        value_name = "URL",
        requires = "gateway",
        help = "Override the preferred gateway's base URL, for example a Cloudflare custom domain; with --gateway tempo, an MPP endpoint to pay, offering the models it lists, as one more gateway named by its host"
    )]
    pub gateway_base_url: Option<String>,

    #[arg(
        long,
        value_enum,
        env = "NANOCODE_TEMPO_NETWORK",
        help = "Chain to pay --gateway tempo's --gateway-base-url on, instead of the one its payment challenge names"
    )]
    pub tempo_network: Option<TempoNetworkArg>,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_CURRENCY",
        value_name = "TOKEN",
        help = "Token to prefer paying MPP endpoints (Boundless, the Tempo gateway and --gateway tempo's --gateway-base-url) in: mach, usdc.e or ousd (mainnet), pathusd (testnet) or a token address; an endpoint that does not offer it is paid in one it does, in sessions of their own"
    )]
    pub tempo_currency: Option<String>,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_FEE_TOKEN",
        value_name = "TOKEN",
        help = "Token to pay the gas of MPP endpoint (Boundless, the Tempo gateway and --gateway tempo's --gateway-base-url) session opens and top-ups in, instead of the fee token its payment challenge names: usdc.e or usdt0 (mainnet), pathusd or a token address; MACH and OUSD cannot pay gas"
    )]
    pub tempo_fee_token: Option<String>,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_SESSION",
        value_name = "NAME",
        help = "Pay each Tempo service (--gateway tempo's endpoint or proxies, the paid tools and the services shell commands reach) from this named session instead of the lowest of 64 slots no other process holds"
    )]
    pub tempo_session: Option<String>,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_CLOSE_SESSION",
        help = "Close the sessions of the MPP endpoints this run paid cooperatively on exit, which returns the unspent deposits at once, instead of keeping them open for the next run to reuse"
    )]
    pub tempo_close_session: bool,

    #[arg(
        long,
        hide = true,
        env = "NANOCODE_TEMPO_KEEP_SESSION",
        conflicts_with = "tempo_close_session"
    )]
    pub tempo_keep_session: bool,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_DEPOSIT",
        value_name = "BASE_UNITS",
        help = "Model session deposit and top-up for the gateways the Tempo wallet pays, in the token's base units (6 decimals), when the service suggests no deposit of its own: 2000000 (2 MACH) on Tempo's proxies and 1000000 (1 of the token) on an MPP endpoint unless set"
    )]
    pub tempo_deposit: Option<u128>,

    #[arg(
        long,
        env = "NANOCODE_TEMPO_TOOL_DEPOSIT",
        value_name = "BASE_UNITS",
        help = "Session deposit and top-up, in base units, for the Tempo gateway's tools and the paid services shell commands reach, when a service suggests no deposit of its own (the Tempo gateway suggests 1 MACH): 100000 (0.1 MACH) unless set"
    )]
    pub tempo_tool_deposit: Option<u128>,

    #[arg(
        long,
        env = "NANOCODE_NO_TEMPO_TOOLS",
        help = "Leave out the paid tools of the Tempo gateway (key-value store, buckets, compute) and Mercator, and the local proxy through which shell commands pay Tempo's services (*.mpp.tempo.xyz), the Tempo gateway and Mercator; every session has them by default and pays from the Tempo wallet (/login tempo, /fund) when one charges. A session whose environment sets its own HTTP(S)_PROXY, ALL_PROXY or CA bundle keeps those and gets no proxy"
    )]
    pub no_tempo_tools: bool,
}

impl SessionArgs {
    pub fn or(self, root: Self) -> Self {
        Self {
            prompt: self.prompt.or(root.prompt),
            model: self.model.or(root.model),
            harness: self.harness.or(root.harness),
            thinking: self.thinking.or(root.thinking),
            tier: self.tier.or(root.tier),
            cyber_access_program: self.cyber_access_program.or(root.cyber_access_program),
            workspace: self.workspace.or(root.workspace),
            resume: self.resume.or(root.resume),
            reload_state: self.reload_state.or(root.reload_state),
            continue_last: self.continue_last || root.continue_last,
            ephemeral: self.ephemeral || root.ephemeral,
            home: self.home.or(root.home),
            no_mouse: self.no_mouse || root.no_mouse,
            no_subagents: self.no_subagents || root.no_subagents,
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
                tempo_network: self.gateway.tempo_network.or(root.gateway.tempo_network),
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

#[derive(Clone, Debug, Default, Args)]
pub struct RemoteArgs {
    #[arg(
        long,
        value_name = "WS_URL",
        conflicts_with = "runner_host",
        help = "Connect to a runner started with 'nanocode runner --listen'"
    )]
    pub runner_connect: Option<String>,

    #[arg(
        long,
        value_name = "ADDR",
        help = "Listen for a runner started with 'nanocode runner --connect'"
    )]
    pub runner_host: Option<SocketAddr>,

    #[arg(
        long,
        env = "NANOCODE_RUNNER_TOKEN",
        hide_env_values = true,
        help = "Shared secret authenticating the runner connection"
    )]
    pub runner_token: Option<String>,

    #[arg(
        long,
        help = "Runner name; also the mount point /<name> in alongside mode. Defaults to runner, or to the name a Code Mode executor advertises"
    )]
    pub runner_name: Option<String>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Override the physical workspace advertised by the runner"
    )]
    pub runner_workspace: Option<PathBuf>,

    #[arg(
        long,
        value_enum,
        default_value_t,
        help = "Use the runner alongside local tools or instead of them"
    )]
    pub runner_mode: RunnerModeArg,

    #[arg(
        long,
        value_name = "MS",
        help = "Hard deadline for each remote tool call, in milliseconds"
    )]
    pub runner_call_timeout_ms: Option<u64>,

    #[arg(
        long,
        conflicts_with_all = ["runner_connect", "runner_host"],
        help = "Make a browser tab this session's hand: prints a nanocode.xyz/hand link that pairs the tab with this agent through the nanocode.xyz hand relay, and waits for it"
    )]
    pub tab: bool,
}

#[derive(Clone, Debug, Args)]
pub struct HandArgs {
    #[arg(
        value_name = "URL",
        required_unless_present = "listen",
        conflicts_with = "listen",
        help = "Brain endpoint to dial, such as wss://<worker>/sessions/<name>/hands; the hand reconnects until the brain disconnects it"
    )]
    pub url: Option<String>,

    #[arg(
        long,
        value_name = "ADDR",
        help = "Listen on this loopback address for a brain to dial instead"
    )]
    pub listen: Option<SocketAddr>,

    #[arg(
        long,
        env = "NANOCODE_RUNNER_TOKEN",
        hide_env_values = true,
        help = "Shared secret presented to or required from the brain"
    )]
    pub token: String,

    #[arg(long, help = "Hand name (defaults to this machine's host name)")]
    pub name: Option<String>,

    #[arg(
        short = 'C',
        long = "cd",
        help = "Starting directory (defaults to the current directory)"
    )]
    pub directory: Option<PathBuf>,

    #[arg(long, help = "Also serve computer use from the installed provider")]
    pub cua: bool,

    #[arg(
        long,
        value_name = "FILE",
        help = "Charge for operations by the rules in this JSON file, matched first to last: {\"payee\": \"0x…\", \"describe\": \"…\", \"currency\": \"OUSD\" (default) or \"USDC.e\", \"rules\": [{\"paths\": [\"data/**\"], \"ops\": [\"read\"], \"usd\": 0.01}]}"
    )]
    pub pricing: Option<PathBuf>,

    #[arg(
        long,
        help = "Run no scripts here: agents call this hand's tools one at a time, so --pricing sees every file and command"
    )]
    pub tools_only: bool,
}

#[derive(Clone, Debug, Args)]
pub struct SessionsArgs {
    #[arg(
        long,
        env = "NANOCODE_HOME",
        help = "Session store directory (defaults to ~/.nanocode)"
    )]
    pub home: Option<PathBuf>,

    #[arg(long, help = "Include sessions from every workspace")]
    pub all: bool,

    #[arg(help = "Print this session's user and assistant messages instead of listing sessions")]
    pub session: Option<String>,
}
