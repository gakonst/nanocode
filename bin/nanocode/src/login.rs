use nanocode::{
    claude::{ClaudeHomeLogin, subscription::ClaudeSubscriptionStatus},
    codex::auth::{ChatGptLogin, chatgpt_auth_file},
};
use nanocode_tui::transcript::NoticeLevel;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{App, AppEvent};

impl App {
    pub(crate) fn login(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "login", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let tx = self.tx.clone();
        match argument.trim() {
            "codex" | "chatgpt" | "openai" => {
                nanocode_platform::spawn_task(codex(tx));
            }
            "claude" | "anthropic" => {
                nanocode_platform::spawn_task(claude(tx));
            }
            "tempo" => {
                nanocode_platform::spawn_task(tempo(tx));
            }
            _ => self.notice(NoticeLevel::Info, "usage: /login codex|claude|tempo"),
        }
    }

    pub(crate) fn fund(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "fund", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        nanocode_platform::spawn_task(fund(
            self.tx.clone(),
            argument.trim().trim_start_matches('$').to_owned(),
        ));
    }

    pub(crate) fn pass(&mut self, argument: &str) {
        match self.host.config.clients.answering() {
            Some(answer) => answer(argument),
            None => self.notice(
                NoticeLevel::Info,
                "pass: no MPP service can ask for an email pass in this session",
            ),
        }
    }

    pub(crate) fn wallet(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "wallet", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        nanocode_platform::spawn_task(wallet(self.tx.clone()));
    }
}

async fn codex(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let path = match chatgpt_auth_file() {
        Ok(path) => path,
        Err(error) => return notice(NoticeLevel::Error, format!("login codex: {error}")),
    };
    let login = match ChatGptLogin::start(path).await {
        Ok(login) => login,
        Err(error) => return notice(NoticeLevel::Error, format!("login codex: {error}")),
    };
    notice(
        NoticeLevel::Info,
        browse("ChatGPT sign-in", login.authorization_url()),
    );
    match login.complete().await {
        Ok(status) => notice(
            NoticeLevel::Info,
            format!(
                "signed in to ChatGPT as {}",
                status.email.unwrap_or(status.account_id)
            ),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login codex: {error}")),
    }
}

async fn claude(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let login = match ClaudeHomeLogin::start().await {
        Ok(login) => login,
        Err(error) => return notice(NoticeLevel::Error, format!("login claude: {error}")),
    };
    notice(
        NoticeLevel::Info,
        browse("Claude sign-in", login.authorization_url()),
    );
    match login.complete().await {
        Ok(ClaudeSubscriptionStatus::Authenticated { .. }) => {
            notice(NoticeLevel::Info, "signed in to Claude".to_owned());
        }
        Ok(status) => notice(
            NoticeLevel::Error,
            format!("login claude ended as {status:?}"),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login claude: {error}")),
    }
}

const TEMPO_PAYS: &str = "it pays for the Tempo gateway's and Mercator's tools and the Tempo services shell commands reach, and --gateway tempo pays for models with it; /fund buys MACH by card";

async fn tempo(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let started = match tempo_wallet(&["wallet", "login", "--no-wait", "--no-browser"]).await {
        Ok(started) => started,
        Err(error) => return notice(NoticeLevel::Error, format!("login tempo: {error}")),
    };
    if let Some(wallet) = started["wallet"].as_str() {
        return notice(
            NoticeLevel::Info,
            format!("signed in to the Tempo wallet {wallet}; {TEMPO_PAYS}"),
        );
    }
    let (Some(url), Some(code)) = (
        started["verification_url"].as_str(),
        started["user_code"].as_str(),
    ) else {
        return notice(
            NoticeLevel::Error,
            format!("login tempo: tempo wallet login answered {started}"),
        );
    };
    notice(
        NoticeLevel::Info,
        format!(
            "{}; check that it shows {code}",
            browse("Tempo sign-in", url)
        ),
    );
    match tempo_wallet(&["wallet", "login", "status"]).await {
        Ok(status) => notice(
            NoticeLevel::Info,
            format!(
                "signed in to the Tempo wallet {}; {TEMPO_PAYS}",
                status["accounts"][0].as_str().unwrap_or_default()
            ),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login tempo: {error}")),
    }
}

async fn fund(tx: UnboundedSender<AppEvent>, amount: String) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let mut args = vec!["wallet", "fund", "--mach", "--no-browser"];
    if !amount.is_empty() {
        args.extend(["--amount", amount.as_str()]);
    }
    let checkout = match tempo_wallet(&args).await {
        Ok(checkout) => checkout,
        Err(error) => return notice(NoticeLevel::Error, format!("fund: {error}")),
    };
    let (Some(url), Some(wallet)) = (checkout["url"].as_str(), checkout["wallet"].as_str()) else {
        return notice(
            NoticeLevel::Error,
            format!("fund: tempo wallet fund answered {checkout}"),
        );
    };
    notice(
        NoticeLevel::Info,
        format!(
            "{}; MACH bought there goes to the Tempo wallet {wallet}, and nanocode pays with it once it arrives",
            browse("the MACH checkout", url)
        ),
    );
}

async fn wallet(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let doctor = match tempo_wallet(&["wallet", "doctor"]).await {
        Ok(doctor) => doctor,
        Err(error) => return notice(NoticeLevel::Error, format!("wallet: {error}")),
    };
    let Some(url) = doctor["auth"]["url"].as_str() else {
        return notice(
            NoticeLevel::Error,
            format!(
                "wallet: tempo wallet doctor named no wallet app: {}",
                doctor["auth"]
            ),
        );
    };
    let held = match doctor["wallet"]["address"].as_str() {
        Some(address) if doctor["wallet"]["logged_in"].as_bool() == Some(true) => {
            format!("nanocode pays from {address}; /fund buys MACH for it")
        }
        _ => "/login tempo signs nanocode in to a wallet".to_owned(),
    };
    notice(
        NoticeLevel::Info,
        format!("{}; {held}", browse("the Tempo wallet app", url)),
    );
}

async fn tempo_wallet(args: &[&str]) -> Result<serde_json::Value, String> {
    let candidates = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".tempo/bin/tempo"))
        .into_iter()
        .chain([std::path::PathBuf::from("tempo")]);
    let mut ran = None;
    for program in candidates {
        match tokio::process::Command::new(&program)
            .args(args)
            .args(["--format", "json"])
            .output()
            .await
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            output => {
                ran = Some((program, output));
                break;
            }
        }
    }
    let Some((program, output)) = ran else {
        return Err(
            "the Tempo CLI is not installed: curl -fsSL https://tempo.xyz/install | bash"
                .to_owned(),
        );
    };
    let output = output.map_err(|error| format!("running {}: {error}", program.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(message) = [&stdout, &stderr].into_iter().find_map(|text| {
            let error: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
            error["message"].as_str().map(str::to_owned)
        }) {
            return Err(message);
        }
        return Err(format!(
            "{} {} exited with {}: {}",
            program.display(),
            args.join(" "),
            output.status,
            [stderr.trim(), stdout.trim()].join(" ").trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "{} {} printed no JSON: {error}",
            program.display(),
            args.join(" ")
        )
    })
}

fn browse(page: &str, url: &str) -> String {
    if crate::extras::open_url(url) {
        format!("opened {page} in your browser; if it did not appear, visit {url}")
    } else {
        format!("visit {url} for {page}")
    }
}
