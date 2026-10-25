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
        about = "Install cua-driver (and ask for its macOS permissions) and the nanocode extension's native host in each Chromium browser"
    )]
    Install {
        #[arg(
            long,
            help = "Browser name (Chrome, Edge, Brave, ...) or an absolute Chromium user data directory; repeat for several (defaults to every one found)"
        )]
        browser: Vec<String>,
        #[arg(long, help = "Leave cua-driver as it is")]
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
        help = "Route Codex models (gpt-6.1-sol, luna, astra) through a Responses gateway for this process, including /new, /model and resumed sessions; keys are read from the provider's environment variable and never saved; Claude models keep their own login; restart to change"
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
        help = "Override the gateway base URL, for example a Cloudflare custom domain"
    )]
    pub gateway_base_url: Option<String>,
}

impl SessionArgs {
    pub fn or(self, root: Self) -> Self {
        Self {
            prompt: self.prompt.or(root.prompt),
            model: self.model.or(root.model),
            harness: self.harness.or(root.harness),
            thinking: self.thinking.or(root.thinking),
            tier: self.tier.or(root.tier),
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
